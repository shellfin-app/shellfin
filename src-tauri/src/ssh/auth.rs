//! Per-hop authentication (§5.6).
//!
//! ```text
//! authenticate_none → Success ⇒ done; Failure{remaining} ⇒ server_methods
//! budget = 6 failed attempts ("none" and partial successes do not count); the attempt that
//!   REACHES the server's MaxAuthTries arrives as a disconnect, not as Failure.
//! reserve = 3 if (Password|KeyboardInteractive ∈ server_methods) && plan ∈ {Auto, Password}
//! pk_limit = budget − reserve
//! disconnect during attempt n:  n == budget ⇒ authFailed
//!                               n <  budget && ≥1 publickey failure && plan Auto && not re-dialled ⇒ Redial
//!                               otherwise ⇒ tooManyAuthFailures
//! order: publickey → keyboard-interactive → password (OpenSSH default); Failure{partial} ⇒
//!   continue with the remaining methods (2FA).
//! ```
//!
//! Publickey candidates (OpenSSH `pubkey_prepare` order): (1) identity files whose public key
//! the agent holds → signed by the agent; (2) other agent identities (not with IdentitiesOnly or
//! plan Key); (3) identity files not in the agent (explicit, else existing defaults), decrypted
//! locally with passphrase prompts. Every RSA key (agent-held or file) uses the server's best
//! RSA hash; without `server-sig-algs` SHA-256 then SHA-1 are tried.
//!
//! Secrets (`HopSecrets`) are only handed in by the caller when the hop's pinned host key
//! matches (§5.4, §5.3); kbd-int answers are never cached.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use russh::keys::agent::Constraint;
use russh::keys::{HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKey};
use zeroize::Zeroizing;

use crate::error::{AppError, AuthMethodKind, DisconnectKind, ErrorCode};
use crate::ipc::dto::{
    AgentKind, AuthMethodUsed, HopRef, HostKeyVerification, KbdInteractiveField, Prompt,
    PromptAnswer, PromptLastError, SessionAuthInfo, SessionEvent,
};
use crate::ssh::agent::{AgentIdentityInfo, VerifiedAgent, list_identities};
use crate::ssh::handler::HopShared;
use crate::ssh::keys::{self, DecryptError, IdentityFile, KeyFile, KeyLoadFailure};
use crate::ssh::prompt::{PromptBroker, PromptOutcome};
use crate::storage::bookmarks::PasswordBinding;

/// OpenSSH default `MaxAuthTries`.
pub const AUTH_BUDGET: u32 = 6;
/// Attempts kept for password / keyboard-interactive when the server offers them.
pub const INTERACTIVE_RESERVE: u32 = 3;
pub const PASSPHRASE_TRIES: u32 = 3;
pub const PASSWORD_TRIES: u32 = 3;
pub const MAX_KBD_ROUNDS: usize = 10;
pub const MAX_KBD_PROMPTS: usize = 10;
/// Cap for server-supplied kbd-int name/instructions/labels (the frontend sanitizes them).
pub const MAX_KBD_TEXT: usize = 256;
/// Lifetime constraint for keys added to the agent (12 h, §5.5).
pub const AGENT_KEY_LIFETIME_SECS: u32 = 43_200;

// ---------------------------------------------------------------------------------------------
// configuration & secrets
// ---------------------------------------------------------------------------------------------

/// Bookmark auth (§5.1 rule 2); ssh-config hosts and quick specs use `Auto`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthPlan {
    /// Config identities + agent + interactive methods.
    Auto,
    /// Only this file (still signed by the agent if the agent holds it).
    Key(PathBuf),
    /// Password / keyboard-interactive only.
    Password,
    /// Agent identities only.
    Agent,
}

/// Keychain account of this hop's password and the binding stored with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeychainRef {
    pub account: String,
    pub binding: KeychainBinding,
}

/// How a saved password is bound to a destination (§5.6, §7.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeychainBinding {
    /// Bookmark account with the binding stored in `bookmarks.json`.
    Stored(PasswordBinding),
    /// `sshconfig:<user>@<host>:<port>`: the account name is the binding.
    ImpliedByAccount,
    /// Bookmark account without a stored binding (never saved, or forgotten while the keychain
    /// delete failed): an entry under it is NEVER used — a missing binding is a mismatch.
    Missing,
}

/// Everything §5.6 needs to know about one hop, built by `ssh::target`.
#[derive(Debug, Clone)]
pub struct HopAuthConfig {
    pub user: String,
    pub plan: AuthPlan,
    /// Explicit identity files (config/bookmark); empty ⇒ existing defaults in `user_ssh_dir`.
    pub identity_files: Vec<IdentityFile>,
    pub user_ssh_dir: PathBuf,
    /// `~` for `~`-abbreviated key paths.
    pub home: PathBuf,
    pub identities_only: bool,
    /// `AddKeysToAgent yes` from a trusted user config file (§5.5).
    pub add_keys_to_agent: bool,
    pub pubkey_enabled: bool,
    pub password_enabled: bool,
    pub kbd_interactive_enabled: bool,
    pub keychain: Option<KeychainRef>,
    /// This hop's `(hostname, port, user)`.
    pub binding: PasswordBinding,
    /// Keychain available and the target can store a password ("Save in keychain" checkbox).
    pub can_remember_password: bool,
}

/// Per-hop session secrets (§5.4): typed password, passphrases and decrypted keys. Replayed only
/// to the same pinned host key (the caller's responsibility); dropped with the session.
#[derive(Default)]
pub struct HopSecrets {
    password: Option<Zeroizing<String>>,
    passphrases: HashMap<PathBuf, Zeroizing<String>>,
    keys: HashMap<PathBuf, Arc<PrivateKey>>,
}

impl std::fmt::Debug for HopSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HopSecrets")
            .field("password", &self.password.as_ref().map(|_| "***"))
            .field("passphrases", &self.passphrases.len())
            .field("keys", &self.keys.len())
            .finish()
    }
}

impl HopSecrets {
    /// Seed the session password (e.g. the draft password of `session_open`, §5.1 rule 4).
    pub fn set_password(&mut self, password: Zeroizing<String>) {
        self.password = Some(password);
    }

    pub fn clear(&mut self) {
        self.password = None;
        self.passphrases.clear();
        self.keys.clear();
    }
}

/// Read access to saved passwords (injectable; production: [`OsKeychain`]). Called from
/// `spawn_blocking`.
pub trait KeychainAccess: Send + Sync + 'static {
    fn get(&self, account: &str) -> Result<Option<Zeroizing<String>>, AppError>;

    /// Whether an entry exists. The default reads it and drops (zeroizes) the secret at
    /// once; keyring 4 offers no attribute-only lookup.
    fn exists(&self, account: &str) -> Result<bool, AppError> {
        self.get(account).map(|p| p.is_some())
    }
}

/// The OS keychain via `crate::secrets`.
#[derive(Debug, Default, Clone, Copy)]
pub struct OsKeychain;

impl KeychainAccess for OsKeychain {
    fn get(&self, account: &str) -> Result<Option<Zeroizing<String>>, AppError> {
        crate::secrets::get_password(account)
    }
}

/// What authentication needs from an agent (implemented by [`VerifiedAgent`]; faked in tests).
pub trait AgentOps: Send {
    fn kind(&self) -> AgentKind;
    fn identities(
        &mut self,
    ) -> impl Future<Output = Result<Vec<AgentIdentityInfo>, AppError>> + Send;
    /// `add_identity` with `KeyLifetime` only (never unconstrained, §5.5).
    fn add_with_lifetime(
        &mut self,
        key: &PrivateKey,
        seconds: u32,
    ) -> impl Future<Output = Result<(), String>> + Send;
}

impl AgentOps for VerifiedAgent {
    fn kind(&self) -> AgentKind {
        VerifiedAgent::kind(self)
    }

    async fn identities(&mut self) -> Result<Vec<AgentIdentityInfo>, AppError> {
        list_identities(self)
            .await
            .map_err(|e| AppError::new(ErrorCode::AgentUnavailable).with_detail(e.to_string()))
    }

    async fn add_with_lifetime(&mut self, key: &PrivateKey, seconds: u32) -> Result<(), String> {
        // Bounded like every non-interactive agent request.
        tokio::time::timeout(
            crate::ssh::agent::AGENT_OP_TIMEOUT,
            self.client
                .add_identity(key, &[Constraint::KeyLifetime { seconds }]),
        )
        .await
        .map_err(|_| "add_identity: timed out".to_owned())?
        .map_err(|e| e.to_string())
    }
}

/// Inputs that change per attempt/session.
pub struct AuthEnv<'a, A: AgentOps = VerifiedAgent> {
    pub hop: HopRef,
    /// Keychain auto-use only when `KnownHosts` (§5.6).
    pub verification: HostKeyVerification,
    /// Prompts, plus warnings via `broker.sink()`.
    pub broker: &'a PromptBroker,
    /// An owner-verified agent, if any.
    pub agent: Option<&'a mut A>,
    pub secrets: &'a mut HopSecrets,
    /// Set by the caller on the single re-dial after [`AuthOutcome::Redial`].
    pub skip_publickey: bool,
    pub keychain: Arc<dyn KeychainAccess>,
    /// Called right after a key was added to the agent, so the SESSION records it and removes it
    /// on every exit path (failure, cancel, re-dial, close).
    pub on_agent_add: Option<Arc<dyn Fn(PublicKey) + Send + Sync>>,
}

/// `Debug` is implemented by hand: `remember_password` must never reach a log.
pub enum AuthOutcome {
    Success {
        info: SessionAuthInfo,
        // Keys added to an agent are reported through `AuthEnv::on_agent_add` the moment they
        // are added (session-owned record), not here.
        /// The user ticked "Save in keychain" for the password that succeeded. The caller stores
        /// it (with the binding) once the hop is connected and, for bookmarks, sets
        /// `passwordSaved` in the store.
        remember_password: Option<Zeroizing<String>>,
    },
    /// The server disconnected before the local budget was used, after a publickey failure:
    /// re-dial once with `skip_publickey = true` (server `MaxAuthTries` < 6).
    Redial,
    /// The transport died (reset, EOF, keepalive timeout, …) — not an auth decision of the
    /// server. §5.4: the caller re-dials once with the cached answers (same pinned host key).
    /// Carries `disconnected{reason}`.
    Lost(AppError),
    Failed(AppError),
}

impl std::fmt::Debug for AuthOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Success {
                info,
                remember_password,
            } => f
                .debug_struct("Success")
                .field("info", info)
                .field(
                    "remember_password",
                    &remember_password.as_ref().map(|_| "***"),
                )
                .finish(),
            Self::Redial => f.write_str("Redial"),
            Self::Lost(e) => f.debug_tuple("Lost").field(e).finish(),
            Self::Failed(e) => f.debug_tuple("Failed").field(e).finish(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// transport
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum TransportError {
    /// The server closed the connection right at an attempt (e.g. `MaxAuthTries` reached).
    Disconnected,
    /// The connection itself was lost: reset, EOF, keepalive timeout, network error.
    Lost(DisconnectKind),
    Other(AppError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptResult {
    Success,
    Failure {
        remaining: Vec<AuthMethodKind>,
        partial_success: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KbdStep {
    Success,
    Failure {
        remaining: Vec<AuthMethodKind>,
        partial_success: bool,
    },
    InfoRequest {
        name: String,
        instructions: String,
        prompts: Vec<(String, bool)>,
    },
}

type TResult<T> = Result<T, TransportError>;

/// The SSH user-auth operations of one connection (real: [`RusshTransport`]).
pub trait AuthTransport: Send {
    type Agent: AgentOps;
    fn auth_none(&mut self, user: &str) -> impl Future<Output = TResult<AttemptResult>> + Send;
    fn auth_password(
        &mut self,
        user: &str,
        password: &str,
    ) -> impl Future<Output = TResult<AttemptResult>> + Send;
    fn auth_publickey(
        &mut self,
        user: &str,
        key: Arc<PrivateKey>,
        hash: Option<HashAlg>,
    ) -> impl Future<Output = TResult<AttemptResult>> + Send;
    fn auth_agent(
        &mut self,
        user: &str,
        key: &PublicKey,
        hash: Option<HashAlg>,
        agent: &mut Self::Agent,
    ) -> impl Future<Output = TResult<AttemptResult>> + Send;
    fn kbd_start(&mut self, user: &str) -> impl Future<Output = TResult<KbdStep>> + Send;
    fn kbd_respond(
        &mut self,
        responses: Vec<String>,
    ) -> impl Future<Output = TResult<KbdStep>> + Send;
    /// `best_supported_rsa_hash()`: `Some(Some(h))` / `Some(None)` (SHA-1 only) / `None` (no
    /// `server-sig-algs`).
    fn best_rsa_hash(&mut self) -> impl Future<Output = TResult<Option<Option<HashAlg>>>> + Send;
}

fn method_kinds(set: &russh::MethodSet) -> Vec<AuthMethodKind> {
    set.iter()
        .filter_map(|m| match m {
            russh::MethodKind::None => Some(AuthMethodKind::None),
            russh::MethodKind::Password => Some(AuthMethodKind::Password),
            russh::MethodKind::PublicKey => Some(AuthMethodKind::Publickey),
            russh::MethodKind::KeyboardInteractive => Some(AuthMethodKind::KeyboardInteractive),
            _ => None,
        })
        .collect()
}

/// Map a russh error during authentication. A closed session is `Disconnected` (refined by
/// [`RusshTransport`] with the hop's recorded loss reason); resets/EOF/keepalive are `Lost`.
pub fn map_russh_auth_error(e: russh::Error) -> TransportError {
    use std::io::ErrorKind;
    match e {
        russh::Error::Disconnect
        | russh::Error::HUP
        | russh::Error::SendError
        | russh::Error::RecvError => TransportError::Disconnected,
        russh::Error::KeepaliveTimeout => TransportError::Lost(DisconnectKind::KeepaliveTimeout),
        russh::Error::IO(io)
            if matches!(
                io.kind(),
                ErrorKind::ConnectionReset
                    | ErrorKind::ConnectionAborted
                    | ErrorKind::BrokenPipe
                    | ErrorKind::UnexpectedEof
            ) =>
        {
            TransportError::Lost(DisconnectKind::ConnectionReset)
        }
        russh::Error::IO(_) => TransportError::Lost(DisconnectKind::NetworkError),
        other => TransportError::Other(
            AppError::new(ErrorCode::ProtocolError).with_detail(other.to_string()),
        ),
    }
}

/// russh reports a closed session during auth as `Failure` with an empty method list.
fn map_auth_result(r: russh::client::AuthResult) -> TResult<AttemptResult> {
    match r {
        russh::client::AuthResult::Success => Ok(AttemptResult::Success),
        russh::client::AuthResult::Failure {
            remaining_methods,
            partial_success,
        } => {
            if remaining_methods.is_empty() && !partial_success {
                Err(TransportError::Disconnected)
            } else {
                Ok(AttemptResult::Failure {
                    remaining: method_kinds(&remaining_methods),
                    partial_success,
                })
            }
        }
    }
}

fn map_kbd(
    r: Result<russh::client::KeyboardInteractiveAuthResponse, russh::Error>,
) -> TResult<KbdStep> {
    use russh::client::KeyboardInteractiveAuthResponse as K;
    match r.map_err(map_russh_auth_error)? {
        K::Success => Ok(KbdStep::Success),
        K::Failure {
            remaining_methods,
            partial_success,
        } => {
            if remaining_methods.is_empty() && !partial_success {
                Err(TransportError::Disconnected)
            } else {
                Ok(KbdStep::Failure {
                    remaining: method_kinds(&remaining_methods),
                    partial_success,
                })
            }
        }
        K::InfoRequest {
            name,
            instructions,
            prompts,
        } => Ok(KbdStep::InfoRequest {
            name,
            instructions,
            prompts: prompts.into_iter().map(|p| (p.prompt, p.echo)).collect(),
        }),
    }
}

/// [`AuthTransport`] over a russh client handle.
pub struct RusshTransport<'h, H: russh::client::Handler> {
    pub handle: &'h mut russh::client::Handle<H>,
    /// The hop's shared state: tells a server decision (`Disconnected`) apart from a lost
    /// connection (`Lost`) when russh only reports "closed".
    pub shared: Option<Arc<HopShared>>,
}

impl<H: russh::client::Handler> RusshTransport<'_, H> {
    async fn refine<T>(&self, r: TResult<T>) -> TResult<T> {
        match r {
            Err(TransportError::Disconnected) => {
                Err(classify_closed(self.shared.as_deref(), LOSS_RECORD_GRACE).await)
            }
            other => other,
        }
    }
}

/// How long to wait for the handler's `disconnected` callback after russh reported "closed".
pub const LOSS_RECORD_GRACE: Duration = Duration::from_millis(300);

/// Classify a request that russh reported as "closed". russh closes its request
/// channels **before** it calls `Handler::disconnected` (which records the reason), so the
/// reason may not be recorded yet: wait up to `grace` for the hop's `closed` notification.
/// A recorded non-`ServerClosed` reason is a lost connection (`Lost`, §5.4 re-dial); a recorded
/// SSH_MSG_DISCONNECT, or nothing recorded within `grace`, stays a server decision
/// (`Disconnected`).
pub async fn classify_closed(shared: Option<&HopShared>, grace: Duration) -> TransportError {
    let Some(shared) = shared else {
        return TransportError::Disconnected;
    };
    let kind = match shared.loss() {
        Some(kind) => Some(kind),
        None => {
            // Register as a waiter BEFORE re-checking, so a notification in between is not lost.
            let notified = shared.closed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            match shared.loss() {
                Some(kind) => Some(kind),
                None => {
                    let _ = tokio::time::timeout(grace, notified).await;
                    shared.loss()
                }
            }
        }
    };
    match kind {
        Some(kind) if kind != DisconnectKind::ServerClosed => TransportError::Lost(kind),
        _ => TransportError::Disconnected,
    }
}

impl<H> AuthTransport for RusshTransport<'_, H>
where
    H: russh::client::Handler + Send + 'static,
{
    type Agent = VerifiedAgent;

    async fn auth_none(&mut self, user: &str) -> TResult<AttemptResult> {
        let r = self
            .handle
            .authenticate_none(user)
            .await
            .map_err(map_russh_auth_error)
            .and_then(map_auth_result);
        self.refine(r).await
    }

    async fn auth_password(&mut self, user: &str, password: &str) -> TResult<AttemptResult> {
        let r = self
            .handle
            .authenticate_password(user, password)
            .await
            .map_err(map_russh_auth_error)
            .and_then(map_auth_result);
        self.refine(r).await
    }

    async fn auth_publickey(
        &mut self,
        user: &str,
        key: Arc<PrivateKey>,
        hash: Option<HashAlg>,
    ) -> TResult<AttemptResult> {
        let r = self
            .handle
            .authenticate_publickey(user, PrivateKeyWithHashAlg::new(key, hash))
            .await
            .map_err(map_russh_auth_error)
            .and_then(map_auth_result);
        self.refine(r).await
    }

    async fn auth_agent(
        &mut self,
        user: &str,
        key: &PublicKey,
        hash: Option<HashAlg>,
        agent: &mut VerifiedAgent,
    ) -> TResult<AttemptResult> {
        let r = match self
            .handle
            .authenticate_publickey_with(user, key.clone(), hash, &mut agent.client)
            .await
        {
            Ok(r) => map_auth_result(r),
            Err(russh::AgentAuthError::Send(_)) => Err(TransportError::Disconnected),
            // §4.3: AgentAuthError → agentUnavailable (with detail).
            Err(e) => Err(TransportError::Other(
                AppError::new(ErrorCode::AgentUnavailable).with_detail(e.to_string()),
            )),
        };
        self.refine(r).await
    }

    async fn kbd_start(&mut self, user: &str) -> TResult<KbdStep> {
        let r = map_kbd(
            self.handle
                .authenticate_keyboard_interactive_start(user, None::<String>)
                .await,
        );
        self.refine(r).await
    }

    async fn kbd_respond(&mut self, responses: Vec<String>) -> TResult<KbdStep> {
        let r = map_kbd(
            self.handle
                .authenticate_keyboard_interactive_respond(responses)
                .await,
        );
        self.refine(r).await
    }

    async fn best_rsa_hash(&mut self) -> TResult<Option<Option<HashAlg>>> {
        let r = self
            .handle
            .best_supported_rsa_hash()
            .await
            .map_err(map_russh_auth_error);
        self.refine(r).await
    }
}

// ---------------------------------------------------------------------------------------------
// algorithm
// ---------------------------------------------------------------------------------------------

/// `^\s*password(\s+for\s+\S+)?\s*:?\s*$` (case-insensitive) and NOT matching
/// `code|otp|token|verification|passcode` ⇒ a plain password prompt (§5.6).
pub fn is_password_prompt(label: &str) -> bool {
    let l = label.to_lowercase();
    if ["code", "otp", "token", "verification", "passcode"]
        .iter()
        .any(|w| l.contains(w))
    {
        return false;
    }
    let Some(rest) = l.trim().strip_prefix("password") else {
        return false;
    };
    let rest = rest.trim_end();
    let rest = rest.strip_suffix(':').unwrap_or(rest).trim_end();
    if rest.is_empty() {
        return true;
    }
    if !rest.starts_with(char::is_whitespace) {
        return false;
    }
    let Some(after_for) = rest.trim_start().strip_prefix("for") else {
        return false;
    };
    if !after_for.starts_with(char::is_whitespace) {
        return false;
    }
    let who = after_for.trim_start();
    !who.is_empty() && !who.contains(char::is_whitespace)
}

fn cap(s: &str) -> String {
    s.chars().take(MAX_KBD_TEXT).collect()
}

/// The transport died during authentication.
fn lost(kind: DisconnectKind) -> AuthOutcome {
    AuthOutcome::Lost(AppError::new(ErrorCode::Disconnected {
        reason: kind,
        message: None,
    }))
}

/// Terminal results travel through `Err` so phases can use `?`.
type Step<T> = Result<T, AuthOutcome>;

enum Verdict {
    Accepted,
    Partial,
    Rejected,
}

enum Candidate {
    /// Signed by the agent: the agent's public key and the label for `keySource`.
    Agent {
        key: PublicKey,
        source: String,
    },
    File(KeyFile),
}

struct Run<'a, 'e, T: AuthTransport> {
    t: &'a mut T,
    cfg: &'a HopAuthConfig,
    env: AuthEnv<'e, T::Agent>,
    failures: u32,
    pk_failures: u32,
    reserve: u32,
    server_methods: Vec<AuthMethodKind>,
    tried: Vec<AuthMethodKind>,
    rsa_hashes: Option<Vec<Option<HashAlg>>>,
    pw_attempts: u32,
    pw_last_error: Option<PromptLastError>,
    keychain_auto_tried: bool,
    /// Cached "a saved password exists for this hop" (one keychain lookup per auth).
    saved_exists: Option<bool>,
    /// Keyboard-interactive exchanges that failed after a later (non-password) round
    /// while the cached password was kept. The second such failure drops the cached
    /// password: PAM often checks the password only after the second factor (password + OTP),
    /// so a stale cached password would otherwise be replayed until the budget runs out.
    kept_password_failures: u32,
    remember_candidate: Option<Zeroizing<String>>,
    /// Key info of a publickey partial success (2FA), reported with the final method.
    partial_key: Option<(Option<String>, Option<String>)>,
    /// Key files whose passphrase answer asked "Add to ssh-agent"; added only once the server
    /// accepted the key.
    agent_add_wanted: std::collections::HashSet<PathBuf>,
}

/// Authenticate one hop (§5.6). Never calls an `auth_*` method after `Success` (App. A #8).
pub async fn authenticate<T: AuthTransport>(
    t: &mut T,
    cfg: &HopAuthConfig,
    env: AuthEnv<'_, T::Agent>,
) -> AuthOutcome {
    let mut run = Run {
        t,
        cfg,
        env,
        failures: 0,
        pk_failures: 0,
        reserve: 0,
        server_methods: Vec::new(),
        tried: Vec::new(),
        rsa_hashes: None,
        pw_attempts: 0,
        pw_last_error: None,
        keychain_auto_tried: false,
        saved_exists: None,
        kept_password_failures: 0,
        remember_candidate: None,
        partial_key: None,
        agent_add_wanted: std::collections::HashSet::new(),
    };
    match run.drive().await {
        Ok(()) => AuthOutcome::Failed(run.auth_failed()),
        Err(outcome) => outcome,
    }
}

impl<T: AuthTransport> Run<'_, '_, T> {
    fn interactive_plan(&self) -> bool {
        matches!(self.cfg.plan, AuthPlan::Auto | AuthPlan::Password)
    }

    fn offers(&self, m: AuthMethodKind) -> bool {
        self.server_methods.contains(&m)
    }

    fn can_attempt(&self) -> bool {
        self.failures < AUTH_BUDGET
    }

    fn pk_limit(&self) -> u32 {
        AUTH_BUDGET - self.reserve
    }

    fn warn(&self, warning: AppError) {
        self.env
            .broker
            .sink()
            .send(SessionEvent::Warning { warning });
    }

    fn auth_failed(&self) -> AppError {
        AppError::new(ErrorCode::AuthFailed {
            user: self.cfg.user.clone(),
            host: self.env.hop.host.clone(),
            tried: self.tried.clone(),
            server_methods: self.server_methods.clone(),
        })
    }

    fn note_tried(&mut self, kind: AuthMethodKind) {
        if !self.tried.contains(&kind) {
            self.tried.push(kind);
        }
    }

    fn success(
        &mut self,
        method: AuthMethodUsed,
        key_type: Option<String>,
        key_source: Option<String>,
    ) -> AuthOutcome {
        let (key_type, key_source) = match (key_type, key_source, self.partial_key.take()) {
            (None, None, Some(partial)) => partial,
            (kt, ks, _) => (kt, ks),
        };
        let remember_password = if matches!(
            method,
            AuthMethodUsed::Password | AuthMethodUsed::KeyboardInteractive
        ) {
            self.remember_candidate.take()
        } else {
            None
        };
        AuthOutcome::Success {
            info: SessionAuthInfo {
                method,
                key_type,
                key_source,
            },
            remember_password,
        }
    }

    /// Server disconnect during attempt `n` (§5.6 boundary).
    fn on_disconnect(&self) -> AuthOutcome {
        let n = self.failures + 1;
        if n >= AUTH_BUDGET {
            AuthOutcome::Failed(self.auth_failed())
        } else if self.pk_failures >= 1
            && self.cfg.plan == AuthPlan::Auto
            && !self.env.skip_publickey
        {
            AuthOutcome::Redial
        } else {
            AuthOutcome::Failed(AppError::new(ErrorCode::TooManyAuthFailures))
        }
    }

    fn after_attempt(
        &mut self,
        kind: AuthMethodKind,
        res: TResult<AttemptResult>,
    ) -> Step<Verdict> {
        self.note_tried(kind);
        match res {
            Ok(AttemptResult::Success) => Ok(Verdict::Accepted),
            Ok(AttemptResult::Failure {
                remaining,
                partial_success: true,
            }) => {
                self.server_methods = remaining;
                Ok(Verdict::Partial)
            }
            Ok(AttemptResult::Failure {
                remaining,
                partial_success: false,
            }) => {
                self.failures += 1;
                if matches!(kind, AuthMethodKind::Publickey | AuthMethodKind::Agent) {
                    self.pk_failures += 1;
                }
                self.server_methods = remaining;
                Ok(Verdict::Rejected)
            }
            Err(TransportError::Disconnected) => Err(self.on_disconnect()),
            Err(TransportError::Lost(kind)) => Err(lost(kind)),
            Err(TransportError::Other(e)) => Err(AuthOutcome::Failed(e)),
        }
    }

    async fn drive(&mut self) -> Step<()> {
        match self.t.auth_none(&self.cfg.user).await {
            Ok(AttemptResult::Success) => {
                // Reported as what it is, not as keyboard-interactive.
                return Err(self.success(AuthMethodUsed::None, None, None));
            }
            Ok(AttemptResult::Failure { remaining, .. }) => self.server_methods = remaining,
            Err(TransportError::Disconnected) => {
                return Err(AuthOutcome::Failed(AppError::new(
                    ErrorCode::Disconnected {
                        reason: DisconnectKind::ServerClosed,
                        message: None,
                    },
                )));
            }
            Err(TransportError::Lost(kind)) => return Err(lost(kind)),
            Err(TransportError::Other(e)) => return Err(AuthOutcome::Failed(e)),
        }
        let interactive_possible = (self.offers(AuthMethodKind::Password)
            && self.cfg.password_enabled)
            || (self.offers(AuthMethodKind::KeyboardInteractive)
                && self.cfg.kbd_interactive_enabled);
        self.reserve = if interactive_possible && self.interactive_plan() {
            INTERACTIVE_RESERVE
        } else {
            0
        };

        let (mut pk_done, mut kbd_done, mut pw_done) = (false, false, false);
        loop {
            if !pk_done && self.publickey_applicable() {
                pk_done = true;
                self.publickey_phase().await?;
                continue;
            }
            if !pk_done && self.cfg.plan == AuthPlan::Agent {
                // Agent-only plan: report a missing/empty agent even if the server hides publickey.
                pk_done = true;
                self.check_agent_plan().await?;
            }
            if !kbd_done
                && self.offers(AuthMethodKind::KeyboardInteractive)
                && self.cfg.kbd_interactive_enabled
                && self.interactive_plan()
            {
                kbd_done = true;
                self.kbd_phase().await?;
                continue;
            }
            if !pw_done
                && self.offers(AuthMethodKind::Password)
                && self.cfg.password_enabled
                && self.interactive_plan()
            {
                pw_done = true;
                self.password_phase().await?;
                continue;
            }
            return Ok(());
        }
    }

    fn publickey_applicable(&self) -> bool {
        self.offers(AuthMethodKind::Publickey)
            && self.cfg.pubkey_enabled
            && !self.env.skip_publickey
            && self.cfg.plan != AuthPlan::Password
    }

    async fn check_agent_plan(&mut self) -> Step<()> {
        let Some(agent) = self.env.agent.as_deref_mut() else {
            return Err(AuthOutcome::Failed(AppError::new(
                ErrorCode::AgentUnavailable,
            )));
        };
        match agent.identities().await {
            Ok(ids) if ids.is_empty() => Err(AuthOutcome::Failed(AppError::new(
                ErrorCode::AgentNoIdentities,
            ))),
            Ok(_) => Ok(()),
            Err(e) => Err(AuthOutcome::Failed(e)),
        }
    }

    // ----- publickey -------------------------------------------------------------------------

    /// Load `files`, or with `defaults_in` the default identities that exist there. Both the
    /// discovery (`is_file`) and the reads run on a blocking thread.
    async fn load_key_files(
        &self,
        files: Vec<IdentityFile>,
        defaults_in: Option<PathBuf>,
    ) -> Vec<(IdentityFile, Result<KeyFile, KeyLoadFailure>)> {
        let home = self.cfg.home.clone();
        tokio::task::spawn_blocking(move || {
            let files = match defaults_in {
                Some(dir) => keys::default_identity_files(&dir),
                None => files,
            };
            files
                .into_iter()
                .map(|f| {
                    let r = keys::read_key_file(&f.path, &home);
                    (f, r)
                })
                .collect()
        })
        .await
        .unwrap_or_default()
    }

    async fn candidates(&mut self) -> Step<Vec<Candidate>> {
        let plan = self.cfg.plan.clone();
        let agent_ids = match self.env.agent.as_deref_mut() {
            Some(agent) => match agent.identities().await {
                Ok(ids) => ids,
                Err(e) if plan == AuthPlan::Agent => return Err(AuthOutcome::Failed(e)),
                Err(e) => {
                    tracing::warn!(error = ?e.kind, "agent identities unavailable; continuing without the agent");
                    Vec::new()
                }
            },
            None if plan == AuthPlan::Agent => {
                return Err(AuthOutcome::Failed(AppError::new(
                    ErrorCode::AgentUnavailable,
                )));
            }
            None => Vec::new(),
        };
        if plan == AuthPlan::Agent && agent_ids.is_empty() {
            return Err(AuthOutcome::Failed(AppError::new(
                ErrorCode::AgentNoIdentities,
            )));
        }
        let (files, defaults_in) = match &plan {
            AuthPlan::Key(p) => (
                vec![IdentityFile {
                    path: p.clone(),
                    explicit: true,
                }],
                None,
            ),
            AuthPlan::Auto if self.cfg.identity_files.is_empty() => {
                (Vec::new(), Some(self.cfg.user_ssh_dir.clone()))
            }
            AuthPlan::Auto => (self.cfg.identity_files.clone(), None),
            AuthPlan::Agent | AuthPlan::Password => (Vec::new(), None),
        };
        let mut group1 = Vec::new();
        let mut group3 = Vec::new();
        let mut matched = vec![false; agent_ids.len()];
        for (file, loaded) in self.load_key_files(files, defaults_in).await {
            let kf = match loaded {
                Ok(kf) => kf,
                Err(failure) => {
                    let shown = keys::abbreviate_home(&file.path, &self.cfg.home);
                    let err = failure.to_app_error(&shown);
                    if matches!(plan, AuthPlan::Key(_)) {
                        return Err(AuthOutcome::Failed(err));
                    }
                    self.warn(err);
                    continue;
                }
            };
            let in_agent = kf.public.as_ref().and_then(|p| {
                agent_ids
                    .iter()
                    .position(|id| id.key.key_data() == p.key_data())
            });
            match in_agent {
                Some(i) => {
                    if let Some(m) = matched.get_mut(i) {
                        *m = true;
                    }
                    if let Some(id) = agent_ids.get(i) {
                        group1.push(Candidate::Agent {
                            key: id.key.clone(),
                            source: kf.display.clone(),
                        });
                    }
                }
                None => group3.push(Candidate::File(kf)),
            }
        }
        let mut out = group1;
        let other_agent_ids =
            plan == AuthPlan::Agent || (!self.cfg.identities_only && plan == AuthPlan::Auto);
        if other_agent_ids {
            for (id, used) in agent_ids.into_iter().zip(matched) {
                if !used {
                    out.push(Candidate::Agent {
                        source: format!("agent: {}", id.comment),
                        key: id.key,
                    });
                }
            }
        }
        out.extend(group3);
        Ok(out)
    }

    async fn hashes_for(&mut self, key: &PublicKey) -> Step<Vec<Option<HashAlg>>> {
        if !key.algorithm().is_rsa() {
            return Ok(vec![None]);
        }
        if let Some(h) = &self.rsa_hashes {
            return Ok(h.clone());
        }
        let hashes = match self.t.best_rsa_hash().await {
            Ok(Some(Some(h))) => vec![Some(h)],
            Ok(Some(None)) => vec![None],
            Ok(None) => vec![Some(HashAlg::Sha256), None],
            Err(TransportError::Disconnected) => return Err(self.on_disconnect()),
            Err(TransportError::Lost(kind)) => return Err(lost(kind)),
            Err(TransportError::Other(e)) => return Err(AuthOutcome::Failed(e)),
        };
        self.rsa_hashes = Some(hashes.clone());
        Ok(hashes)
    }

    fn pk_exhausted(&self) -> bool {
        self.pk_failures >= self.pk_limit() || !self.can_attempt()
    }

    async fn publickey_phase(&mut self) -> Step<()> {
        let candidates = self.candidates().await?;
        for candidate in candidates {
            if self.pk_exhausted() {
                break;
            }
            match candidate {
                Candidate::Agent { key, source } => {
                    for hash in self.hashes_for(&key).await? {
                        if self.pk_exhausted() {
                            break;
                        }
                        let Some(agent) = self.env.agent.as_deref_mut() else {
                            break;
                        };
                        let res = self.t.auth_agent(&self.cfg.user, &key, hash, agent).await;
                        match self.after_attempt(AuthMethodKind::Agent, res)? {
                            Verdict::Accepted => {
                                return Err(self.success(
                                    AuthMethodUsed::Agent,
                                    Some(key.algorithm().to_string()),
                                    Some(source),
                                ));
                            }
                            Verdict::Partial => {
                                self.partial_key =
                                    Some((Some(key.algorithm().to_string()), Some(source)));
                                return Ok(());
                            }
                            Verdict::Rejected => {}
                        }
                    }
                }
                Candidate::File(kf) => {
                    let Some(key) = self.obtain_key(&kf).await? else {
                        continue;
                    };
                    let public = key.public_key().clone();
                    for hash in self.hashes_for(&public).await? {
                        if self.pk_exhausted() {
                            break;
                        }
                        let res = self
                            .t
                            .auth_publickey(&self.cfg.user, key.clone(), hash)
                            .await;
                        match self.after_attempt(AuthMethodKind::Publickey, res)? {
                            Verdict::Accepted => {
                                self.add_accepted_key_to_agent(&kf, &key).await;
                                return Err(self.success(
                                    AuthMethodUsed::Publickey,
                                    Some(public.algorithm().to_string()),
                                    Some(kf.display.clone()),
                                ));
                            }
                            Verdict::Partial => {
                                self.add_accepted_key_to_agent(&kf, &key).await;
                                self.partial_key = Some((
                                    Some(public.algorithm().to_string()),
                                    Some(kf.display.clone()),
                                ));
                                return Ok(());
                            }
                            Verdict::Rejected => {}
                        }
                    }
                }
            }
        }
        Ok(())
    }

    async fn decrypt_blocking(
        kf: &KeyFile,
        passphrase: Option<Zeroizing<String>>,
    ) -> Result<PrivateKey, DecryptError> {
        let kf = kf.clone();
        tokio::task::spawn_blocking(move || {
            keys::decrypt(&kf, passphrase.as_deref().map(String::as_str))
        })
        .await
        .unwrap_or_else(|e| {
            Err(DecryptError::Failed(KeyLoadFailure {
                reason: crate::error::KeyLoadReason::Corrupt,
                // Never the `JoinError` text: it carries the panic payload.
                detail: if e.is_panic() {
                    "decrypt task panicked".to_owned()
                } else {
                    "decrypt task cancelled".to_owned()
                },
            }))
        })
    }

    fn key_unusable(
        &self,
        kf: &KeyFile,
        failure: &KeyLoadFailure,
    ) -> Step<Option<Arc<PrivateKey>>> {
        let err = failure.to_app_error(&kf.display);
        if matches!(self.cfg.plan, AuthPlan::Key(_)) {
            return Err(AuthOutcome::Failed(err));
        }
        self.warn(err);
        Ok(None)
    }

    /// Decrypted key for a file candidate: cached → unencrypted → cached passphrase → prompt.
    /// `None` = skip this candidate.
    async fn obtain_key(&mut self, kf: &KeyFile) -> Step<Option<Arc<PrivateKey>>> {
        if let Some(k) = self.env.secrets.keys.get(&kf.path) {
            return Ok(Some(k.clone()));
        }
        if !kf.encrypted {
            return match Self::decrypt_blocking(kf, None).await {
                Ok(k) => {
                    let k = Arc::new(k);
                    self.env.secrets.keys.insert(kf.path.clone(), k.clone());
                    Ok(Some(k))
                }
                Err(DecryptError::Failed(f)) => self.key_unusable(kf, &f),
                Err(_) => Ok(None),
            };
        }
        if let Some(failure) = kf.kdf_refusal() {
            // Never prompt for a key whose KDF parameters are refused anyway.
            return self.key_unusable(kf, &failure);
        }
        if let Some(pp) = self.env.secrets.passphrases.get(&kf.path).cloned() {
            match Self::decrypt_blocking(kf, Some(pp)).await {
                Ok(k) => {
                    let k = Arc::new(k);
                    self.env.secrets.keys.insert(kf.path.clone(), k.clone());
                    return Ok(Some(k));
                }
                Err(_) => {
                    self.env.secrets.passphrases.remove(&kf.path);
                }
            }
        }
        let agent_kind = self.env.agent.as_deref().map(AgentOps::kind);
        let agent_available = agent_kind.is_some();
        let add_supported = agent_kind.is_some_and(|k| k != AgentKind::OpensshPipe);
        let add_default = add_supported && self.cfg.add_keys_to_agent;
        let mut attempts_left = PASSPHRASE_TRIES;
        let mut last_error = None;
        loop {
            let hop = self.env.hop.clone();
            let key_path = kf.display.clone();
            let outcome = self
                .env
                .broker
                .ask(|id| Prompt::Passphrase {
                    id,
                    hop,
                    key_path,
                    attempts_left,
                    last_error,
                    agent_available,
                    add_to_agent_supported: add_supported,
                    add_to_agent_default: add_default,
                })
                .await;
            let (passphrase, add_to_agent) = match outcome {
                PromptOutcome::Answer(PromptAnswer::Passphrase {
                    passphrase,
                    add_to_agent,
                }) => (Zeroizing::new(passphrase.expose().clone()), add_to_agent),
                PromptOutcome::Answer(PromptAnswer::Cancel) | PromptOutcome::Withdrawn => {
                    return Err(AuthOutcome::Failed(AppError::new(ErrorCode::Cancelled)));
                }
                PromptOutcome::Timeout => {
                    return Err(AuthOutcome::Failed(AppError::new(ErrorCode::PromptTimeout)));
                }
                PromptOutcome::Answer(_) => return Ok(None), // skip (or an unrelated answer)
            };
            match Self::decrypt_blocking(kf, Some(passphrase.clone())).await {
                Ok(k) => {
                    if add_to_agent && add_supported {
                        // Deferred until the server accepts the key.
                        self.agent_add_wanted.insert(kf.path.clone());
                    }
                    let k = Arc::new(k);
                    self.env
                        .secrets
                        .passphrases
                        .insert(kf.path.clone(), passphrase);
                    self.env.secrets.keys.insert(kf.path.clone(), k.clone());
                    return Ok(Some(k));
                }
                Err(DecryptError::WrongPassphrase | DecryptError::NeedsPassphrase) => {
                    attempts_left -= 1;
                    last_error = Some(PromptLastError::PassphraseIncorrect);
                    if attempts_left == 0 {
                        self.warn(AppError::new(ErrorCode::PassphraseIncorrect {
                            path: kf.display.clone(),
                        }));
                        return Ok(None);
                    }
                }
                Err(DecryptError::Failed(f)) => return self.key_unusable(kf, &f),
            }
        }
    }

    /// After the server accepted `key`: add it to the agent if the user asked for it.
    async fn add_accepted_key_to_agent(&mut self, kf: &KeyFile, key: &PrivateKey) {
        if self.agent_add_wanted.remove(&kf.path) {
            self.add_to_agent(key).await;
        }
    }

    /// `add_identity` with the 12 h lifetime only; a rejection is a warning, never retried
    /// unconstrained (§5.5). A key the agent already holds is not added again — removing "our"
    /// copy on close would delete the user's own entry.
    async fn add_to_agent(&mut self, key: &PrivateKey) {
        let Some(agent) = self.env.agent.as_deref_mut() else {
            return;
        };
        let public = key.public_key().clone();
        if let Ok(ids) = agent.identities().await
            && ids.iter().any(|id| id.key.key_data() == public.key_data())
        {
            return;
        }
        match agent.add_with_lifetime(key, AGENT_KEY_LIFETIME_SECS).await {
            Ok(()) => {
                if let Some(record) = &self.env.on_agent_add {
                    record(public);
                }
            }
            Err(detail) => {
                self.warn(AppError::new(ErrorCode::AgentConstraintUnsupported).with_detail(detail));
            }
        }
    }

    // ----- passwords -------------------------------------------------------------------------

    /// Only a stored binding equal to this hop, or an account that implies its binding,
    /// matches; a bookmark account without a binding never does.
    fn binding_matches(&self, kc: &KeychainRef) -> bool {
        match &kc.binding {
            KeychainBinding::Stored(b) => *b == self.cfg.binding,
            KeychainBinding::ImpliedByAccount => true,
            KeychainBinding::Missing => false,
        }
    }

    /// Whether "Use saved password" can be offered: the binding matches and an entry
    /// exists. Looked up once per authentication and cached, and the secret is dropped (zeroized)
    /// right after the check, instead of being read on every password prompt. keyring 4 has no
    /// attribute-only lookup, so the one check still reads the item.
    async fn saved_password_exists(&mut self) -> bool {
        let Some(kc) = self.cfg.keychain.as_ref() else {
            return false;
        };
        if !self.binding_matches(kc) {
            return false;
        }
        if let Some(known) = self.saved_exists {
            return known;
        }
        let keychain = self.env.keychain.clone();
        let account = kc.account.clone();
        let exists = match tokio::task::spawn_blocking(move || keychain.exists(&account)).await {
            Ok(Ok(found)) => found,
            Ok(Err(e)) => {
                tracing::warn!(error = ?e.kind, "keychain lookup failed");
                false
            }
            Err(_) => false,
        };
        self.saved_exists = Some(exists);
        exists
    }

    /// The saved password if an entry exists and its binding matches this hop.
    async fn saved_password(&self) -> Option<Zeroizing<String>> {
        let kc = self.cfg.keychain.as_ref()?;
        if !self.binding_matches(kc) {
            return None;
        }
        let keychain = self.env.keychain.clone();
        let account = kc.account.clone();
        match tokio::task::spawn_blocking(move || keychain.get(&account)).await {
            Ok(Ok(found)) => found,
            Ok(Err(e)) => {
                tracing::warn!(error = ?e.kind, "keychain read failed");
                None
            }
            Err(_) => None,
        }
    }

    /// Next password to try: session cache → keychain auto-use (binding + `knownHosts`) →
    /// prompt. `None` = skipped. A try is counted when the server rejects the password
    /// ([`Self::password_rejected`]), so replaying the cached password inside a 2FA retry is free.
    async fn obtain_password(
        &mut self,
        server_label: Option<String>,
    ) -> Step<Option<Zeroizing<String>>> {
        if let Some(p) = self.env.secrets.password.clone() {
            self.remember_candidate = None;
            return Ok(Some(p));
        }
        if !self.keychain_auto_tried && self.env.verification == HostKeyVerification::KnownHosts {
            // Auto-use is the only path that needs the secret before the user asks for it.
            self.keychain_auto_tried = true;
            let saved = self.saved_password().await;
            self.saved_exists = Some(saved.is_some());
            if let Some(p) = saved {
                self.remember_candidate = None;
                return Ok(Some(p));
            }
        }
        let mut saved_available = self.saved_password_exists().await;
        let can_remember = self.cfg.can_remember_password && self.cfg.keychain.is_some();
        loop {
            let hop = self.env.hop.clone();
            let user = self.cfg.user.clone();
            let host = self.env.hop.host.clone();
            let attempts_left = PASSWORD_TRIES.saturating_sub(self.pw_attempts);
            let last_error = self.pw_last_error;
            let label = server_label.clone();
            let outcome = self
                .env
                .broker
                .ask(|id| Prompt::Password {
                    id,
                    hop,
                    user,
                    host,
                    attempts_left,
                    last_error,
                    can_remember,
                    saved_password_available: saved_available,
                    server_label: label,
                })
                .await;
            match outcome {
                PromptOutcome::Answer(PromptAnswer::Password { password, remember }) => {
                    let p = Zeroizing::new(password.expose().clone());
                    self.env.secrets.password = Some(p.clone());
                    self.remember_candidate = (remember && can_remember).then(|| p.clone());
                    return Ok(Some(p));
                }
                PromptOutcome::Answer(PromptAnswer::UseSavedPassword) => {
                    // The frontend flag is advisory: re-check entry and binding (§5.6).
                    if let Some(p) = self.saved_password().await {
                        self.env.secrets.password = Some(p.clone());
                        self.remember_candidate = None;
                        return Ok(Some(p));
                    }
                    saved_available = false;
                    self.saved_exists = Some(false);
                }
                PromptOutcome::Answer(PromptAnswer::Cancel) | PromptOutcome::Withdrawn => {
                    return Err(AuthOutcome::Failed(AppError::new(ErrorCode::Cancelled)));
                }
                PromptOutcome::Timeout => {
                    return Err(AuthOutcome::Failed(AppError::new(ErrorCode::PromptTimeout)));
                }
                PromptOutcome::Answer(_) => return Ok(None),
            }
        }
    }

    fn password_rejected(&mut self) {
        self.pw_attempts += 1;
        self.env.secrets.password = None;
        self.pw_last_error = Some(PromptLastError::PasswordIncorrect);
    }

    async fn password_phase(&mut self) -> Step<()> {
        while self.can_attempt() && self.pw_attempts < PASSWORD_TRIES {
            let Some(pw) = self.obtain_password(None).await? else {
                return Ok(());
            };
            let res = self.t.auth_password(&self.cfg.user, &pw).await;
            match self.after_attempt(AuthMethodKind::Password, res)? {
                Verdict::Accepted => {
                    return Err(self.success(AuthMethodUsed::Password, None, None));
                }
                Verdict::Partial => return Ok(()),
                Verdict::Rejected => self.password_rejected(),
            }
        }
        Ok(())
    }

    // ----- keyboard-interactive --------------------------------------------------------------

    async fn kbd_phase(&mut self) -> Step<()> {
        loop {
            if !self.can_attempt() {
                return Ok(());
            }
            let mut step = self.t.kbd_start(&self.cfg.user).await;
            let mut rounds = 0usize;
            // Some round of this exchange was a password round / the LAST answered round was.
            let mut password_like = false;
            let mut last_round_password = false;
            let mut skipped = false;
            loop {
                match step {
                    Err(TransportError::Disconnected) => {
                        self.note_tried(AuthMethodKind::KeyboardInteractive);
                        return Err(self.on_disconnect());
                    }
                    Err(TransportError::Lost(kind)) => return Err(lost(kind)),
                    Err(TransportError::Other(e)) => return Err(AuthOutcome::Failed(e)),
                    Ok(KbdStep::Success) => {
                        self.note_tried(AuthMethodKind::KeyboardInteractive);
                        return Err(self.success(AuthMethodUsed::KeyboardInteractive, None, None));
                    }
                    Ok(KbdStep::Failure {
                        remaining,
                        partial_success,
                    }) => {
                        let verdict = self.after_attempt(
                            AuthMethodKind::KeyboardInteractive,
                            Ok(AttemptResult::Failure {
                                remaining,
                                partial_success,
                            }),
                        )?;
                        if matches!(verdict, Verdict::Partial) {
                            return Ok(());
                        }
                        // Only a failure right after the password round rejects the
                        // password. A wrong second factor (e.g. "Verification code:") keeps the
                        // cached password and re-prompts the code in a new exchange (bounded by
                        // the auth budget).
                        if last_round_password {
                            self.password_rejected();
                        } else if password_like {
                            // One retry with the kept password, then ask for it again.
                            self.kept_password_failures += 1;
                            if self.kept_password_failures >= 2 {
                                self.kept_password_failures = 0;
                                self.env.secrets.password = None;
                            }
                        }
                        let retry = password_like
                            && !skipped
                            && self.offers(AuthMethodKind::KeyboardInteractive)
                            && (!last_round_password || self.pw_attempts < PASSWORD_TRIES);
                        if retry {
                            break; // start a new exchange
                        }
                        return Ok(());
                    }
                    Ok(KbdStep::InfoRequest {
                        name,
                        instructions,
                        prompts,
                    }) => {
                        rounds += 1;
                        if rounds > MAX_KBD_ROUNDS || prompts.len() > MAX_KBD_PROMPTS {
                            self.note_tried(AuthMethodKind::KeyboardInteractive);
                            return Err(AuthOutcome::Failed(self.auth_failed()));
                        }
                        let was_password = password_like;
                        password_like = false;
                        let responses = self
                            .kbd_answers(
                                &name,
                                &instructions,
                                &prompts,
                                &mut password_like,
                                &mut skipped,
                            )
                            .await?;
                        // An empty-prompt round (PAM's final InfoRequest) changes nothing.
                        if !prompts.is_empty() {
                            last_round_password = password_like;
                            // A password the user typed into a keyboard-interactive
                            // "Password:" prompt counts as a tried password, so the failure
                            // panel does not show only "Keyboard-interactive".
                            if password_like {
                                self.note_tried(AuthMethodKind::Password);
                            }
                        }
                        password_like |= was_password;
                        step = self.t.kbd_respond(responses).await;
                    }
                }
            }
        }
    }

    async fn kbd_answers(
        &mut self,
        name: &str,
        instructions: &str,
        prompts: &[(String, bool)],
        password_like: &mut bool,
        skipped: &mut bool,
    ) -> Step<Vec<String>> {
        let empties = || vec![String::new(); prompts.len()];
        if prompts.is_empty() {
            return Ok(Vec::new());
        }
        if let [(label, false)] = prompts
            && is_password_prompt(label)
        {
            if self.pw_attempts >= PASSWORD_TRIES {
                *skipped = true;
                return Ok(empties());
            }
            return match self.obtain_password(Some(cap(label))).await? {
                Some(pw) => {
                    *password_like = true;
                    Ok(vec![pw.to_string()])
                }
                None => {
                    *skipped = true;
                    Ok(empties())
                }
            };
        }
        let hop = self.env.hop.clone();
        let fields = prompts
            .iter()
            .map(|(label, echo)| KbdInteractiveField {
                label: cap(label),
                echo: *echo,
            })
            .collect();
        let (name, instructions) = (cap(name), cap(instructions));
        let outcome = self
            .env
            .broker
            .ask(|id| Prompt::KeyboardInteractive {
                id,
                hop,
                name,
                instructions,
                fields,
            })
            .await;
        match outcome {
            PromptOutcome::Answer(PromptAnswer::KeyboardInteractive { responses }) => {
                let mut out: Vec<String> = responses.iter().map(|r| r.expose().clone()).collect();
                out.resize(prompts.len(), String::new());
                Ok(out)
            }
            PromptOutcome::Answer(PromptAnswer::Cancel) | PromptOutcome::Withdrawn => {
                Err(AuthOutcome::Failed(AppError::new(ErrorCode::Cancelled)))
            }
            PromptOutcome::Timeout => {
                Err(AuthOutcome::Failed(AppError::new(ErrorCode::PromptTimeout)))
            }
            PromptOutcome::Answer(_) => {
                *skipped = true;
                Ok(empties())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! §10.1 `ssh::auth`: scripted fake transport + scripted prompt answers.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use std::collections::VecDeque;
    use std::path::Path;
    use std::sync::{Mutex, OnceLock, Weak};
    use std::time::Duration;

    use russh::keys::ssh_key::public::{Ed25519PublicKey, KeyData};

    use super::*;
    use crate::ipc::secret::Secret;
    use crate::platform::test_support::private_tempdir;
    use crate::ssh::keys::fixtures::*;
    use crate::ssh::prompt::EventSink;
    use crate::ssh::prompt::test_support::prompt_id;

    const PK: AuthMethodKind = AuthMethodKind::Publickey;
    const PW: AuthMethodKind = AuthMethodKind::Password;
    const KI: AuthMethodKind = AuthMethodKind::KeyboardInteractive;

    fn fp(k: &PublicKey) -> String {
        k.fingerprint(HashAlg::Sha256).to_string()
    }

    // ----- fake transport --------------------------------------------------------------------

    #[derive(Debug, Clone, PartialEq)]
    enum Call {
        None,
        Password(String),
        PublicKey(String, Option<HashAlg>),
        Agent(String, Option<HashAlg>),
        KbdStart,
        KbdRespond(Vec<String>),
        BestRsa,
    }

    enum Reply {
        A(TResult<AttemptResult>),
        K(TResult<KbdStep>),
        R(TResult<Option<Option<HashAlg>>>),
    }

    type Handler = Box<dyn FnMut(&Call, &[Call]) -> Reply + Send>;

    struct Fake {
        calls: Vec<Call>,
        handler: Handler,
    }

    impl Fake {
        fn new(handler: impl FnMut(&Call, &[Call]) -> Reply + Send + 'static) -> Self {
            Self {
                calls: Vec::new(),
                handler: Box::new(handler),
            }
        }

        fn reply(&mut self, call: Call) -> Reply {
            let r = (self.handler)(&call, &self.calls);
            self.calls.push(call);
            r
        }

        fn attempt(&mut self, call: Call) -> TResult<AttemptResult> {
            match self.reply(call) {
                Reply::A(r) => r,
                _ => panic!("wrong reply kind"),
            }
        }

        fn kbd(&mut self, call: Call) -> TResult<KbdStep> {
            match self.reply(call) {
                Reply::K(r) => r,
                _ => panic!("wrong reply kind"),
            }
        }
    }

    impl AuthTransport for Fake {
        type Agent = FakeAgent;

        async fn auth_none(&mut self, _user: &str) -> TResult<AttemptResult> {
            self.attempt(Call::None)
        }
        async fn auth_password(&mut self, _user: &str, password: &str) -> TResult<AttemptResult> {
            self.attempt(Call::Password(password.to_owned()))
        }
        async fn auth_publickey(
            &mut self,
            _user: &str,
            key: Arc<PrivateKey>,
            hash: Option<HashAlg>,
        ) -> TResult<AttemptResult> {
            self.attempt(Call::PublicKey(fp(key.public_key()), hash))
        }
        async fn auth_agent(
            &mut self,
            _user: &str,
            key: &PublicKey,
            hash: Option<HashAlg>,
            agent: &mut FakeAgent,
        ) -> TResult<AttemptResult> {
            assert!(
                agent
                    .ids
                    .iter()
                    .any(|id| id.key.key_data() == key.key_data()),
                "agent signs only its own keys"
            );
            self.attempt(Call::Agent(fp(key), hash))
        }
        async fn kbd_start(&mut self, _user: &str) -> TResult<KbdStep> {
            self.kbd(Call::KbdStart)
        }
        async fn kbd_respond(&mut self, responses: Vec<String>) -> TResult<KbdStep> {
            self.kbd(Call::KbdRespond(responses))
        }
        async fn best_rsa_hash(&mut self) -> TResult<Option<Option<HashAlg>>> {
            match self.reply(Call::BestRsa) {
                Reply::R(r) => r,
                _ => panic!("wrong reply kind"),
            }
        }
    }

    fn fail(methods: &[AuthMethodKind]) -> TResult<AttemptResult> {
        Ok(AttemptResult::Failure {
            remaining: methods.to_vec(),
            partial_success: false,
        })
    }

    /// A server that offers `methods`, rejects every attempt and supports rsa-sha2-512.
    fn rejecting(methods: &'static [AuthMethodKind]) -> impl FnMut(&Call, &[Call]) -> Reply + Send {
        move |c, _| match c {
            Call::BestRsa => Reply::R(Ok(Some(Some(HashAlg::Sha512)))),
            Call::KbdStart | Call::KbdRespond(_) => Reply::K(Ok(KbdStep::Failure {
                remaining: methods.to_vec(),
                partial_success: false,
            })),
            _ => Reply::A(fail(methods)),
        }
    }

    fn count(calls: &[Call], pred: impl Fn(&Call) -> bool) -> usize {
        calls.iter().filter(|c| pred(c)).count()
    }

    // ----- fake agent / keychain -------------------------------------------------------------

    struct FakeAgent {
        kind: AgentKind,
        ids: Vec<AgentIdentityInfo>,
        /// If set, every listing after the first returns these (the agent changed meanwhile).
        ids_later: Option<Vec<AgentIdentityInfo>>,
        listings: usize,
        add_ok: bool,
        adds: Vec<(String, u32)>,
    }

    impl FakeAgent {
        fn new(kind: AgentKind, ids: Vec<AgentIdentityInfo>) -> Self {
            Self {
                kind,
                ids,
                ids_later: None,
                listings: 0,
                add_ok: true,
                adds: Vec::new(),
            }
        }
    }

    impl AgentOps for FakeAgent {
        fn kind(&self) -> AgentKind {
            self.kind
        }
        async fn identities(&mut self) -> Result<Vec<AgentIdentityInfo>, AppError> {
            self.listings += 1;
            match &self.ids_later {
                Some(later) if self.listings > 1 => Ok(later.clone()),
                _ => Ok(self.ids.clone()),
            }
        }
        async fn add_with_lifetime(
            &mut self,
            key: &PrivateKey,
            seconds: u32,
        ) -> Result<(), String> {
            self.adds.push((fp(key.public_key()), seconds));
            if self.add_ok {
                Ok(())
            } else {
                Err("agent refused the constraint".into())
            }
        }
    }

    fn agent_id(key: PublicKey, comment: &str) -> AgentIdentityInfo {
        AgentIdentityInfo {
            key,
            comment: comment.into(),
        }
    }

    fn extra_key(n: u8) -> PublicKey {
        PublicKey::new(KeyData::Ed25519(Ed25519PublicKey([n; 32])), "extra")
    }

    #[derive(Default)]
    struct FakeKeychain {
        entries: Mutex<HashMap<String, String>>,
        /// Number of keychain reads.
        reads: std::sync::atomic::AtomicUsize,
    }

    impl KeychainAccess for FakeKeychain {
        fn get(&self, account: &str) -> Result<Option<Zeroizing<String>>, AppError> {
            self.reads
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(self
                .entries
                .lock()
                .unwrap()
                .get(account)
                .cloned()
                .map(Zeroizing::new))
        }
    }

    // ----- scripted prompts ------------------------------------------------------------------

    type Answerer = Box<dyn FnOnce(&Prompt) -> PromptAnswer + Send>;

    #[derive(Default)]
    struct Scripted {
        broker: OnceLock<Weak<PromptBroker>>,
        answers: Mutex<VecDeque<Answerer>>,
        prompts: Mutex<Vec<Prompt>>,
        warnings: Mutex<Vec<AppError>>,
    }

    impl EventSink for Scripted {
        fn send(&self, event: SessionEvent) {
            match event {
                SessionEvent::Prompt { prompt } => {
                    let answer = self
                        .answers
                        .lock()
                        .unwrap()
                        .pop_front()
                        .map_or(PromptAnswer::Cancel, |f| f(&prompt));
                    let id = prompt_id(&prompt).to_owned();
                    self.prompts.lock().unwrap().push(prompt);
                    if let Some(b) = self.broker.get().and_then(Weak::upgrade) {
                        b.answer(&id, answer).unwrap();
                    }
                }
                SessionEvent::Warning { warning } => self.warnings.lock().unwrap().push(warning),
                _ => {}
            }
        }
    }

    fn pw(p: &str, remember: bool) -> PromptAnswer {
        PromptAnswer::Password {
            password: Secret::new(p.to_owned()),
            remember,
        }
    }

    fn passphrase(p: &str, add_to_agent: bool) -> PromptAnswer {
        PromptAnswer::Passphrase {
            passphrase: Secret::new(p.to_owned()),
            add_to_agent,
        }
    }

    struct H {
        sink: Arc<Scripted>,
        broker: Arc<PromptBroker>,
        secrets: HopSecrets,
        keychain: Arc<FakeKeychain>,
        verification: HostKeyVerification,
        skip_pk: bool,
        /// Keys reported through `on_agent_add` (session-owned record).
        added: Arc<Mutex<Vec<PublicKey>>>,
        agent_adds: Arc<dyn Fn(PublicKey) + Send + Sync>,
    }

    impl H {
        fn new() -> Self {
            let sink = Arc::new(Scripted::default());
            let broker = Arc::new(PromptBroker::new(sink.clone(), Duration::from_secs(5)));
            sink.broker.set(Arc::downgrade(&broker)).ok().unwrap();
            let added: Arc<Mutex<Vec<PublicKey>>> = Arc::default();
            let rec = added.clone();
            Self {
                sink,
                broker,
                secrets: HopSecrets::default(),
                keychain: Arc::new(FakeKeychain::default()),
                verification: HostKeyVerification::KnownHosts,
                skip_pk: false,
                added,
                agent_adds: Arc::new(move |k| rec.lock().unwrap().push(k)),
            }
        }

        fn answer(&self, f: impl FnOnce(&Prompt) -> PromptAnswer + Send + 'static) {
            self.sink.answers.lock().unwrap().push_back(Box::new(f));
        }

        fn answer_n(&self, n: usize, a: impl Fn() -> PromptAnswer + Send + Sync + Clone + 'static) {
            for _ in 0..n {
                let a = a.clone();
                self.answer(move |_| a());
            }
        }

        fn prompts(&self) -> Vec<Prompt> {
            self.sink.prompts.lock().unwrap().clone()
        }

        fn warnings(&self) -> Vec<ErrorCode> {
            self.sink
                .warnings
                .lock()
                .unwrap()
                .iter()
                .map(|w| w.kind.clone())
                .collect()
        }

        async fn run(
            &mut self,
            t: &mut Fake,
            cfg: &HopAuthConfig,
            agent: Option<&mut FakeAgent>,
        ) -> AuthOutcome {
            let env = AuthEnv {
                hop: hop(),
                verification: self.verification,
                broker: &self.broker,
                agent,
                secrets: &mut self.secrets,
                skip_publickey: self.skip_pk,
                keychain: self.keychain.clone(),
                on_agent_add: Some(self.agent_adds.clone()),
            };
            authenticate(t, cfg, env).await
        }
    }

    fn hop() -> HopRef {
        HopRef {
            index: 0,
            label: "web".into(),
            user: "alice".into(),
            host: "example.com".into(),
            port: 22,
        }
    }

    fn binding() -> PasswordBinding {
        PasswordBinding {
            hostname: "example.com".into(),
            port: 22,
            user: "alice".into(),
        }
    }

    fn cfg(home: &Path, plan: AuthPlan, files: &[&Path]) -> HopAuthConfig {
        HopAuthConfig {
            user: "alice".into(),
            plan,
            identity_files: files
                .iter()
                .map(|p| IdentityFile {
                    path: p.to_path_buf(),
                    explicit: true,
                })
                .collect(),
            user_ssh_dir: home.join(".ssh"),
            home: home.to_path_buf(),
            identities_only: false,
            add_keys_to_agent: false,
            pubkey_enabled: true,
            password_enabled: true,
            kbd_interactive_enabled: true,
            keychain: None,
            binding: binding(),
            can_remember_password: true,
        }
    }

    fn failed_code(out: &AuthOutcome) -> ErrorCode {
        match out {
            AuthOutcome::Failed(e) => e.kind.clone(),
            other => panic!("expected failure, got {other:?}"),
        }
    }

    fn success_info(out: AuthOutcome) -> (SessionAuthInfo, Option<Zeroizing<String>>) {
        match out {
            AuthOutcome::Success {
                info,
                remember_password,
            } => (info, remember_password),
            other => panic!("expected success, got {other:?}"),
        }
    }

    fn auth_failed(tried: &[AuthMethodKind], server: &[AuthMethodKind]) -> ErrorCode {
        ErrorCode::AuthFailed {
            user: "alice".into(),
            host: "example.com".into(),
            tried: tried.to_vec(),
            server_methods: server.to_vec(),
        }
    }

    // ----- candidate order -------------------------------------------------------------------

    struct OrderFixture {
        _dir: tempfile::TempDir,
        rsa: PathBuf,
        ed: PathBuf,
        rsa_pub: PublicKey,
        ed_pub: PublicKey,
        other: PublicKey,
    }

    fn order_fixture() -> OrderFixture {
        let dir = private_tempdir();
        let rsa = write_key(dir.path(), "id_work_rsa", RSA_OPENSSH);
        let ed = write_key(dir.path(), "id_held", ED_PLAIN);
        let rsa_pub = keys::read_key_file(&rsa, dir.path())
            .unwrap()
            .public
            .unwrap();
        OrderFixture {
            rsa,
            ed,
            rsa_pub,
            ed_pub: public(ED_PLAIN_PUB),
            other: public(ED_AGENT_PUB),
            _dir: dir,
        }
    }

    #[tokio::test]
    async fn candidate_order_agent_held_config_key_first() {
        let f = order_fixture();
        let mut agent = FakeAgent::new(
            AgentKind::UnixSocket,
            vec![
                agent_id(f.other.clone(), "other"),
                agent_id(f.ed_pub.clone(), "held"),
            ],
        );
        let mut t = Fake::new(rejecting(&[PK, PW]));
        let mut h = H::new();
        h.answer(|_| PromptAnswer::Cancel);
        let c = cfg(f._dir.path(), AuthPlan::Auto, &[&f.rsa, &f.ed]);
        let out = h.run(&mut t, &c, Some(&mut agent)).await;
        assert_eq!(failed_code(&out), ErrorCode::Cancelled);
        // (1) config key held by the agent, (2) other agent identities, (3) file keys.
        // Reserve 3 for password ⇒ exactly 3 publickey attempts.
        assert_eq!(
            t.calls,
            vec![
                Call::None,
                Call::Agent(fp(&f.ed_pub), None),
                Call::Agent(fp(&f.other), None),
                Call::BestRsa,
                Call::PublicKey(fp(&f.rsa_pub), Some(HashAlg::Sha512)),
            ]
        );
        assert!(matches!(
            h.prompts()[0],
            Prompt::Password {
                attempts_left: 3,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn identities_only_skips_other_agent_keys() {
        let f = order_fixture();
        let mut agent = FakeAgent::new(
            AgentKind::UnixSocket,
            vec![
                agent_id(f.other.clone(), "other"),
                agent_id(f.ed_pub.clone(), "held"),
            ],
        );
        let mut t = Fake::new(rejecting(&[PK]));
        let mut h = H::new();
        let mut c = cfg(f._dir.path(), AuthPlan::Auto, &[&f.rsa, &f.ed]);
        c.identities_only = true;
        let out = h.run(&mut t, &c, Some(&mut agent)).await;
        assert_eq!(
            failed_code(&out),
            auth_failed(&[AuthMethodKind::Agent, PK], &[PK])
        );
        assert_eq!(
            t.calls,
            vec![
                Call::None,
                Call::Agent(fp(&f.ed_pub), None),
                Call::BestRsa,
                Call::PublicKey(fp(&f.rsa_pub), Some(HashAlg::Sha512)),
            ]
        );
    }

    #[tokio::test]
    async fn plan_key_uses_only_that_file() {
        let f = order_fixture();
        let mut agent = FakeAgent::new(
            AgentKind::UnixSocket,
            vec![
                agent_id(f.other.clone(), "other"),
                agent_id(f.ed_pub.clone(), "held"),
            ],
        );
        let mut t = Fake::new(rejecting(&[PK, PW]));
        let mut h = H::new();
        let c = cfg(f._dir.path(), AuthPlan::Key(f.ed.clone()), &[]);
        let out = h.run(&mut t, &c, Some(&mut agent)).await;
        // Signed by the agent because it holds the key; no other identities; no password.
        assert_eq!(
            failed_code(&out),
            auth_failed(&[AuthMethodKind::Agent], &[PK, PW])
        );
        assert_eq!(t.calls, vec![Call::None, Call::Agent(fp(&f.ed_pub), None)]);
        assert!(h.prompts().is_empty());
    }

    // ----- budget / reserve / boundary -------------------------------------------------------

    fn eight_agent_keys() -> FakeAgent {
        FakeAgent::new(
            AgentKind::UnixSocket,
            (1..=8).map(|n| agent_id(extra_key(n), "extra")).collect(),
        )
    }

    /// A server that accepts `none` is reported as `none`, not keyboard-interactive.
    #[tokio::test]
    async fn accepted_none_is_reported_as_none() {
        let dir = private_tempdir();
        let mut t = Fake::new(|c, _| match c {
            Call::None => Reply::A(Ok(AttemptResult::Success)),
            _ => Reply::A(fail(&[PK])),
        });
        let mut h = H::new();
        let out = h
            .run(&mut t, &cfg(dir.path(), AuthPlan::Auto, &[]), None)
            .await;
        let (info, remember) = success_info(out);
        assert_eq!(info.method, AuthMethodUsed::None);
        assert!(remember.is_none());
    }

    #[tokio::test]
    async fn budget_without_reserve_is_six_publickey_attempts() {
        let dir = private_tempdir();
        let mut agent = eight_agent_keys();
        let mut t = Fake::new(rejecting(&[PK]));
        let mut h = H::new();
        let out = h
            .run(
                &mut t,
                &cfg(dir.path(), AuthPlan::Auto, &[]),
                Some(&mut agent),
            )
            .await;
        assert_eq!(
            failed_code(&out),
            auth_failed(&[AuthMethodKind::Agent], &[PK])
        );
        assert_eq!(count(&t.calls, |c| matches!(c, Call::Agent(..))), 6);
    }

    #[tokio::test]
    async fn reserve_keeps_three_attempts_for_password() {
        let dir = private_tempdir();
        let mut agent = eight_agent_keys();
        let mut t = Fake::new(|c, _| match c {
            Call::Password(p) if p == "right" => Reply::A(Ok(AttemptResult::Success)),
            _ => Reply::A(fail(&[PK, PW])),
        });
        let mut h = H::new();
        h.answer(|_| pw("right", false));
        let out = h
            .run(
                &mut t,
                &cfg(dir.path(), AuthPlan::Auto, &[]),
                Some(&mut agent),
            )
            .await;
        let (info, remember) = success_info(out);
        assert_eq!(info.method, AuthMethodUsed::Password);
        assert!(remember.is_none());
        assert_eq!(count(&t.calls, |c| matches!(c, Call::Agent(..))), 3);
        // §10.2: an agent with 8 keys + password still reaches password.
        assert_eq!(t.calls.last(), Some(&Call::Password("right".into())));
    }

    #[tokio::test]
    async fn disconnect_on_attempt_six_is_auth_failed() {
        let dir = private_tempdir();
        let mut agent = eight_agent_keys();
        let mut t = Fake::new(|c, history| match c {
            Call::Agent(..) if count(history, |c| matches!(c, Call::Agent(..))) == 5 => {
                Reply::A(Err(TransportError::Disconnected))
            }
            _ => Reply::A(fail(&[PK])),
        });
        let mut h = H::new();
        let out = h
            .run(
                &mut t,
                &cfg(dir.path(), AuthPlan::Auto, &[]),
                Some(&mut agent),
            )
            .await;
        assert_eq!(
            failed_code(&out),
            auth_failed(&[AuthMethodKind::Agent], &[PK])
        );
    }

    #[tokio::test]
    async fn early_disconnect_after_publickey_failure_redials_once() {
        let dir = private_tempdir();
        let mut agent = eight_agent_keys();
        // Server MaxAuthTries = 3: the 3rd attempt arrives as a disconnect.
        let mut t = Fake::new(|c, history| match c {
            Call::Agent(..) if count(history, |c| matches!(c, Call::Agent(..))) == 2 => {
                Reply::A(Err(TransportError::Disconnected))
            }
            _ => Reply::A(fail(&[PK, PW])),
        });
        let mut h = H::new();
        let out = h
            .run(
                &mut t,
                &cfg(dir.path(), AuthPlan::Auto, &[]),
                Some(&mut agent),
            )
            .await;
        assert!(matches!(out, AuthOutcome::Redial), "{out:?}");

        // The re-dial skips publickey; a second early disconnect is fatal.
        let mut t = Fake::new(|c, _| match c {
            Call::Password(_) => Reply::A(Err(TransportError::Disconnected)),
            _ => Reply::A(fail(&[PK, PW])),
        });
        h.skip_pk = true;
        h.answer(|_| pw("pw", false));
        let out = h
            .run(
                &mut t,
                &cfg(dir.path(), AuthPlan::Auto, &[]),
                Some(&mut agent),
            )
            .await;
        assert_eq!(failed_code(&out), ErrorCode::TooManyAuthFailures);
        assert_eq!(count(&t.calls, |c| matches!(c, Call::Agent(..))), 0);
    }

    #[tokio::test]
    async fn early_disconnect_without_publickey_failure_is_too_many_failures() {
        let dir = private_tempdir();
        let mut t = Fake::new(|c, _| match c {
            Call::Password(_) => Reply::A(Err(TransportError::Disconnected)),
            _ => Reply::A(fail(&[PK, PW])),
        });
        let mut h = H::new();
        h.answer(|_| pw("pw", false));
        let out = h
            .run(&mut t, &cfg(dir.path(), AuthPlan::Password, &[]), None)
            .await;
        assert_eq!(failed_code(&out), ErrorCode::TooManyAuthFailures);
    }

    // ----- RSA hash selection ----------------------------------------------------------------

    async fn agent_rsa_calls(best: Option<Option<HashAlg>>) -> Vec<Call> {
        let dir = private_tempdir();
        let rsa = write_key(dir.path(), "id_rsa_x", RSA_OPENSSH);
        let rsa_pub = keys::read_key_file(&rsa, dir.path())
            .unwrap()
            .public
            .unwrap();
        let mut agent = FakeAgent::new(AgentKind::UnixSocket, vec![agent_id(rsa_pub, "rsa")]);
        let mut t = Fake::new(move |c, _| match c {
            Call::BestRsa => Reply::R(Ok(best)),
            _ => Reply::A(fail(&[PK])),
        });
        let mut h = H::new();
        let _ = h
            .run(
                &mut t,
                &cfg(dir.path(), AuthPlan::Agent, &[]),
                Some(&mut agent),
            )
            .await;
        t.calls
            .into_iter()
            .filter(|c| matches!(c, Call::Agent(..)))
            .collect()
    }

    #[tokio::test]
    async fn rsa_hash_selection_for_agent_keys() {
        let hashes = |calls: Vec<Call>| -> Vec<Option<HashAlg>> {
            calls
                .into_iter()
                .map(|c| match c {
                    Call::Agent(_, h) => h,
                    _ => unreachable!(),
                })
                .collect()
        };
        assert_eq!(
            hashes(agent_rsa_calls(Some(Some(HashAlg::Sha512))).await),
            vec![Some(HashAlg::Sha512)]
        );
        assert_eq!(hashes(agent_rsa_calls(Some(None)).await), vec![None]);
        // No server-sig-algs: SHA-256 first, then SHA-1.
        assert_eq!(
            hashes(agent_rsa_calls(None).await),
            vec![Some(HashAlg::Sha256), None]
        );
    }

    #[tokio::test]
    async fn agent_plan_requires_agent_and_identities() {
        let dir = private_tempdir();
        let mut h = H::new();
        let mut t = Fake::new(rejecting(&[PK]));
        let out = h
            .run(&mut t, &cfg(dir.path(), AuthPlan::Agent, &[]), None)
            .await;
        assert_eq!(failed_code(&out), ErrorCode::AgentUnavailable);
        let mut empty = FakeAgent::new(AgentKind::UnixSocket, vec![]);
        let mut t = Fake::new(rejecting(&[PK]));
        let out = h
            .run(
                &mut t,
                &cfg(dir.path(), AuthPlan::Agent, &[]),
                Some(&mut empty),
            )
            .await;
        assert_eq!(failed_code(&out), ErrorCode::AgentNoIdentities);
    }

    // ----- kbd-int classification ------------------------------------------------------------

    #[test]
    fn password_prompt_regex() {
        for yes in [
            "Password:",
            "password",
            "  Password: ",
            "Password for alice@example.com:",
            "PASSWORD :",
        ] {
            assert!(is_password_prompt(yes), "{yes:?}");
        }
        for no in [
            "Passcode:",
            "Verification code:",
            "OTP",
            "Password OTP:",
            "One-time password (OTP):",
            "Password token:",
            "New password:",
            "Password for:",
            "Password for a b:",
            "Passwords:",
            "Enter password:",
        ] {
            assert!(!is_password_prompt(no), "{no:?}");
        }
    }

    // ----- passphrases -----------------------------------------------------------------------

    #[tokio::test]
    async fn passphrase_retry_then_success_and_cache() {
        let dir = private_tempdir();
        let key = write_key(dir.path(), "id_enc", ED_ENC);
        let enc_pub = public(ED_ENC_PUB);
        let accept = {
            let want = fp(&enc_pub);
            move |c: &Call, _: &[Call]| match c {
                Call::PublicKey(f, _) if *f == want => Reply::A(Ok(AttemptResult::Success)),
                _ => Reply::A(fail(&[PK])),
            }
        };
        let mut t = Fake::new(accept.clone());
        let mut h = H::new();
        h.answer(|_| passphrase("wrong-1", false));
        h.answer(|_| passphrase("wrong-2", false));
        h.answer(|_| passphrase(PASSPHRASE, false));
        let c = cfg(dir.path(), AuthPlan::Key(key.clone()), &[]);
        let (info, _) = success_info(h.run(&mut t, &c, None).await);
        assert_eq!(info.method, AuthMethodUsed::Publickey);
        assert_eq!(info.key_type.as_deref(), Some("ssh-ed25519"));
        assert!(info.key_source.unwrap().starts_with('~'));
        let seen: Vec<(u32, Option<PromptLastError>)> = h
            .prompts()
            .iter()
            .map(|p| match p {
                Prompt::Passphrase {
                    attempts_left,
                    last_error,
                    ..
                } => (*attempts_left, *last_error),
                other => panic!("{other:?}"),
            })
            .collect();
        let bad = Some(PromptLastError::PassphraseIncorrect);
        assert_eq!(seen, vec![(3, None), (2, bad), (1, bad)]);
        // Same pinned session: the decrypted key is replayed without a prompt.
        let mut t = Fake::new(accept);
        let before = h.prompts().len();
        success_info(h.run(&mut t, &c, None).await);
        assert_eq!(h.prompts().len(), before);
    }

    #[tokio::test]
    async fn three_wrong_passphrases_warn_and_move_on() {
        let dir = private_tempdir();
        let enc = write_key(dir.path(), "id_enc", ED_ENC);
        let plain = write_key(dir.path(), "id_plain", ED_PLAIN);
        let want = fp(&public(ED_PLAIN_PUB));
        let mut t = Fake::new(move |c, _| match c {
            Call::PublicKey(f, _) if *f == want => Reply::A(Ok(AttemptResult::Success)),
            _ => Reply::A(fail(&[PK])),
        });
        let mut h = H::new();
        h.answer_n(3, || passphrase("nope", false));
        let out = h
            .run(
                &mut t,
                &cfg(dir.path(), AuthPlan::Auto, &[&enc, &plain]),
                None,
            )
            .await;
        let (info, _) = success_info(out);
        assert!(info.key_source.unwrap().ends_with("id_plain"));
        assert!(matches!(
            &h.warnings()[..],
            [ErrorCode::PassphraseIncorrect { .. }]
        ));
        // The encrypted key never reached the server.
        assert_eq!(count(&t.calls, |c| matches!(c, Call::PublicKey(..))), 1);
    }

    #[tokio::test]
    async fn skip_moves_to_next_candidate_and_cancel_aborts() {
        let dir = private_tempdir();
        let enc = write_key(dir.path(), "id_enc", ED_ENC);
        let plain = write_key(dir.path(), "id_plain", ED_PLAIN);
        let c = cfg(dir.path(), AuthPlan::Auto, &[&enc, &plain]);
        let mut t = Fake::new(rejecting(&[PK]));
        let mut h = H::new();
        h.answer(|_| PromptAnswer::Skip);
        let out = h.run(&mut t, &c, None).await;
        assert_eq!(failed_code(&out), auth_failed(&[PK], &[PK]));
        assert_eq!(count(&t.calls, |c| matches!(c, Call::PublicKey(..))), 1);

        let mut t = Fake::new(rejecting(&[PK]));
        let mut h = H::new();
        h.answer(|_| PromptAnswer::Cancel);
        let out = h.run(&mut t, &c, None).await;
        assert_eq!(failed_code(&out), ErrorCode::Cancelled);
        assert_eq!(count(&t.calls, |c| matches!(c, Call::PublicKey(..))), 0);
    }

    // ----- agent add (KeyLifetime only) ------------------------------------------------------

    #[tokio::test]
    async fn add_to_agent_constraint_rejected_warns_without_unconstrained_retry() {
        let dir = private_tempdir();
        let key = write_key(dir.path(), "id_enc", ED_ENC);
        let enc_fp = fp(&public(ED_ENC_PUB));
        let want = enc_fp.clone();
        let mut t = Fake::new(move |c, _| match c {
            Call::PublicKey(f, _) if *f == want => Reply::A(Ok(AttemptResult::Success)),
            _ => Reply::A(fail(&[PK])),
        });
        let mut agent = FakeAgent::new(AgentKind::NamedPipe, vec![]);
        agent.add_ok = false;
        let mut h = H::new();
        h.answer(|p| {
            assert!(matches!(
                p,
                Prompt::Passphrase {
                    agent_available: true,
                    add_to_agent_supported: true,
                    add_to_agent_default: false,
                    ..
                }
            ));
            passphrase(PASSPHRASE, true)
        });
        let out = h
            .run(
                &mut t,
                &cfg(dir.path(), AuthPlan::Key(key), &[]),
                Some(&mut agent),
            )
            .await;
        let (info, _) = success_info(out);
        assert_eq!(info.method, AuthMethodUsed::Publickey);
        assert!(h.added.lock().unwrap().is_empty());
        assert_eq!(agent.adds, vec![(enc_fp, AGENT_KEY_LIFETIME_SECS)]);
        assert_eq!(h.warnings(), vec![ErrorCode::AgentConstraintUnsupported]);
    }

    #[tokio::test]
    async fn add_to_agent_success_and_openssh_pipe_never_gets_keys() {
        let dir = private_tempdir();
        let key = write_key(dir.path(), "id_enc", ED_ENC);
        let accept = |c: &Call, _: &[Call]| match c {
            Call::PublicKey(..) => Reply::A(Ok(AttemptResult::Success)),
            _ => Reply::A(fail(&[PK])),
        };
        // Accepting agent: the key is added with the lifetime and reported for removal.
        let mut agent = FakeAgent::new(AgentKind::UnixSocket, vec![]);
        let mut h = H::new();
        h.answer(|_| passphrase(PASSPHRASE, true));
        let mut c = cfg(dir.path(), AuthPlan::Key(key.clone()), &[]);
        c.add_keys_to_agent = true;
        success_info(h.run(&mut Fake::new(accept), &c, Some(&mut agent)).await);
        assert_eq!(h.added.lock().unwrap().len(), 1);
        assert_eq!(agent.adds.len(), 1);

        // Windows OpenSSH agent: never offered, never added (keeps keys permanently, §5.5).
        let mut pipe = FakeAgent::new(AgentKind::OpensshPipe, vec![]);
        let mut h = H::new();
        h.answer(|p| {
            assert!(matches!(
                p,
                Prompt::Passphrase {
                    add_to_agent_supported: false,
                    add_to_agent_default: false,
                    ..
                }
            ));
            passphrase(PASSPHRASE, true)
        });
        success_info(h.run(&mut Fake::new(accept), &c, Some(&mut pipe)).await);
        assert!(h.added.lock().unwrap().is_empty());
        assert!(pipe.adds.is_empty());
    }

    // ----- passwords & keychain --------------------------------------------------------------

    fn password_server(right: &'static str) -> impl FnMut(&Call, &[Call]) -> Reply + Send {
        move |c, _| match c {
            Call::Password(p) if p == right => Reply::A(Ok(AttemptResult::Success)),
            _ => Reply::A(fail(&[PW])),
        }
    }

    fn with_keychain(h: &H, c: &mut HopAuthConfig, stored: PasswordBinding) {
        h.keychain
            .entries
            .lock()
            .unwrap()
            .insert("bookmark:b1".into(), "saved-pw".into());
        c.keychain = Some(KeychainRef {
            account: "bookmark:b1".into(),
            binding: KeychainBinding::Stored(stored),
        });
    }

    #[tokio::test]
    async fn keychain_auto_use_only_for_known_hosts() {
        let dir = private_tempdir();
        let mut c = cfg(dir.path(), AuthPlan::Password, &[]);
        let mut h = H::new();
        with_keychain(&h, &mut c, binding());
        let (info, _) = success_info(
            h.run(&mut Fake::new(password_server("saved-pw")), &c, None)
                .await,
        );
        assert_eq!(info.method, AuthMethodUsed::Password);
        assert!(h.prompts().is_empty());

        // Auto-accepted host key: prompt with the "Use saved password" option instead.
        let mut h = H::new();
        h.verification = HostKeyVerification::AutoAccepted;
        with_keychain(&h, &mut c, binding());
        h.answer(|p| {
            assert!(matches!(
                p,
                Prompt::Password {
                    saved_password_available: true,
                    can_remember: true,
                    ..
                }
            ));
            PromptAnswer::UseSavedPassword
        });
        let mut t = Fake::new(password_server("saved-pw"));
        success_info(h.run(&mut t, &c, None).await);
        assert_eq!(t.calls.last(), Some(&Call::Password("saved-pw".into())));
    }

    #[tokio::test]
    async fn use_saved_password_with_stale_binding_reprompts() {
        let dir = private_tempdir();
        let mut c = cfg(dir.path(), AuthPlan::Password, &[]);
        let mut h = H::new();
        let stale = PasswordBinding {
            port: 2222,
            ..binding()
        };
        with_keychain(&h, &mut c, stale);
        h.answer(|p| {
            assert!(matches!(
                p,
                Prompt::Password {
                    saved_password_available: false,
                    ..
                }
            ));
            PromptAnswer::UseSavedPassword // advisory only: the backend re-checks
        });
        h.answer(|p| {
            assert!(matches!(
                p,
                Prompt::Password {
                    saved_password_available: false,
                    attempts_left: 3,
                    ..
                }
            ));
            pw("typed", true)
        });
        let mut t = Fake::new(password_server("typed"));
        let (_, remember) = success_info(h.run(&mut t, &c, None).await);
        assert_eq!(remember.as_deref().map(String::as_str), Some("typed"));
        // The stale saved password was never sent.
        assert!(!t.calls.contains(&Call::Password("saved-pw".into())));
    }

    /// "Use saved password" availability is looked up once per authentication, not read
    /// from the keychain on every prompt.
    #[tokio::test]
    async fn saved_password_is_looked_up_once_per_authentication() {
        let dir = private_tempdir();
        let mut c = cfg(dir.path(), AuthPlan::Password, &[]);
        let mut h = H::new();
        h.verification = HostKeyVerification::AutoAccepted;
        with_keychain(&h, &mut c, binding());
        for _ in 0..3 {
            h.answer(|p| {
                assert!(matches!(
                    p,
                    Prompt::Password {
                        saved_password_available: true,
                        ..
                    }
                ));
                pw("wrong", false)
            });
        }
        let out = h
            .run(&mut Fake::new(password_server("saved-pw")), &c, None)
            .await;
        assert!(matches!(out, AuthOutcome::Failed(_)));
        assert_eq!(h.prompts().len(), 3);
        assert_eq!(
            h.keychain.reads.load(std::sync::atomic::Ordering::Relaxed),
            1
        );
    }

    /// PAM password + OTP that checks the password only after the code. A stale cached
    /// password (e.g. replayed on reconnect) is kept for one retry, then dropped and
    /// asked for again instead of being replayed until the budget runs out.
    #[tokio::test]
    async fn stale_cached_password_is_dropped_after_one_failed_retry() {
        let dir = private_tempdir();
        let mut t = Fake::new(|c, _| match c {
            Call::None => Reply::A(fail(&[KI])),
            Call::KbdStart => info(&[("Password: ", false)]),
            Call::KbdRespond(r) if r.len() == 1 && r[0].starts_with("pw") => {
                info(&[("Verification code: ", false)])
            }
            // Success only for the new password + code; the old password fails after the code.
            Call::KbdRespond(r) if r == &["123456".to_owned()] => Reply::K(Ok(KbdStep::Success)),
            _ => Reply::K(Ok(KbdStep::Failure {
                remaining: vec![KI],
                partial_success: false,
            })),
        });
        let mut h = H::new();
        h.secrets.set_password(Zeroizing::new("pw-old".into()));
        let code = |v: &str| PromptAnswer::KeyboardInteractive {
            responses: vec![Secret::new(v.to_owned())],
        };
        h.answer(move |_| code("111111"));
        h.answer(move |_| code("222222"));
        h.answer(|p| {
            assert!(matches!(p, Prompt::Password { .. }), "{p:?}");
            pw("pw-new", false)
        });
        h.answer(move |_| code("123456"));
        let out = h
            .run(&mut t, &cfg(dir.path(), AuthPlan::Auto, &[]), None)
            .await;
        // The fake accepts "123456" after any password; what matters is the prompt order.
        success_info(out);
        let kinds: Vec<&str> = h
            .prompts()
            .iter()
            .map(|p| match p {
                Prompt::Password { .. } => "password",
                Prompt::KeyboardInteractive { .. } => "code",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, ["code", "code", "password", "code"]);
        assert_eq!(
            count(
                &t.calls,
                |c| matches!(c, Call::KbdRespond(r) if r == &["pw-old".to_owned()])
            ),
            2,
            "the cached password is replayed once more, then dropped"
        );
    }

    /// `rememberPasswords = false`: the prompt says `canRemember: false` and a racing
    /// `remember: true` answer leaves nothing to store.
    #[tokio::test]
    async fn remember_answer_is_ignored_when_remembering_is_off() {
        let dir = private_tempdir();
        let mut c = cfg(dir.path(), AuthPlan::Password, &[]);
        let mut h = H::new();
        // A keychain account exists (nothing saved yet), so only the setting forbids storing.
        c.keychain = Some(KeychainRef {
            account: "bookmark:b1".into(),
            binding: KeychainBinding::Stored(binding()),
        });
        c.can_remember_password = false;
        h.answer(|p| {
            assert!(matches!(
                p,
                Prompt::Password {
                    can_remember: false,
                    ..
                }
            ));
            pw("typed", true)
        });
        let mut t = Fake::new(password_server("typed"));
        let (_, remember) = success_info(h.run(&mut t, &c, None).await);
        assert!(remember.is_none());
    }

    #[tokio::test]
    async fn three_wrong_passwords_fail_with_attempt_feedback() {
        let dir = private_tempdir();
        let c = cfg(dir.path(), AuthPlan::Auto, &[]);
        let mut h = H::new();
        h.answer_n(3, || pw("wrong", false));
        let out = h
            .run(&mut Fake::new(password_server("right")), &c, None)
            .await;
        assert_eq!(failed_code(&out), auth_failed(&[PW], &[PW]));
        let seen: Vec<(u32, Option<PromptLastError>)> = h
            .prompts()
            .iter()
            .map(|p| match p {
                Prompt::Password {
                    attempts_left,
                    last_error,
                    ..
                } => (*attempts_left, *last_error),
                other => panic!("{other:?}"),
            })
            .collect();
        let bad = Some(PromptLastError::PasswordIncorrect);
        assert_eq!(seen, vec![(3, None), (2, bad), (1, bad)]);
    }

    #[tokio::test]
    async fn session_cached_password_is_replayed_and_skip_ends_password() {
        let dir = private_tempdir();
        let c = cfg(dir.path(), AuthPlan::Password, &[]);
        let mut h = H::new();
        h.secrets.set_password(Zeroizing::new("cached".into()));
        success_info(
            h.run(&mut Fake::new(password_server("cached")), &c, None)
                .await,
        );
        assert!(h.prompts().is_empty());

        let mut h = H::new();
        h.answer(|_| PromptAnswer::Skip);
        let mut t = Fake::new(password_server("x"));
        let out = h.run(&mut t, &c, None).await;
        assert_eq!(failed_code(&out), auth_failed(&[], &[PW]));
        assert_eq!(t.calls, vec![Call::None]);
    }

    // ----- keyboard-interactive & 2FA --------------------------------------------------------

    fn info(prompts: &[(&str, bool)]) -> Reply {
        Reply::K(Ok(KbdStep::InfoRequest {
            name: String::new(),
            instructions: String::new(),
            prompts: prompts.iter().map(|(l, e)| ((*l).to_owned(), *e)).collect(),
        }))
    }

    #[tokio::test]
    async fn two_factor_publickey_then_password_and_code() {
        let dir = private_tempdir();
        let plain = write_key(dir.path(), "id_ed25519", ED_PLAIN);
        let mut t = Fake::new(|c, _| match c {
            Call::None => Reply::A(fail(&[PK])),
            Call::PublicKey(..) => Reply::A(Ok(AttemptResult::Failure {
                remaining: vec![KI],
                partial_success: true,
            })),
            Call::KbdStart => info(&[("Password: ", false)]),
            Call::KbdRespond(r) if r == &["pw".to_owned()] => {
                info(&[("Verification code: ", false)])
            }
            Call::KbdRespond(r) if r == &["123456".to_owned()] => Reply::K(Ok(KbdStep::Success)),
            _ => Reply::K(Ok(KbdStep::Failure {
                remaining: vec![KI],
                partial_success: false,
            })),
        });
        let mut h = H::new();
        h.answer(|p| {
            assert!(
                matches!(p, Prompt::Password { server_label: Some(l), .. } if l == "Password: ")
            );
            pw("pw", false)
        });
        h.answer(|p| {
            assert!(matches!(p, Prompt::KeyboardInteractive { fields, .. } if fields.len() == 1 && !fields[0].echo));
            PromptAnswer::KeyboardInteractive { responses: vec![Secret::new("123456".to_owned())] }
        });
        let out = h
            .run(&mut t, &cfg(dir.path(), AuthPlan::Auto, &[&plain]), None)
            .await;
        let (info, _) = success_info(out);
        assert_eq!(info.method, AuthMethodUsed::KeyboardInteractive);
        // Key info of the partial publickey success is kept for the status bar.
        assert_eq!(info.key_type.as_deref(), Some("ssh-ed25519"));
    }

    #[tokio::test]
    async fn kbd_too_many_rounds_fails() {
        let dir = private_tempdir();
        let mut t = Fake::new(|c, _| match c {
            Call::None => Reply::A(fail(&[KI])),
            _ => info(&[("Name:", true)]),
        });
        let mut h = H::new();
        h.answer_n(12, || PromptAnswer::KeyboardInteractive {
            responses: vec![Secret::new("x".to_owned())],
        });
        let out = h
            .run(&mut t, &cfg(dir.path(), AuthPlan::Auto, &[]), None)
            .await;
        assert_eq!(failed_code(&out), auth_failed(&[KI], &[KI]));
        assert_eq!(h.prompts().len(), MAX_KBD_ROUNDS);
    }

    #[tokio::test]
    async fn kbd_password_like_prompt_retries_up_to_three_times() {
        let dir = private_tempdir();
        let mut t = Fake::new(|c, _| match c {
            Call::None => Reply::A(fail(&[KI])),
            Call::KbdStart => info(&[("Password:", false)]),
            Call::KbdRespond(r) if r == &["right".to_owned()] => Reply::K(Ok(KbdStep::Success)),
            _ => Reply::K(Ok(KbdStep::Failure {
                remaining: vec![KI],
                partial_success: false,
            })),
        });
        let mut h = H::new();
        h.answer(|_| pw("w1", false));
        h.answer(|_| pw("w2", false));
        h.answer(|p| {
            assert!(matches!(
                p,
                Prompt::Password {
                    attempts_left: 1,
                    last_error: Some(PromptLastError::PasswordIncorrect),
                    ..
                }
            ));
            pw("right", true)
        });
        let mut c = cfg(dir.path(), AuthPlan::Auto, &[]);
        c.keychain = Some(KeychainRef {
            account: "sshconfig:alice@example.com:22".into(),
            binding: KeychainBinding::ImpliedByAccount,
        });
        let (info, remember) = success_info(h.run(&mut t, &c, None).await);
        assert_eq!(info.method, AuthMethodUsed::KeyboardInteractive);
        assert_eq!(remember.as_deref().map(String::as_str), Some("right"));
        assert_eq!(count(&t.calls, |c| matches!(c, Call::KbdStart)), 3);
    }

    /// A password typed into a keyboard-interactive "Password:" prompt that the server
    /// rejects shows up as a tried `password` in `authFailed`.
    #[tokio::test]
    async fn kbd_password_failure_reports_password_as_tried() {
        let dir = private_tempdir();
        let mut t = Fake::new(|c, _| match c {
            Call::None => Reply::A(fail(&[KI])),
            Call::KbdStart => info(&[("Password:", false)]),
            _ => Reply::K(Ok(KbdStep::Failure {
                remaining: vec![KI],
                partial_success: false,
            })),
        });
        let mut h = H::new();
        h.answer(|_| pw("w1", false));
        h.answer(|_| pw("w2", false));
        h.answer(|_| pw("w3", false));
        let c = cfg(dir.path(), AuthPlan::Auto, &[]);
        let AuthOutcome::Failed(e) = h.run(&mut t, &c, None).await else {
            panic!("expected a failure");
        };
        let ErrorCode::AuthFailed { tried, .. } = e.kind else {
            panic!("expected authFailed, got {e:?}");
        };
        assert!(tried.contains(&AuthMethodKind::Password), "{tried:?}");
        assert!(tried.contains(&AuthMethodKind::KeyboardInteractive));
    }

    #[test]
    fn secrets_debug_is_masked() {
        let mut s = HopSecrets::default();
        s.set_password(Zeroizing::new("hunter2".into()));
        let dbg = format!("{s:?}");
        assert!(!dbg.contains("hunter2"), "{dbg}");
    }

    // ----- regressions --------------------------------------------------------

    /// Password ok → code wrong → failure ⇒ the code is asked again in a new exchange,
    /// the cached password is replayed (not re-asked), no "incorrect password".
    #[tokio::test]
    async fn wrong_second_factor_keeps_password_and_reprompts_code() {
        let dir = private_tempdir();
        let mut t = Fake::new(|c, _| match c {
            Call::None => Reply::A(fail(&[KI])),
            Call::KbdStart => info(&[("Password: ", false)]),
            Call::KbdRespond(r) if r == &["pw".to_owned()] => {
                info(&[("Verification code: ", false)])
            }
            Call::KbdRespond(r) if r == &["123456".to_owned()] => Reply::K(Ok(KbdStep::Success)),
            _ => Reply::K(Ok(KbdStep::Failure {
                remaining: vec![KI],
                partial_success: false,
            })),
        });
        let mut h = H::new();
        h.answer(|_| pw("pw", false));
        h.answer(|_| PromptAnswer::KeyboardInteractive {
            responses: vec![Secret::new("000000".to_owned())],
        });
        h.answer(|_| PromptAnswer::KeyboardInteractive {
            responses: vec![Secret::new("123456".to_owned())],
        });
        let (info, _) = success_info(
            h.run(&mut t, &cfg(dir.path(), AuthPlan::Auto, &[]), None)
                .await,
        );
        assert_eq!(info.method, AuthMethodUsed::KeyboardInteractive);
        let kinds: Vec<&str> = h
            .prompts()
            .iter()
            .map(|p| match p {
                Prompt::Password { .. } => "password",
                Prompt::KeyboardInteractive { .. } => "code",
                _ => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            ["password", "code", "code"],
            "the password must not be re-asked"
        );
        assert_eq!(count(&t.calls, |c| matches!(c, Call::KbdStart)), 2);
        assert_eq!(
            count(
                &t.calls,
                |c| matches!(c, Call::KbdRespond(r) if r == &["pw".to_owned()])
            ),
            2,
            "the cached password is replayed in the second exchange"
        );
    }

    /// A lost transport is `Lost(disconnected{reason})`, never `tooManyAuthFailures`.
    #[tokio::test]
    async fn transport_loss_is_reported_as_lost() {
        let dir = private_tempdir();
        let mut t = Fake::new(|c, _| match c {
            Call::None => Reply::A(fail(&[PW])),
            _ => Reply::A(Err(TransportError::Lost(DisconnectKind::ConnectionReset))),
        });
        let mut h = H::new();
        h.answer(|_| pw("typed", false));
        let out = h
            .run(&mut t, &cfg(dir.path(), AuthPlan::Password, &[]), None)
            .await;
        match out {
            AuthOutcome::Lost(e) => assert_eq!(
                e.kind,
                ErrorCode::Disconnected {
                    reason: DisconnectKind::ConnectionReset,
                    message: None
                }
            ),
            other => panic!("expected Lost, got {other:?}"),
        }
        // The answer is cached for the §5.4 re-dial.
        assert!(h.secrets.password.is_some());
    }

    /// Russh reports "closed" before `Handler::disconnected` records the reason. A
    /// loss recorded shortly AFTER the closure is observed is still `Lost`; a recorded
    /// SSH_MSG_DISCONNECT or nothing at all (within the grace) stays `Disconnected`.
    #[tokio::test]
    async fn late_recorded_loss_is_still_lost() {
        let record_later = |kind: Option<DisconnectKind>| {
            let shared = Arc::new(HopShared::default());
            let s = shared.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                if let Some(kind) = kind {
                    *s.loss.lock().unwrap() = Some(kind);
                }
                s.closed.notify_waiters();
            });
            shared
        };
        let grace = Duration::from_secs(5);

        let shared = record_later(Some(DisconnectKind::ConnectionReset));
        let t0 = std::time::Instant::now();
        let e = classify_closed(Some(&shared), grace).await;
        assert!(
            matches!(e, TransportError::Lost(DisconnectKind::ConnectionReset)),
            "{e:?}"
        );
        assert!(
            t0.elapsed() < grace,
            "woken by the notification, not the timeout"
        );

        let shared = record_later(Some(DisconnectKind::ServerClosed));
        let e = classify_closed(Some(&shared), grace).await;
        assert!(matches!(e, TransportError::Disconnected), "{e:?}");

        // Already recorded: no wait.
        let shared = HopShared::default();
        *shared.loss.lock().unwrap() = Some(DisconnectKind::KeepaliveTimeout);
        let e = classify_closed(Some(&shared), grace).await;
        assert!(
            matches!(e, TransportError::Lost(DisconnectKind::KeepaliveTimeout)),
            "{e:?}"
        );

        // Nothing recorded within the grace (e.g. russh's shutdown failed first).
        let shared = HopShared::default();
        let e = classify_closed(Some(&shared), Duration::from_millis(20)).await;
        assert!(matches!(e, TransportError::Disconnected), "{e:?}");
        assert!(matches!(
            classify_closed(None, grace).await,
            TransportError::Disconnected
        ));
    }

    /// A bookmark account WITHOUT a stored binding never auto-uses or offers the entry.
    #[tokio::test]
    async fn missing_binding_is_a_mismatch() {
        let dir = private_tempdir();
        let mut c = cfg(dir.path(), AuthPlan::Password, &[]);
        let mut h = H::new();
        h.keychain
            .entries
            .lock()
            .unwrap()
            .insert("bookmark:b1".into(), "orphaned-pw".into());
        c.keychain = Some(KeychainRef {
            account: "bookmark:b1".into(),
            binding: KeychainBinding::Missing,
        });
        h.answer(|p| {
            assert!(matches!(
                p,
                Prompt::Password {
                    saved_password_available: false,
                    ..
                }
            ));
            PromptAnswer::UseSavedPassword
        });
        h.answer(|_| pw("typed", false));
        let mut t = Fake::new(password_server("typed"));
        success_info(h.run(&mut t, &c, None).await);
        assert!(!t.calls.contains(&Call::Password("orphaned-pw".into())));
    }

    /// "Add to ssh-agent" adds the key only after the server accepted it; a rejected
    /// key never reaches the agent. Every add is reported to the session immediately.
    #[tokio::test]
    async fn agent_add_only_after_acceptance() {
        let dir = private_tempdir();
        let key = write_key(dir.path(), "id_enc", ED_ENC);
        let mut c = cfg(dir.path(), AuthPlan::Key(key), &[]);
        c.add_keys_to_agent = true;

        // Rejected by the server: nothing added.
        let mut agent = FakeAgent::new(AgentKind::UnixSocket, vec![]);
        let mut h = H::new();
        h.answer(|_| passphrase(PASSPHRASE, true));
        let out = h
            .run(&mut Fake::new(rejecting(&[PK])), &c, Some(&mut agent))
            .await;
        assert!(matches!(out, AuthOutcome::Failed(_)));
        assert!(agent.adds.is_empty());
        assert!(h.added.lock().unwrap().is_empty());

        // Accepted: added once and reported through `on_agent_add`.
        let mut agent = FakeAgent::new(AgentKind::UnixSocket, vec![]);
        let mut h = H::new();
        h.answer(|_| passphrase(PASSPHRASE, true));
        let accept = |c: &Call, _: &[Call]| match c {
            Call::PublicKey(..) => Reply::A(Ok(AttemptResult::Success)),
            _ => Reply::A(fail(&[PK])),
        };
        success_info(h.run(&mut Fake::new(accept), &c, Some(&mut agent)).await);
        assert_eq!(agent.adds.len(), 1);
        assert_eq!(h.added.lock().unwrap().len(), 1);
    }

    /// A key the agent already holds is not added again (removing "our" copy on close
    /// would delete the user's own entry).
    #[tokio::test]
    async fn key_already_in_agent_is_not_added_again() {
        let dir = private_tempdir();
        let key = write_key(dir.path(), "id_enc", ED_ENC);
        let mut c = cfg(dir.path(), AuthPlan::Key(key), &[]);
        c.add_keys_to_agent = true;
        // The candidate listing does not show the key; by the time of the add, the agent has it.
        let mut agent = FakeAgent::new(AgentKind::UnixSocket, vec![]);
        agent.ids_later = Some(vec![agent_id(public(ED_ENC_PUB), "user's own copy")]);
        let mut h = H::new();
        h.answer(|_| passphrase(PASSPHRASE, true));
        let accept = |c: &Call, _: &[Call]| match c {
            Call::PublicKey(..) => Reply::A(Ok(AttemptResult::Success)),
            _ => Reply::A(fail(&[PK])),
        };
        success_info(h.run(&mut Fake::new(accept), &c, Some(&mut agent)).await);
        assert!(agent.adds.is_empty());
        assert!(h.added.lock().unwrap().is_empty());
    }

    /// `AuthOutcome` never prints a remembered password.
    #[test]
    fn auth_outcome_debug_masks_password() {
        let out = AuthOutcome::Success {
            info: SessionAuthInfo {
                method: AuthMethodUsed::Password,
                key_type: None,
                key_source: None,
            },
            remember_password: Some(Zeroizing::new("hunter2".into())),
        };
        let dbg = format!("{out:?}");
        assert!(!dbg.contains("hunter2"), "{dbg}");
        assert!(dbg.contains("***"));
    }
}
