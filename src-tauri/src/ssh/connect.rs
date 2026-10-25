//! Dialing and hop chaining (§5.2), error mapping (§4.3), eager SFTP init (§6.1).
//!
//! ```text
//! hop[0]: lookup_host ─fail→ dnsFailed; per addr: TcpStream::connect, all under ONE
//!         connect_timeout deadline (lookup included)
//!         set_nodelay(true); connect_stream(cfg0, tcp, ClientHandler{hop 0}); authenticate(hop 0)
//! hop[i]: prev.channel_open_direct_tcpip(hostname, port, "127.0.0.1", 0) ─fail→ jumpChannelFailed
//!         connect_stream(cfg_i, ch.into_stream(), ClientHandler{hop i}); authenticate(hop i)
//! then:   SFTP init on the destination (≤ 10 s) → Connected
//! ```
//! The handshake timer covers network phases only and is paused while a prompt is open.

use std::borrow::Cow;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use russh::client::{self, Handle};
use russh::keys::Algorithm;
use russh_sftp::client::RawSftpSession;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

use crate::error::{AppError, DisconnectKind, ErrorCode};
use crate::ipc::dto::{
    HopInfo, HopRef, HostKeyVerification, PromptClosedReason, RemotePathStyle, SessionAuthInfo,
    SessionEvent, SessionState,
};
use crate::known_hosts::{self, KnownHostsFile, Origin, TrustOnceApprovals};
use crate::platform::file_trust::FileKind;
use crate::platform::local_path::{LocalPathError, read_local};
use crate::ssh::agent::{self, AgentEndpoint, VerifiedAgent};
use crate::ssh::auth::{self, AuthEnv, AuthOutcome, HopAuthConfig, HopSecrets, RusshTransport};
use crate::ssh::handler::{
    ClientHandler, HopShared, HostKeyOutcome, HostKeyPin, HostKeyPolicy, effective_strict,
};
use crate::ssh::prompt::PromptBroker;
use crate::ssh::target::{
    AgentChoice, KnownHostsFile as KnownHostsOpt, ResolvedHop, ResolvedTarget,
};

/// SFTP init budget (§6.1).
pub const SFTP_INIT_TIMEOUT: Duration = Duration::from_secs(10);
/// Per-request SFTP timeout (§6.1).
pub const SFTP_REQUEST_TIMEOUT_SECS: u64 = 30;
const TIMER_TICK: Duration = Duration::from_millis(100);

/// Keys this session added to an agent, owned by the session (not by one connect attempt), so
/// they are removed on every exit path: failure, cancel, re-dial, loss, close.
pub type AgentKeyRecord = Arc<std::sync::Mutex<Vec<(AgentEndpoint, russh::keys::PublicKey)>>>;

/// How often a running authentication checks its hop for a lost connection (to withdraw an
/// open prompt with `promptClosed{connectionLost}`).
const AUTH_LOSS_POLL: Duration = Duration::from_millis(200);

/// Environment of a connection attempt.
#[derive(Clone)]
pub struct ConnectCtx {
    /// Session-owned record of keys added to agents.
    pub agent_keys: AgentKeyRecord,
    pub broker: Arc<PromptBroker>,
    pub approvals: Arc<TrustOnceApprovals>,
    /// Windows: `%USERPROFILE%` (known_hosts learning is confined to it, §5.3).
    pub profile_root: Option<PathBuf>,
    pub home: PathBuf,
    pub user_ssh_dir: PathBuf,
    /// `SSH_AUTH_SOCK` lookup (env).
    pub env: crate::sshconfig::EnvLookup,
    pub keychain_available: bool,
    /// `connection.rememberPasswords`: `false` = prompts never offer to remember.
    pub remember_passwords: bool,
}

/// Byte counters of the outer TCP stream (all hops' traffic flows through it).
#[derive(Debug, Default)]
pub struct Traffic {
    pub tx: AtomicU64,
    pub rx: AtomicU64,
}

/// `TcpStream` wrapper feeding [`Traffic`] (session stats, §3).
pub struct CountingStream {
    inner: TcpStream,
    traffic: Arc<Traffic>,
}

impl AsyncRead for CountingStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let res = Pin::new(&mut self.inner).poll_read(cx, buf);
        let n = buf.filled().len().saturating_sub(before);
        self.traffic.rx.fetch_add(n as u64, Ordering::Relaxed);
        res
    }
}

impl AsyncWrite for CountingStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let res = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = &res {
            self.traffic.tx.fetch_add(*n as u64, Ordering::Relaxed);
        }
        res
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// One connected (and authenticated) hop.
pub struct HopConnection {
    pub handle: Arc<Handle<ClientHandler>>,
    pub shared: Arc<HopShared>,
    pub pin: HostKeyPin,
    pub info: HopInfo,
}

/// A fully established session transport.
pub struct Connection {
    /// Jumps first, destination last.
    pub hops: Vec<HopConnection>,
    pub sftp: Option<Arc<RawSftpSession>>,
    pub sftp_extensions: Vec<String>,
    /// `limits@openssh.com` values (read/write lengths; `None` = not advertised).
    pub sftp_limits: SftpLimits,
    pub home_dir: Option<String>,
    pub start_dir: Option<String>,
    pub remote_path_style: RemotePathStyle,
    /// `id -u; id -G` of the destination account; `None` when unknown.
    pub remote_identity: Option<crate::ipc::dto::RemoteIdentity>,
    pub auth: SessionAuthInfo,
    pub traffic: Arc<Traffic>,
    /// Bookmark `startDir` did not exist (fallback to home, §6.1).
    pub start_dir_warning: Option<AppError>,
    /// Passwords the user asked to save (hop index, password); stored by the supervisor once
    /// the whole session is connected (§5.6 "success && remember ⇒ keychain set with binding").
    pub remember_passwords: Vec<(usize, zeroize::Zeroizing<String>)>,
    /// Set by the supervisor when it gives this connection up (loss, probe failure, close), so
    /// requests still running on it report `notConnected` / `disconnected` (§5.8).
    pub abandoned: std::sync::atomic::AtomicBool,
}

/// Server-advertised SFTP request limits (§6.1, `limits@openssh.com`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SftpLimits {
    pub read_len: Option<u64>,
    pub write_len: Option<u64>,
}

impl Connection {
    /// The connection is gone or was given up by the supervisor.
    pub fn is_gone(&self) -> bool {
        self.abandoned.load(Ordering::Relaxed) || self.loss().is_some()
    }

    /// `disconnected{reason}` if any hop's transport ended (a lost jump ⇒ `jumpHostLost`).
    pub fn loss(&self) -> Option<AppError> {
        let last = self.hops.len().saturating_sub(1);
        for (i, hop) in self.hops.iter().enumerate() {
            if hop.handle.is_closed() || hop.shared.loss().is_some() {
                let reason = if i < last {
                    DisconnectKind::JumpHostLost
                } else {
                    hop.shared.loss().unwrap_or(DisconnectKind::ConnectionReset)
                };
                let message = hop.shared.disconnect_message();
                return Some(AppError::new(ErrorCode::Disconnected { reason, message }));
            }
        }
        None
    }

    /// `true` if the server advertised the extension at SFTP init (e.g. `posix-rename@openssh.com`).
    pub fn has_sftp_extension(&self, name: &str) -> bool {
        self.sftp_extensions.iter().any(|e| e == name)
    }

    /// Disconnect destination first, then jumps in reverse (§5.2).
    pub async fn disconnect(&self) {
        for hop in self.hops.iter().rev() {
            let _ = hop
                .handle
                .disconnect(russh::Disconnect::ByApplication, "", "en")
                .await;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// error mapping (§4.3)
// ---------------------------------------------------------------------------------------------

fn is_reset(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::ConnectionReset
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::UnexpectedEof
            | io::ErrorKind::ConnectionAborted
    )
}

/// Map a russh error of hop `hop` (after the TCP dial) to the contract.
/// `timeout_secs` is the hop's connect timeout (reported in `connectTimeout{seconds}`).
pub fn map_russh(
    err: &russh::Error,
    hop: &HopRef,
    shared: &HopShared,
    timeout_secs: u32,
) -> AppError {
    use russh::Error as E;
    let detail = format!("{err:?}");
    let code = match err {
        E::UnknownKey => match shared.host_key() {
            Some(HostKeyOutcome::Rejected(e)) => return e,
            _ => ErrorCode::HostKeyRejected {
                host: hop.host.clone(),
                port: hop.port,
            },
        },
        E::KeepaliveTimeout => ErrorCode::Disconnected {
            reason: DisconnectKind::KeepaliveTimeout,
            message: None,
        },
        E::IO(e) if is_reset(e) => ErrorCode::Disconnected {
            reason: DisconnectKind::ConnectionReset,
            message: None,
        },
        E::Disconnect | E::HUP | E::SendError | E::RecvError => {
            let message = shared
                .disconnect_message
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            ErrorCode::Disconnected {
                reason: DisconnectKind::ServerClosed,
                message,
            }
        }
        E::ConnectionTimeout | E::Elapsed(_) => ErrorCode::ConnectTimeout {
            host: hop.host.clone(),
            port: hop.port,
            seconds: timeout_secs,
        },
        E::KexInit | E::UnknownAlgo | E::NoCommonAlgo { .. } | E::Kex => ErrorCode::KexFailed,
        E::Version
        | E::PacketAuth
        | E::Inconsistent
        | E::WrongServerSig
        | E::PacketSize(_)
        | E::DecryptionError
        | E::StrictKeyExchangeViolation { .. } => ErrorCode::ProtocolError,
        E::IO(_) => ErrorCode::Disconnected {
            reason: DisconnectKind::NetworkError,
            message: None,
        },
        _ => ErrorCode::Internal,
    };
    AppError::new(code).with_detail(detail)
}

fn dial_error(e: &io::Error, host: &str, port: u16, seconds: u32) -> AppError {
    let code = match e.kind() {
        io::ErrorKind::ConnectionRefused => ErrorCode::ConnectRefused {
            host: host.to_owned(),
            port,
        },
        io::ErrorKind::HostUnreachable
        | io::ErrorKind::NetworkUnreachable
        | io::ErrorKind::AddrNotAvailable => ErrorCode::NetworkUnreachable {
            host: host.to_owned(),
        },
        io::ErrorKind::TimedOut => ErrorCode::ConnectTimeout {
            host: host.to_owned(),
            port,
            seconds,
        },
        _ => ErrorCode::NetworkUnreachable {
            host: host.to_owned(),
        },
    };
    AppError::new(code).with_detail(e.to_string())
}

/// `lookup_host` + `TcpStream::connect` per address, all under ONE deadline (like
/// OpenSSH's `ConnectTimeout`): the DNS lookup and every address share `timeout`, so a slow
/// resolver or many unreachable A/AAAA records cannot multiply it. `set_nodelay(true)`.
pub async fn dial(host: &str, port: u16, timeout: Duration) -> Result<TcpStream, AppError> {
    let stream = dial_with(
        host,
        port,
        timeout,
        tokio::net::lookup_host((host, port)),
        TcpStream::connect,
    )
    .await?;
    let _ = stream.set_nodelay(true);
    Ok(stream)
}

/// [`dial`] with the lookup and the connect step injected (tests).
async fn dial_with<A, S, L, C, CF>(
    host: &str,
    port: u16,
    timeout: Duration,
    lookup: L,
    connect: C,
) -> Result<S, AppError>
where
    A: IntoIterator<Item = SocketAddr>,
    L: Future<Output = io::Result<A>>,
    C: Fn(SocketAddr) -> CF,
    CF: Future<Output = io::Result<S>>,
{
    let seconds = u32::try_from(timeout.as_secs()).unwrap_or(u32::MAX);
    let timed_out = || {
        AppError::new(ErrorCode::ConnectTimeout {
            host: host.to_owned(),
            port,
            seconds,
        })
    };
    let deadline = tokio::time::Instant::now() + timeout;
    let addrs: Vec<SocketAddr> = match tokio::time::timeout_at(deadline, lookup).await {
        Err(_) => return Err(timed_out().with_detail("DNS lookup timed out")),
        Ok(Err(e)) => {
            return Err(AppError::new(ErrorCode::DnsFailed {
                host: host.to_owned(),
            })
            .with_detail(e.to_string()));
        }
        Ok(Ok(addrs)) => addrs.into_iter().collect(),
    };
    if addrs.is_empty() {
        return Err(AppError::new(ErrorCode::DnsFailed {
            host: host.to_owned(),
        }));
    }
    let mut last = None;
    for addr in addrs {
        match tokio::time::timeout_at(deadline, connect(addr)).await {
            Ok(Ok(stream)) => return Ok(stream),
            Ok(Err(e)) => last = Some(dial_error(&e, host, port, seconds)),
            Err(_) => return Err(timed_out()),
        }
    }
    Err(last.unwrap_or_else(|| {
        AppError::new(ErrorCode::NetworkUnreachable {
            host: host.to_owned(),
        })
    }))
}

/// Run `fut` with a time limit that only counts while `paused()` is false (prompts, §5.2).
pub async fn with_pausable_timeout<F: Future>(
    fut: F,
    limit: Duration,
    paused: impl Fn() -> bool,
) -> Option<F::Output> {
    tokio::pin!(fut);
    let mut elapsed = Duration::ZERO;
    loop {
        tokio::select! {
            out = &mut fut => return Some(out),
            () = tokio::time::sleep(TIMER_TICK) => {
                if !paused() {
                    elapsed += TIMER_TICK;
                    if elapsed >= limit {
                        return None;
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// per-hop preparation
// ---------------------------------------------------------------------------------------------

/// Host-key algorithms (§5.2): russh defaults minus `ssh-rsa` (SHA-1) unless the trusted config
/// re-adds it, with types already recorded in known_hosts for the lookup first.
pub fn preferred_host_key_algs(add_ssh_rsa: bool, known_types: &[String]) -> Vec<Algorithm> {
    let mut algs: Vec<Algorithm> = russh::Preferred::DEFAULT
        .key
        .iter()
        .filter(|a| !matches!(a, Algorithm::Rsa { hash: None }))
        .cloned()
        .collect();
    if add_ssh_rsa {
        algs.push(Algorithm::Rsa { hash: None });
    }
    let family = |a: &Algorithm| -> String {
        match a {
            Algorithm::Rsa { .. } => "ssh-rsa".to_owned(),
            other => other.as_str().to_owned(),
        }
    };
    algs.sort_by_key(|a| !known_types.contains(&family(a)));
    algs
}

struct KnownHostsSetup {
    /// Shared by every re-dial of the hop (no deep copy of up to 4 MiB per policy).
    files: Arc<[KnownHostsFile]>,
    learn_target: Option<PathBuf>,
    /// Files that exist but were ignored (untrusted, too large, unreadable), for the
    /// `knownHostsIgnored` warning (§5.3).
    ignored: Vec<String>,
    /// A *user* file among them: hosts the user recorded may look new.
    user_file_ignored: bool,
    /// The first user file is `none`/`/dev/null` (nothing to record into, by choice).
    user_files_disabled: bool,
}

/// Read the hop's known_hosts files (blocking): trusted files are parsed, untrusted ones ignored
/// (logged, and listed for the `knownHostsIgnored` warning). The learn target is the first
/// enabled user file that is missing or trusted (and, on Windows, under the profile).
fn load_known_hosts(hop: &ResolvedHop, profile_root: Option<&Path>) -> KnownHostsSetup {
    load_known_hosts_files(&hop.user_known_hosts, &hop.global_known_hosts, profile_root)
}

fn load_known_hosts_files(
    user: &[KnownHostsOpt],
    global: &[KnownHostsOpt],
    profile_root: Option<&Path>,
) -> KnownHostsSetup {
    let mut files = Vec::new();
    let mut learn_target = None;
    let mut first_user = true;
    let mut ignored = Vec::new();
    let mut user_file_ignored = false;
    let user_files_disabled = matches!(user.first(), Some(KnownHostsOpt::Disabled));
    let sets = [
        (user, Origin::User, FileKind::UserKnownHosts),
        (global, Origin::Global, FileKind::GlobalKnownHosts),
    ];
    for (list, origin, kind) in sets {
        for opt in list.iter() {
            let KnownHostsOpt::Path(path) = opt else {
                continue;
            };
            let shown = path.display().to_string();
            let readable = match read_local(path, kind) {
                Ok(bytes) => {
                    let (parsed, issues) = known_hosts::parse(path, &shown, origin, &bytes);
                    if !issues.is_empty() {
                        tracing::debug!(file = %shown, count = issues.len(), "skipped malformed known_hosts lines");
                    }
                    files.push(parsed);
                    true
                }
                Err(LocalPathError::NotFound) => true,
                Err(e) => {
                    tracing::warn!(file = %shown, reason = %e, "ignoring known_hosts file");
                    ignored.push(shown.clone());
                    user_file_ignored |= origin == Origin::User;
                    false
                }
            };
            if origin == Origin::User && first_user {
                first_user = false;
                let allowed =
                    profile_root.is_none_or(|root| known_hosts::write::is_under(path, root));
                if readable && allowed {
                    learn_target = Some(path.clone());
                }
            }
        }
    }
    KnownHostsSetup {
        files: files.into(),
        learn_target,
        ignored,
        user_file_ignored,
        user_files_disabled,
    }
}

fn hop_ref(index: usize, hop: &ResolvedHop) -> HopRef {
    HopRef {
        index: u32::try_from(index).unwrap_or(u32::MAX),
        label: hop.label.clone(),
        user: hop.user.clone(),
        host: hop.hostname.clone(),
        port: hop.port,
    }
}

/// Verified agent for a hop (§5.5), or `None`. Untrusted endpoints produce a warning event.
async fn open_agent(choice: &AgentChoice, ctx: &ConnectCtx) -> Option<VerifiedAgent> {
    let raw = match choice {
        AgentChoice::Disabled => return None,
        AgentChoice::Endpoint(e) => Some(e.clone()),
        AgentChoice::Default => (ctx.env)("SSH_AUTH_SOCK")
            .filter(|v| !v.is_empty())
            .or_else(agent::fallback_pipe),
    }?;
    let Some(endpoint) = AgentEndpoint::parse(&raw) else {
        ctx.broker.sink().send(SessionEvent::Warning {
            warning: AppError::new(ErrorCode::AgentUntrusted).with_detail("invalid agent endpoint"),
        });
        return None;
    };
    match agent::connect_verified(&endpoint).await {
        Ok(a) => Some(a),
        Err(agent::AgentError::Untrusted(why)) => {
            ctx.broker.sink().send(SessionEvent::Warning {
                warning: AppError::new(ErrorCode::AgentUntrusted).with_detail(why),
            });
            None
        }
        Err(agent::AgentError::Unavailable(why)) => {
            tracing::debug!(reason = %why, "agent unavailable");
            None
        }
    }
}

/// `Prompt.password.canRemember`: a keychain account exists for the hop, the keychain works and
/// the user allows storing passwords (`connection.rememberPasswords`).
pub fn can_remember_password(has_account: bool, keychain: bool, allowed: bool) -> bool {
    has_account && keychain && allowed
}

fn auth_config(hop: &ResolvedHop, ctx: &ConnectCtx) -> HopAuthConfig {
    HopAuthConfig {
        user: hop.user.clone(),
        plan: hop.auth.clone(),
        identity_files: hop.identity_files.clone(),
        user_ssh_dir: ctx.user_ssh_dir.clone(),
        home: ctx.home.clone(),
        identities_only: hop.identities_only,
        add_keys_to_agent: hop.add_keys_to_agent,
        pubkey_enabled: hop.pubkey_enabled,
        password_enabled: hop.password_enabled,
        kbd_interactive_enabled: hop.kbd_interactive_enabled,
        keychain: hop.keychain.clone(),
        binding: hop.binding(),
        can_remember_password: can_remember_password(
            hop.keychain.is_some(),
            ctx.keychain_available,
            ctx.remember_passwords,
        ),
    }
}

enum Transport {
    Tcp(CountingStream),
    Channel(russh::ChannelStream<client::Msg>),
}

/// Open the byte stream for hop `index` (TCP for hop 0, direct-tcpip through the previous hop).
async fn open_transport(
    index: usize,
    hop: &ResolvedHop,
    prev: Option<&HopConnection>,
    traffic: &Arc<Traffic>,
) -> Result<Transport, AppError> {
    match prev {
        None => {
            let tcp = dial(&hop.hostname, hop.port, hop.connect_timeout).await?;
            Ok(Transport::Tcp(CountingStream {
                inner: tcp,
                traffic: traffic.clone(),
            }))
        }
        Some(prev) => {
            // Bounded like a TCP dial: the jump host may wait minutes for an unresponsive target
            // (e.g. during an auto-reconnect attempt), and russh has no timeout of its own here.
            let open = prev.handle.channel_open_direct_tcpip(
                hop.hostname.clone(),
                u32::from(hop.port),
                "127.0.0.1",
                0,
            );
            let ch = tokio::time::timeout(hop.connect_timeout, open)
                .await
                .map_err(|_| {
                    AppError::new(ErrorCode::ConnectTimeout {
                        host: hop.hostname.clone(),
                        port: hop.port,
                        seconds: u32::try_from(hop.connect_timeout.as_secs()).unwrap_or(u32::MAX),
                    })
                    .with_detail(format!("hop {index}: direct-tcpip open timed out"))
                })?
                .map_err(|e| {
                    AppError::new(ErrorCode::JumpChannelFailed {
                        via: prev.info.hop.label.clone(),
                        host: hop.hostname.clone(),
                        port: hop.port,
                    })
                    .with_detail(format!("hop {index}: {e:?}"))
                })?;
            Ok(Transport::Channel(ch.into_stream()))
        }
    }
}

struct Handshake {
    handle: Handle<ClientHandler>,
    shared: Arc<HopShared>,
}

async fn handshake(
    index: usize,
    hop: &ResolvedHop,
    transport: Transport,
    policy: HostKeyPolicy,
    ctx: &ConnectCtx,
) -> Result<Handshake, AppError> {
    let hop_ref = hop_ref(index, hop);
    let known: Vec<String> = known_hosts::known_key_types(&policy.lookup, &policy.files);
    let preferred = russh::Preferred {
        key: Cow::Owned(preferred_host_key_algs(
            hop.host_key_algorithms_add_ssh_rsa,
            &known,
        )),
        ..russh::Preferred::DEFAULT
    };
    let config = Arc::new(client::Config {
        keepalive_interval: Some(hop.keepalive_interval),
        keepalive_max: hop.keepalive_max,
        inactivity_timeout: None,
        nodelay: true,
        preferred,
        ..client::Config::default()
    });
    let shared = Arc::new(HopShared::default());
    let handler = ClientHandler {
        policy,
        broker: ctx.broker.clone(),
        shared: shared.clone(),
    };
    let broker = ctx.broker.clone();
    let fut = async {
        match transport {
            Transport::Tcp(s) => client::connect_stream(config, s, handler).await,
            Transport::Channel(s) => client::connect_stream(config, s, handler).await,
        }
    };
    let seconds = u32::try_from(hop.connect_timeout.as_secs()).unwrap_or(u32::MAX);
    match with_pausable_timeout(fut, hop.connect_timeout, || broker.has_pending()).await {
        None => Err(AppError::new(ErrorCode::ConnectTimeout {
            host: hop.hostname.clone(),
            port: hop.port,
            seconds,
        })),
        Some(Err(e)) => Err(map_russh(&e, &hop_ref, &shared, seconds)),
        Some(Ok(handle)) => Ok(Handshake { handle, shared }),
    }
}

/// Establish every hop, authenticate, then init SFTP. `pins` (per hop) restrict re-dials to the
/// same host keys; `secrets` (per hop) hold the session-cached answers.
pub async fn connect(
    target: &ResolvedTarget,
    ctx: &ConnectCtx,
    secrets: &mut [HopSecrets],
    pins: &mut [Option<HostKeyPin>],
    on_state: &(dyn Fn(SessionState) + Send + Sync),
) -> Result<Connection, AppError> {
    let traffic = Arc::new(Traffic::default());
    let mut hops: Vec<HopConnection> = Vec::with_capacity(target.hops.len());
    let mut dest_auth = None;
    let mut remember = Vec::new();

    for (index, hop) in target.hops.iter().enumerate() {
        let hop_ref = hop_ref(index, hop);
        let profile = ctx.profile_root.clone();
        let hop_owned = hop.clone();
        // A panic is caught on the blocking thread: the `JoinError` text would carry its payload.
        let kh = tokio::task::spawn_blocking(move || {
            crate::error::catch_panic("known_hosts", || {
                load_known_hosts(&hop_owned, profile.as_deref())
            })
        })
        .await
        .map_err(|e| crate::error::join_failed(e.is_panic(), "known_hosts"))??;
        // §5.3: an ignored known_hosts file is reported, once per session (re-dials and
        // reconnects already carry a pin for this hop).
        if pins.get(index).is_none_or(Option::is_none) {
            for file in &kh.ignored {
                ctx.broker.sink().send(SessionEvent::Warning {
                    warning: AppError::new(ErrorCode::KnownHostsIgnored { file: file.clone() }),
                });
            }
        }
        let strict = effective_strict(
            hop.strict,
            kh.user_file_ignored,
            kh.learn_target.is_some() || kh.user_files_disabled,
        );
        let lookup =
            known_hosts::lookup_name(&hop.hostname, hop.port, hop.host_key_alias.as_deref());
        let mut skip_publickey = false;
        let mut redialled = false;
        // §5.4: if the transport dies during authentication, re-dial ONCE with the cached
        // answers (same pinned host key, `SessionSecrets`).
        let mut lost_redialled = false;

        let (handle, shared, auth_info) = loop {
            on_state(SessionState::Connecting {
                hop: hop_ref.clone(),
            });
            let pinned = pins.get(index).cloned().flatten();
            let policy = HostKeyPolicy {
                hop: hop_ref.clone(),
                lookup: lookup.clone(),
                files: kh.files.clone(),
                learn_target: kh.learn_target.clone(),
                profile_root: ctx.profile_root.clone(),
                strict,
                hash_known_hosts: hop.hash_known_hosts,
                pinned,
                approvals: ctx.approvals.clone(),
            };
            let transport = open_transport(index, hop, hops.last(), &traffic).await?;
            let Handshake { mut handle, shared } =
                handshake(index, hop, transport, policy, ctx).await?;
            let verification = match shared.host_key() {
                Some(HostKeyOutcome::Accepted(info)) => info,
                Some(HostKeyOutcome::Rejected(e)) => return Err(e),
                None => return Err(AppError::internal("host key was not checked")),
            };
            if let Some(slot) = pins.get_mut(index) {
                *slot = Some(HostKeyPin {
                    lookup: lookup.clone(),
                    fingerprint: verification.fingerprint.clone(),
                    verification: verification.verification,
                });
            }

            on_state(SessionState::Authenticating {
                hop: hop_ref.clone(),
            });
            let mut agent = open_agent(&hop.identity_agent, ctx).await;
            let cfg = auth_config(hop, ctx);
            let Some(hop_secrets) = secrets.get_mut(index) else {
                return Err(AppError::internal("missing hop secrets"));
            };
            let on_agent_add = agent.as_ref().map(|a| {
                let endpoint = a.endpoint.clone();
                let record = ctx.agent_keys.clone();
                Arc::new(move |key: russh::keys::PublicKey| {
                    record
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push((endpoint.clone(), key));
                }) as Arc<dyn Fn(russh::keys::PublicKey) + Send + Sync>
            });
            let env = AuthEnv {
                hop: hop_ref.clone(),
                verification: verification.verification,
                broker: &ctx.broker,
                agent: agent.as_mut(),
                secrets: hop_secrets,
                skip_publickey,
                keychain: Arc::new(auth::OsKeychain),
                on_agent_add,
            };
            let outcome = {
                let mut transport = RusshTransport {
                    handle: &mut handle,
                    shared: Some(shared.clone()),
                };
                let auth = auth::authenticate(&mut transport, &cfg, env);
                tokio::pin!(auth);
                loop {
                    tokio::select! {
                        outcome = &mut auth => break outcome,
                        () = tokio::time::sleep(AUTH_LOSS_POLL) => {
                            // The hop died while a prompt is open: withdraw it now instead of
                            // waiting for the 110 s prompt timeout (§5.4).
                            if let Some(reason) = shared.loss()
                                && ctx.broker.has_pending()
                            {
                                ctx.broker.cancel_pending(PromptClosedReason::ConnectionLost);
                                break AuthOutcome::Lost(AppError::new(ErrorCode::Disconnected {
                                    reason,
                                    message: shared.disconnect_message(),
                                }));
                            }
                        }
                    }
                }
            };
            match outcome {
                AuthOutcome::Success {
                    info,
                    remember_password,
                } => {
                    if let Some(pw) = remember_password {
                        remember.push((index, pw));
                    }
                    let host_key = verification;
                    break (handle, shared, (info, host_key));
                }
                AuthOutcome::Redial if !redialled => {
                    redialled = true;
                    skip_publickey = true;
                    let _ = handle
                        .disconnect(russh::Disconnect::ByApplication, "", "en")
                        .await;
                    tracing::info!(
                        hop = index,
                        "server allows fewer auth attempts; re-dialling without publickey"
                    );
                }
                AuthOutcome::Redial => return Err(AppError::new(ErrorCode::TooManyAuthFailures)),
                AuthOutcome::Lost(_) if !lost_redialled => {
                    lost_redialled = true;
                    tracing::info!(
                        hop = index,
                        "connection lost during authentication; re-dialling once with cached answers"
                    );
                }
                AuthOutcome::Lost(e) | AuthOutcome::Failed(e) => return Err(e),
            }
        };

        let (session_auth, host_key) = auth_info;
        let pin = pins
            .get(index)
            .cloned()
            .flatten()
            .ok_or_else(|| AppError::internal("missing pin"))?;
        if index + 1 == target.hops.len() {
            dest_auth = Some(session_auth);
        }
        hops.push(HopConnection {
            handle: Arc::new(handle),
            shared,
            pin,
            info: HopInfo {
                hop: hop_ref,
                host_key,
            },
        });
    }

    let auth = dest_auth.ok_or_else(|| AppError::internal("no destination hop"))?;
    let dest = hops
        .last()
        .ok_or_else(|| AppError::internal("no destination hop"))?;
    on_state(SessionState::OpeningChannels {
        hop: dest.info.hop.clone(),
    });
    // The remote uid/gids run in parallel with the SFTP init (separate session channel).
    let (sftp, remote_identity) = tokio::join!(
        tokio::time::timeout(SFTP_INIT_TIMEOUT, init_sftp(&dest.handle)),
        super::identity::probe(&dest.handle),
    );
    let sftp = match sftp {
        Ok(Ok(s)) => Some(s),
        Ok(Err(e)) => {
            tracing::info!(reason = %e, "SFTP unavailable");
            None
        }
        Err(_) => {
            tracing::info!("SFTP init timed out");
            None
        }
    };
    let (sftp, extensions, sftp_limits, home_dir) = match sftp {
        Some((s, ext, limits, home)) => (Some(s), ext, limits, home),
        None => (None, Vec::new(), SftpLimits::default(), None),
    };
    let remote_path_style = remote_path_style(home_dir.as_deref());
    let (start_dir, start_dir_warning) = match (&sftp, &target.start_dir) {
        (Some(s), Some(dir)) => match s.stat(dir.clone()).await {
            Ok(_) => (Some(dir.clone()), None),
            Err(e) => (
                home_dir.clone(),
                Some(
                    AppError::new(ErrorCode::RemoteNotFound { path: dir.clone() })
                        .with_detail(e.to_string()),
                ),
            ),
        },
        _ => (home_dir.clone(), None),
    };

    Ok(Connection {
        hops,
        sftp,
        sftp_extensions: extensions,
        sftp_limits,
        home_dir,
        start_dir,
        remote_path_style,
        remote_identity,
        auth,
        traffic,
        start_dir_warning,
        remember_passwords: remember,
        abandoned: std::sync::atomic::AtomicBool::new(false),
    })
}

/// `homeDir`: `"/…"` and not `/^\/[A-Za-z]:/` ⇒ posix (§4.4).
pub fn remote_path_style(home: Option<&str>) -> RemotePathStyle {
    match home {
        None => RemotePathStyle::Unknown,
        Some(h) => {
            let b = h.as_bytes();
            let drive = b.len() >= 3
                && b.first() == Some(&b'/')
                && b.get(1).is_some_and(u8::is_ascii_alphabetic)
                && b.get(2) == Some(&b':');
            if drive {
                RemotePathStyle::Windows
            } else if h.starts_with('/') {
                RemotePathStyle::Posix
            } else {
                RemotePathStyle::Unknown
            }
        }
    }
}

/// §6.1: session channel → `sftp` subsystem → init → limits → timeout → `realpath(".")`.
async fn init_sftp(
    handle: &Handle<ClientHandler>,
) -> Result<(Arc<RawSftpSession>, Vec<String>, SftpLimits, Option<String>), String> {
    let channel = handle
        .channel_open_session()
        .await
        .map_err(|e| format!("open session: {e:?}"))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| format!("subsystem: {e:?}"))?;
    let mut sftp = RawSftpSession::new(channel.into_stream());
    let version = sftp.init().await.map_err(|e| format!("init: {e:?}"))?;
    let mut extensions: Vec<String> = version.extensions.keys().cloned().collect();
    extensions.sort();
    let mut advertised = SftpLimits::default();
    if version.extensions.contains_key("limits@openssh.com")
        && let Ok(limits) = sftp.limits().await
    {
        advertised = SftpLimits {
            read_len: (limits.max_read_len > 0).then_some(limits.max_read_len),
            write_len: (limits.max_write_len > 0).then_some(limits.max_write_len),
        };
        sftp.set_limits(limits.into());
    }
    sftp.set_timeout(SFTP_REQUEST_TIMEOUT_SECS);
    let home = sftp
        .realpath(".")
        .await
        .ok()
        .and_then(|name| name.files.into_iter().next())
        .map(|f| f.filename);
    Ok((Arc::new(sftp), extensions, advertised, home))
}

/// The verification of every hop that was not confirmed by known_hosts (status chip, §5.3).
pub fn unverified_hops(conn: &Connection) -> usize {
    conn.hops
        .iter()
        .filter(|h| h.info.host_key.verification == HostKeyVerification::AutoAccepted)
        .count()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    /// A user known_hosts file that exists but cannot be used (here: over the 4 MiB cap)
    /// is listed for the `knownHostsIgnored` warning and flags the hop (accept-new/no ask);
    /// a missing file is not "ignored"; `none` counts as disabled.
    #[test]
    fn ignored_known_hosts_files_are_reported() {
        use crate::sshconfig::StrictMode;
        let dir = crate::platform::test_support::private_tempdir();
        let big = dir.path().join("known_hosts");
        std::fs::write(&big, vec![b'#'; 5 * 1024 * 1024]).unwrap();
        let missing = dir.path().join("missing");
        let ok = dir.path().join("ok");
        std::fs::write(&ok, b"example.com ssh-ed25519 AAAA\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for p in [&big, &ok] {
                std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600)).unwrap();
            }
        }

        let kh = load_known_hosts_files(
            &[KnownHostsOpt::Path(big.clone())],
            &[KnownHostsOpt::Path(missing.clone())],
            None,
        );
        assert_eq!(kh.ignored, vec![big.display().to_string()]);
        assert!(kh.user_file_ignored);
        assert!(kh.learn_target.is_none());
        assert_eq!(
            effective_strict(StrictMode::AcceptNew, kh.user_file_ignored, true),
            StrictMode::Ask
        );

        let kh = load_known_hosts_files(
            &[KnownHostsOpt::Path(ok.clone())],
            &[KnownHostsOpt::Path(big.clone())],
            None,
        );
        assert_eq!(kh.ignored, vec![big.display().to_string()]);
        assert!(!kh.user_file_ignored, "only a global file was ignored");
        assert_eq!(kh.learn_target.as_deref(), Some(ok.as_path()));

        let kh = load_known_hosts_files(&[KnownHostsOpt::Disabled], &[], None);
        assert!(kh.user_files_disabled && kh.ignored.is_empty() && kh.learn_target.is_none());
        let kh = load_known_hosts_files(&[KnownHostsOpt::Path(missing)], &[], None);
        assert!(kh.ignored.is_empty() && kh.learn_target.is_some());
    }

    /// `rememberPasswords = false` removes the offer even with a working keychain.
    #[test]
    fn remember_password_offer_follows_the_setting() {
        assert!(can_remember_password(true, true, true));
        assert!(!can_remember_password(true, true, false));
        assert!(!can_remember_password(true, false, true));
        assert!(!can_remember_password(false, true, true));
    }

    #[test]
    fn host_key_algorithms() {
        let algs = preferred_host_key_algs(false, &[]);
        assert!(!algs.contains(&Algorithm::Rsa { hash: None }));
        assert!(algs.iter().any(|a| a.as_str() == "rsa-sha2-512"));
        let with_rsa = preferred_host_key_algs(true, &[]);
        assert_eq!(with_rsa.last(), Some(&Algorithm::Rsa { hash: None }));
        // Types recorded in known_hosts come first (RSA family maps to ssh-rsa).
        let reordered = preferred_host_key_algs(false, &["ssh-rsa".to_owned()]);
        assert!(
            matches!(reordered[0], Algorithm::Rsa { hash: Some(_) }),
            "{reordered:?}"
        );
        let reordered = preferred_host_key_algs(false, &["ecdsa-sha2-nistp256".to_owned()]);
        assert_eq!(reordered[0].as_str(), "ecdsa-sha2-nistp256");
    }

    #[test]
    fn path_style() {
        assert_eq!(
            remote_path_style(Some("/home/alice")),
            RemotePathStyle::Posix
        );
        assert_eq!(
            remote_path_style(Some("/C:/Users/alice")),
            RemotePathStyle::Windows
        );
        assert_eq!(
            remote_path_style(Some("C:\\Users")),
            RemotePathStyle::Unknown
        );
        assert_eq!(remote_path_style(None), RemotePathStyle::Unknown);
    }

    #[tokio::test]
    async fn pausable_timeout() {
        let paused = std::sync::atomic::AtomicBool::new(true);
        // Paused: a 150 ms limit does not fire for a 400 ms future.
        let out = with_pausable_timeout(
            tokio::time::sleep(Duration::from_millis(400)),
            Duration::from_millis(150),
            || paused.load(Ordering::Relaxed),
        )
        .await;
        assert!(out.is_some());
        paused.store(false, Ordering::Relaxed);
        let out = with_pausable_timeout(
            tokio::time::sleep(Duration::from_secs(5)),
            Duration::from_millis(300),
            || paused.load(Ordering::Relaxed),
        )
        .await;
        assert!(out.is_none());
    }

    #[tokio::test]
    async fn dial_errors() {
        // Port 1 on localhost is closed on CI and dev machines.
        let err = dial("127.0.0.1", 1, Duration::from_secs(3))
            .await
            .unwrap_err();
        assert!(
            matches!(
                err.kind,
                ErrorCode::ConnectRefused { .. } | ErrorCode::ConnectTimeout { .. }
            ),
            "{err:?}"
        );
        let err = dial("nonexistent.invalid", 22, Duration::from_secs(3))
            .await
            .unwrap_err();
        assert!(matches!(err.kind, ErrorCode::DnsFailed { .. }), "{err:?}");
    }

    /// One deadline covers the DNS lookup and every address (not `timeout` per address).
    #[tokio::test(start_paused = true)]
    async fn dial_uses_one_deadline_for_lookup_and_all_addresses() {
        let limit = Duration::from_secs(10);
        let addrs: Vec<SocketAddr> = (1..=5)
            .map(|i| SocketAddr::from(([192, 0, 2, i], 22)))
            .collect();
        let tries = std::sync::atomic::AtomicUsize::new(0);
        // Every address hangs: the whole dial ends after `limit`, not 5 × `limit`.
        let start = tokio::time::Instant::now();
        let err = dial_with(
            "example.com",
            22,
            limit,
            async { Ok::<_, io::Error>(addrs.clone()) },
            |_| {
                tries.fetch_add(1, Ordering::Relaxed);
                std::future::pending::<io::Result<()>>()
            },
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err.kind, ErrorCode::ConnectTimeout { seconds: 10, .. }),
            "{err:?}"
        );
        assert_eq!(start.elapsed(), limit);
        assert_eq!(tries.load(Ordering::Relaxed), 1);

        // A slow lookup uses up the budget too.
        let start = tokio::time::Instant::now();
        let err = dial_with(
            "example.com",
            22,
            limit,
            std::future::pending::<io::Result<Vec<SocketAddr>>>(),
            |_| async { Ok(()) },
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err.kind, ErrorCode::ConnectTimeout { .. }),
            "{err:?}"
        );
        assert_eq!(start.elapsed(), limit);

        // Refused addresses are skipped within the deadline; the next one connects.
        let n = std::sync::atomic::AtomicUsize::new(0);
        let ok = dial_with(
            "example.com",
            22,
            limit,
            async { Ok::<_, io::Error>(addrs.clone()) },
            |_| {
                let i = n.fetch_add(1, Ordering::Relaxed);
                async move {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    if i < 2 {
                        Err(io::Error::from(io::ErrorKind::ConnectionRefused))
                    } else {
                        Ok(i)
                    }
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(ok, 2);
    }

    /// `connectTimeout{seconds}` carries the hop's configured timeout, not 0.
    #[tokio::test]
    async fn connect_timeout_reports_configured_seconds() {
        let elapsed = tokio::time::timeout(Duration::ZERO, std::future::pending::<()>())
            .await
            .unwrap_err();
        let hop = HopRef {
            index: 0,
            label: "alice@example.com".into(),
            user: "alice".into(),
            host: "example.com".into(),
            port: 22,
        };
        let shared = HopShared::default();
        for err in [
            russh::Error::ConnectionTimeout,
            russh::Error::Elapsed(elapsed),
        ] {
            let e = map_russh(&err, &hop, &shared, 17);
            assert!(
                matches!(e.kind, ErrorCode::ConnectTimeout { seconds: 17, .. }),
                "{e:?}"
            );
        }
        let io = io::Error::new(io::ErrorKind::TimedOut, "timed out");
        let e = dial_error(&io, "example.com", 22, 9);
        assert!(
            matches!(e.kind, ErrorCode::ConnectTimeout { seconds: 9, .. }),
            "{e:?}"
        );
    }
}
