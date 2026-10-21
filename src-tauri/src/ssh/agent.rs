//! ssh-agent discovery and verification (§5.5).
//!
//! `connect_verified(endpoint)`:
//! - **Windows (pipe):** the endpoint must match `\\.\pipe\<name>` exactly
//!   (`platform::local_path::valid_agent_pipe_name`, checked before opening); open with a retry on
//!   `ERROR_PIPE_BUSY` every 20 ms for up to 2 s; `GetSecurityInfo(handle, SE_KERNEL_OBJECT,
//!   OWNER)` **on that same handle**, which is then moved into `AgentClient::connect` (no
//!   re-open). Accepted owners: current user, LocalSystem, Administrators. Any API error or NULL
//!   owner ⇒ untrusted.
//! - **Unix (socket):** `UnixStream::connect` → `peer_cred().uid()` must be ours or root.
//!
//! Availability = verified connect **and** a successful first `request_identities()`.

#[cfg(windows)]
use std::time::Duration;

use russh::keys::agent::AgentIdentity;
use russh::keys::agent::client::{AgentClient, AgentStream};
use russh::keys::{HashAlg, PublicKey};

use crate::error::{AppError, ErrorCode};
use crate::ipc::dto::{AgentKey, AgentKind, AgentStatus};

/// The Windows OpenSSH agent's pipe (default endpoint on Windows).
pub const OPENSSH_PIPE: &str = r"\\.\pipe\openssh-ssh-agent";
/// Budget for one agent request that needs no user interaction (connect, list, add, remove):
/// A wedged agent must not leave a session "authenticating" or block its close.
/// Signing keeps no such bound (an agent may show a confirm dialog).
pub const AGENT_OP_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(windows)]
const PIPE_BUSY_RETRY: Duration = Duration::from_millis(20);
#[cfg(windows)]
const PIPE_BUSY_BUDGET: Duration = Duration::from_secs(2);

/// Where to find the agent after `~`/env expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEndpoint {
    /// `\\.\pipe\<name>` (Windows).
    Pipe(String),
    /// Unix-domain socket path.
    Socket(std::path::PathBuf),
}

impl AgentEndpoint {
    /// Interpret a configured endpoint (`IdentityAgent` value or `SSH_AUTH_SOCK`): on Windows
    /// every endpoint is a pipe and must pass the grammar (`\\.\pipe\x` or `//./pipe/x`,
    /// normalised to the backslash form); on Unix it is a socket path.
    pub fn parse(raw: &str) -> Option<Self> {
        if cfg!(windows) {
            crate::platform::local_path::normalize_agent_pipe(raw).map(Self::Pipe)
        } else if raw.starts_with('/') && !raw.contains('\0') {
            Some(Self::Socket(raw.into()))
        } else {
            None
        }
    }

    pub fn kind(&self) -> AgentKind {
        match self {
            Self::Pipe(p) if p.eq_ignore_ascii_case(OPENSSH_PIPE) => AgentKind::OpensshPipe,
            Self::Pipe(_) => AgentKind::NamedPipe,
            Self::Socket(_) => AgentKind::UnixSocket,
        }
    }

    pub fn display(&self) -> String {
        match self {
            Self::Pipe(p) => p.clone(),
            Self::Socket(p) => p.display().to_string(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// Endpoint missing / not running / busy too long / protocol failure.
    #[error("agent unavailable: {0}")]
    Unavailable(String),
    /// Endpoint exists but its owner is not acceptable (or could not be determined).
    #[error("agent untrusted: {0}")]
    Untrusted(String),
}

/// Boxed agent stream; `Sync` so a session supervisor holding the agent stays `Send`.
pub type AgentBox = Box<dyn AgentStream + Send + Sync + Unpin + 'static>;
pub type DynAgent = AgentClient<AgentBox>;

/// A connected, owner-verified agent.
pub struct VerifiedAgent {
    pub client: DynAgent,
    pub endpoint: AgentEndpoint,
}

impl VerifiedAgent {
    pub fn kind(&self) -> AgentKind {
        self.endpoint.kind()
    }
}

/// Connect to `endpoint` and verify its owner (no request sent yet).
pub async fn connect_verified(endpoint: &AgentEndpoint) -> Result<VerifiedAgent, AgentError> {
    let client = match endpoint {
        AgentEndpoint::Pipe(name) => connect_pipe(name).await?,
        AgentEndpoint::Socket(path) => connect_socket(path).await?,
    };
    Ok(VerifiedAgent {
        client,
        endpoint: endpoint.clone(),
    })
}

#[cfg(windows)]
async fn connect_pipe(name: &str) -> Result<DynAgent, AgentError> {
    use crate::platform::win_security::{
        SID_ADMINISTRATORS, SID_LOCAL_SYSTEM, current_user_sid, security_of_kernel_object,
    };
    use tokio::net::windows::named_pipe::ClientOptions;
    const ERROR_PIPE_BUSY: i32 = 231;

    if !crate::platform::local_path::valid_agent_pipe_name(name) {
        return Err(AgentError::Untrusted(format!(
            "invalid pipe endpoint {name:?}"
        )));
    }
    let deadline = tokio::time::Instant::now() + PIPE_BUSY_BUDGET;
    let pipe = loop {
        match ClientOptions::new().open(name) {
            Ok(p) => break p,
            Err(e)
                if e.raw_os_error() == Some(ERROR_PIPE_BUSY)
                    && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(PIPE_BUSY_RETRY).await;
            }
            Err(e) => return Err(AgentError::Unavailable(format!("open {name}: {e}"))),
        }
    };
    // Owner of the pipe object, checked on the handle we will use (no TOCTOU).
    let info = security_of_kernel_object(&pipe)
        .map_err(|e| AgentError::Untrusted(format!("GetSecurityInfo: {e}")))?;
    let me =
        current_user_sid().map_err(|e| AgentError::Untrusted(format!("current user SID: {e}")))?;
    match info.owner.as_deref() {
        Some(owner) if owner == me || owner == SID_LOCAL_SYSTEM || owner == SID_ADMINISTRATORS => {
            Ok(AgentClient::connect(Box::new(pipe) as AgentBox))
        }
        Some(owner) => Err(AgentError::Untrusted(format!(
            "pipe owner {owner} not accepted"
        ))),
        None => Err(AgentError::Untrusted("pipe has no owner".into())),
    }
}

#[cfg(not(windows))]
async fn connect_pipe(name: &str) -> Result<DynAgent, AgentError> {
    Err(AgentError::Unavailable(format!(
        "named pipes are Windows-only: {name}"
    )))
}

#[cfg(unix)]
async fn connect_socket(path: &std::path::Path) -> Result<DynAgent, AgentError> {
    let stream = tokio::net::UnixStream::connect(path)
        .await
        .map_err(|e| AgentError::Unavailable(format!("connect {}: {e}", path.display())))?;
    let cred = stream
        .peer_cred()
        .map_err(|e| AgentError::Untrusted(format!("peer_cred: {e}")))?;
    // SAFETY: getuid has no preconditions.
    let me = unsafe { libc::getuid() };
    if cred.uid() != me && cred.uid() != 0 {
        return Err(AgentError::Untrusted(format!(
            "socket peer uid {} not accepted",
            cred.uid()
        )));
    }
    Ok(AgentClient::connect(Box::new(stream) as AgentBox))
}

#[cfg(not(unix))]
async fn connect_socket(path: &std::path::Path) -> Result<DynAgent, AgentError> {
    Err(AgentError::Unavailable(format!(
        "unix sockets are not supported here: {}",
        path.display()
    )))
}

/// One identity held by the agent (plain public keys only; certificates are ignored, §1.1).
#[derive(Debug, Clone)]
pub struct AgentIdentityInfo {
    pub key: PublicKey,
    pub comment: String,
}

/// `request_identities()`; certificate identities are skipped.
pub async fn list_identities(
    agent: &mut VerifiedAgent,
) -> Result<Vec<AgentIdentityInfo>, AgentError> {
    let ids = tokio::time::timeout(AGENT_OP_TIMEOUT, agent.client.request_identities())
        .await
        .map_err(|_| AgentError::Unavailable("request_identities: timed out".to_owned()))?
        .map_err(|e| AgentError::Unavailable(format!("request_identities: {e}")))?;
    Ok(ids
        .into_iter()
        .filter_map(|id| match id {
            AgentIdentity::PublicKey { key, comment } => Some(AgentIdentityInfo { key, comment }),
            AgentIdentity::Certificate { .. } => None,
        })
        .collect())
}

/// The app-wide default endpoint (§5.5 without a host's `IdentityAgent`): env `SSH_AUTH_SOCK`,
/// then the Windows OpenSSH pipe. `Err(raw)` if the env value fails the endpoint grammar.
pub fn default_endpoint(
    env: &crate::sshconfig::EnvLookup,
) -> Result<Option<AgentEndpoint>, String> {
    match env("SSH_AUTH_SOCK").filter(|v| !v.is_empty()) {
        Some(raw) => AgentEndpoint::parse(&raw).map(Some).ok_or(raw),
        None => Ok(fallback_pipe().map(AgentEndpoint::Pipe)),
    }
}

/// The endpoint used when `SSH_AUTH_SOCK` is unset: the Windows OpenSSH agent pipe. None
/// elsewhere, and none in an `e2e` test build (the real user's agent is user state, src/e2e.rs).
pub fn fallback_pipe() -> Option<String> {
    (cfg!(windows) && !crate::e2e::is_active()).then(|| OPENSSH_PIPE.to_owned())
}

/// `agent_status` (§4.2): verified connect + `request_identities`. Owner mismatch ⇒ unavailable
/// with an `agentUntrusted` warning.
pub async fn status(env: &crate::sshconfig::EnvLookup) -> AgentStatus {
    let unavailable = |kind, endpoint, warning| AgentStatus {
        available: false,
        kind,
        endpoint,
        identities: Vec::new(),
        warning,
    };
    let endpoint = match default_endpoint(env) {
        Ok(Some(e)) => e,
        Ok(None) => return unavailable(None, None, None),
        Err(raw) => {
            return unavailable(
                None,
                Some(raw),
                Some(
                    AppError::new(ErrorCode::AgentUntrusted).with_detail("invalid agent endpoint"),
                ),
            );
        }
    };
    let kind = Some(endpoint.kind());
    let shown = Some(endpoint.display());
    match connect_verified(&endpoint).await {
        Err(AgentError::Untrusted(why)) => unavailable(
            kind,
            shown,
            Some(AppError::new(ErrorCode::AgentUntrusted).with_detail(why)),
        ),
        Err(AgentError::Unavailable(_)) => unavailable(kind, shown, None),
        Ok(mut agent) => match list_identities(&mut agent).await {
            Ok(ids) => AgentStatus {
                available: true,
                kind,
                endpoint: shown,
                identities: ids.iter().map(agent_key_dto).collect(),
                warning: None,
            },
            Err(_) => unavailable(kind, shown, None),
        },
    }
}

/// DTO for `AgentStatus.identities` / `LocalKeyInfo.inAgent` matching.
pub fn agent_key_dto(id: &AgentIdentityInfo) -> AgentKey {
    AgentKey {
        key_type: id.key.algorithm().to_string(),
        fingerprint: id.key.fingerprint(HashAlg::Sha256).to_string(),
        comment: id.comment.clone(),
        bits: key_bits(&id.key),
    }
}

/// Key size for RSA keys (modulus bits); `None` for fixed-size algorithms.
pub fn key_bits(key: &PublicKey) -> Option<u32> {
    key.key_data()
        .rsa()
        .map(russh::keys::ssh_key::public::RsaPublicKey::key_size)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn endpoint_parsing_and_kind() {
        if cfg!(windows) {
            let e = AgentEndpoint::parse(r"\\.\pipe\openssh-ssh-agent").unwrap();
            assert_eq!(e.kind(), AgentKind::OpensshPipe);
            let e = AgentEndpoint::parse(r"\\.\PIPE\pageant.alice.abc").unwrap();
            assert_eq!(e.kind(), AgentKind::NamedPipe);
            assert!(AgentEndpoint::parse(r"\\.\pipe\..\UNC\h\s\x").is_none());
            assert!(AgentEndpoint::parse(r"\\host\pipe\x").is_none());
            assert!(AgentEndpoint::parse("/tmp/agent.sock").is_none());
        } else {
            let e = AgentEndpoint::parse("/tmp/ssh-x/agent.1").unwrap();
            assert_eq!(e.kind(), AgentKind::UnixSocket);
            assert!(AgentEndpoint::parse("relative.sock").is_none());
        }
    }

    #[tokio::test]
    async fn missing_endpoint_is_unavailable() {
        let e = if cfg!(windows) {
            AgentEndpoint::Pipe(r"\\.\pipe\shellfin-test-no-such-agent".into())
        } else {
            AgentEndpoint::Socket("/nonexistent/shellfin-test.sock".into())
        };
        assert!(matches!(
            connect_verified(&e).await,
            Err(AgentError::Unavailable(_))
        ));
    }

    /// Our own pipe server (owner = current user) is accepted by the owner check.
    #[cfg(windows)]
    #[tokio::test]
    async fn own_pipe_is_accepted() {
        use tokio::net::windows::named_pipe::ServerOptions;
        let name = format!(r"\\.\pipe\shellfin-test-agent-{}", uuid::Uuid::new_v4());
        let server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&name)
            .unwrap();
        let accept = tokio::spawn(async move { server.connect().await.map(|()| server) });
        let agent = connect_verified(&AgentEndpoint::Pipe(name.clone())).await;
        assert!(agent.is_ok(), "{:?}", agent.err());
        drop(accept);
    }
}
