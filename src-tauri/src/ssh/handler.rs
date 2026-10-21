//! `ClientHandler` (§5.3, §9.5): host-key verification, banner, disconnect capture, and
//! rejection of every server-initiated channel (`AdministrativelyProhibited`).
//!
//! Host-key decision (§5.3, binding pseudo-code):
//! 1. fatal verdicts first — `Revoked`, `CaManaged`, `Changed` — they win over pins and
//!    trust-once approvals (so they also apply on re-dial/auto-reconnect);
//! 2. reconnect pinning: same fingerprint ⇒ ok; known_hosts-trusted ⇒ re-pin; else
//!    `hostKeyMismatchOnReconnect`;
//! 3. trust-once approvals ⇒ `userConfirmed`;
//! 4. `Trusted` ⇒ `knownHosts`; `Unknown` ⇒ by `StrictHostKeyChecking` (yes: fail; accept-new/no:
//!    learn if a writable user file exists, `autoAccepted`; ask: prompt). A host already known
//!    under another key type always prompts ([`unknown_key_mode`]).
//!    Certificates are never negotiated; if one is presented ⇒ `hostKeyRejected`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use russh::client::{ChannelOpenHandle, DisconnectReason, Handler, Msg, Session};
use russh::keys::{HashAlg, PublicKeyOrCertificate};
use russh::{Channel, ChannelOpenFailure};

use crate::error::{AppError, DisconnectKind, ErrorCode, HintShell};
use crate::ipc::dto::{
    HopRef, HostKeyInfo, HostKeyVerification, Prompt, PromptAnswer, SessionEvent,
};
use crate::known_hosts::{
    self, HostKeyRef, KnownHostsFile, PinDecision, RejectReason, TrustOnceApprovals, Verdict,
};
use crate::ssh::prompt::{PromptBroker, PromptOutcome};
use crate::sshconfig::StrictMode;

/// Banners longer than this are truncated (§9.5).
pub const MAX_BANNER_BYTES: usize = 8 * 1024;

/// Everything the host-key check of one hop needs (prepared by `ssh::connect`).
pub struct HostKeyPolicy {
    pub hop: HopRef,
    /// Lookup name (`HostKeyAlias` verbatim, else `host` or `[host]:port`).
    pub lookup: String,
    /// Parsed user + global known_hosts files that were readable and trusted.
    pub files: Arc<[KnownHostsFile]>,
    /// Where `learn()` appends (first enabled user file, writable location), if any.
    pub learn_target: Option<PathBuf>,
    /// Windows: learning only into files under `%USERPROFILE%`.
    pub profile_root: Option<PathBuf>,
    pub strict: StrictMode,
    pub hash_known_hosts: bool,
    /// Key pinned for this hop earlier in the session (re-dial / reconnect).
    pub pinned: Option<HostKeyPin>,
    pub approvals: Arc<TrustOnceApprovals>,
}

/// The `StrictHostKeyChecking` mode a hop actually uses (defence in depth):
/// `accept-new`/`no` auto-accept an unknown key only while the user's known_hosts setup is fully
/// honoured. A user file that exists but was ignored (its hosts would look new: a changed key
/// would be silently accepted), or no place to record the key although the user files were not
/// disabled (`none`, `/dev/null`), makes the hop ask instead.
pub fn effective_strict(
    strict: StrictMode,
    user_file_ignored: bool,
    can_record_or_disabled: bool,
) -> StrictMode {
    match strict {
        StrictMode::AcceptNew | StrictMode::No if user_file_ignored || !can_record_or_disabled => {
            StrictMode::Ask
        }
        s => s,
    }
}

/// How an `Unknown` key is handled: `accept-new`/`no` auto-accept a key only when the host
/// is new. A host already recorded under another key type (`other_types` non-empty) offering a
/// key of a type we have no record for looks exactly like an algorithm-downgrade MITM, so the
/// user is asked instead (the prompt lists the known types).
pub fn unknown_key_mode(strict: StrictMode, other_types: &[String]) -> StrictMode {
    match strict {
        StrictMode::AcceptNew | StrictMode::No if !other_types.is_empty() => StrictMode::Ask,
        s => s,
    }
}

/// Per-hop host-key pin (§5.3 "on success: pins[hop] = (lookup, fingerprint, verification)").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKeyPin {
    pub lookup: String,
    pub fingerprint: String,
    pub verification: HostKeyVerification,
}

/// What the handler decided; read by `ssh::connect` after the handshake.
#[derive(Debug, Clone)]
pub enum HostKeyOutcome {
    Accepted(HostKeyInfo),
    Rejected(AppError),
}

/// State shared between the handler (moved into russh) and the connect layer.
#[derive(Debug, Default)]
pub struct HopShared {
    pub host_key: Mutex<Option<HostKeyOutcome>>,
    /// Text of a received SSH_MSG_DISCONNECT (server text; shown via displayServerText).
    pub disconnect_message: Mutex<Option<String>>,
    /// Why the hop's connection ended (set by `disconnected`).
    pub loss: Mutex<Option<DisconnectKind>>,
    /// Notified when the hop's connection ended.
    pub closed: tokio::sync::Notify,
    /// Banners forwarded so far (at most [`MAX_BANNERS_PER_HOP`]).
    pub banners: std::sync::atomic::AtomicUsize,
}

/// A server may send any number of `USERAUTH_BANNER`s; more than this per hop are dropped.
pub const MAX_BANNERS_PER_HOP: usize = 16;

/// Cut server text to at most `max` bytes on a char boundary (banners, disconnect messages).
pub fn truncate_server_text(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut cut = max;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text.get(..cut).unwrap_or_default().to_owned()
}

impl HopShared {
    fn set_loss(&self, kind: DisconnectKind) {
        self.loss
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert(kind);
    }

    pub fn loss(&self) -> Option<DisconnectKind> {
        *self.loss.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn disconnect_message(&self) -> Option<String> {
        self.disconnect_message
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn host_key(&self) -> Option<HostKeyOutcome> {
        self.host_key
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set_host_key(&self, outcome: HostKeyOutcome) {
        *self.host_key.lock().unwrap_or_else(PoisonError::into_inner) = Some(outcome);
    }
}

pub struct ClientHandler {
    pub policy: HostKeyPolicy,
    pub broker: Arc<PromptBroker>,
    pub shared: Arc<HopShared>,
}

fn host_port(hop: &HopRef) -> (String, u16) {
    (hop.host.clone(), hop.port)
}

/// The §5.3 decision for a plain public key. Learning (file I/O) runs in `spawn_blocking`.
pub async fn decide(
    policy: &HostKeyPolicy,
    broker: &PromptBroker,
    key_type: &str,
    blob: &[u8],
) -> Result<HostKeyInfo, AppError> {
    let (host, port) = host_port(&policy.hop);
    let fingerprint = known_hosts::fingerprint_sha256(blob);
    let verdict = known_hosts::verify(&policy.lookup, HostKeyRef { key_type, blob }, &policy.files);
    let accepted = |verification| HostKeyInfo {
        key_type: key_type.to_owned(),
        fingerprint: fingerprint.clone(),
        verification,
    };

    // 1–2. Fatal verdicts, then the pin (if any).
    let fatal = |reason: RejectReason| -> AppError {
        match reason {
            RejectReason::Revoked => AppError::new(ErrorCode::HostKeyRevoked {
                host: host.clone(),
                port,
                fingerprint: fingerprint.clone(),
            }),
            RejectReason::CaManaged { file, line } => AppError::new(ErrorCode::HostKeyCaManaged {
                host: host.clone(),
                port,
                fingerprint: fingerprint.clone(),
                known_hosts_file: file,
                line,
            }),
            RejectReason::Changed { file, line } => {
                // The backend builds the removal command with the REAL
                // known_hosts entry name (HostKeyAlias, lowercased lookup) for the local shell.
                let shell = HintShell::local();
                let remove_hint =
                    crate::ssh::shellquote::keygen_remove_hint(&policy.lookup, &file, shell);
                AppError::new(ErrorCode::HostKeyChanged {
                    host: host.clone(),
                    port,
                    key_type: key_type.to_owned(),
                    fingerprint: fingerprint.clone(),
                    known_hosts_file: file,
                    line,
                    remove_hint,
                    remove_hint_shell: shell,
                })
            }
            RejectReason::MismatchOnReconnect { expected, actual } => {
                AppError::new(ErrorCode::HostKeyMismatchOnReconnect {
                    host: host.clone(),
                    port,
                    expected,
                    actual,
                })
            }
        }
    };
    match &verdict {
        Verdict::Revoked { .. } => return Err(fatal(RejectReason::Revoked)),
        Verdict::CaManaged { file, line } => {
            return Err(fatal(RejectReason::CaManaged {
                file: file.clone(),
                line: *line,
            }));
        }
        Verdict::Changed { file, line } => {
            return Err(fatal(RejectReason::Changed {
                file: file.clone(),
                line: *line,
            }));
        }
        _ => {}
    }
    if let Some(pin) = &policy.pinned {
        return match known_hosts::check_against_pin(&verdict, &pin.fingerprint, &fingerprint) {
            PinDecision::Accept => Ok(accepted(if matches!(verdict, Verdict::Trusted { .. }) {
                HostKeyVerification::KnownHosts
            } else {
                // Same key as pinned: keep how it was verified (an auto-accepted key stays unverified).
                pin.verification
            })),
            PinDecision::Repin => Ok(accepted(HostKeyVerification::KnownHosts)),
            PinDecision::Reject(reason) => Err(fatal(reason)),
        };
    }

    // 3. Trust-once approvals.
    if policy.approvals.contains(&policy.lookup, &fingerprint) {
        return Ok(accepted(HostKeyVerification::UserConfirmed));
    }

    // 4. Normal verdict handling.
    let other_types = match verdict {
        Verdict::Trusted { .. } => return Ok(accepted(HostKeyVerification::KnownHosts)),
        Verdict::Unknown { other_types } => other_types,
        // Fatal verdicts were handled above.
        Verdict::Revoked { .. } | Verdict::CaManaged { .. } | Verdict::Changed { .. } => {
            return Err(AppError::internal("unreachable host-key verdict"));
        }
    };
    match unknown_key_mode(policy.strict, &other_types) {
        StrictMode::Yes => Err(AppError::new(ErrorCode::HostKeyUnknownStrict {
            host,
            port,
            key_type: key_type.to_owned(),
            fingerprint,
        })),
        StrictMode::AcceptNew | StrictMode::No => {
            if policy.learn_target.is_some() {
                learn(policy, key_type, blob).await;
            }
            Ok(accepted(HostKeyVerification::AutoAccepted))
        }
        StrictMode::Ask => {
            let outcome = broker
                .ask(|id| Prompt::HostKeyUnknown {
                    id,
                    hop: policy.hop.clone(),
                    known_hosts_name: policy.lookup.clone(),
                    key_type: key_type.to_owned(),
                    fingerprint: fingerprint.clone(),
                    known_hosts_file: policy
                        .learn_target
                        .as_ref()
                        .map(|p| p.display().to_string()),
                    other_key_types_known: other_types,
                    can_remember: policy.learn_target.is_some(),
                })
                .await;
            match outcome {
                PromptOutcome::Answer(PromptAnswer::HostKey {
                    accept: true,
                    remember,
                }) => {
                    if remember
                        && policy.learn_target.is_some()
                        && learn(policy, key_type, blob).await
                    {
                        // Learned: also valid for re-dials in this process.
                    } else {
                        policy.approvals.insert(&policy.lookup, &fingerprint);
                    }
                    Ok(accepted(HostKeyVerification::UserConfirmed))
                }
                PromptOutcome::Answer(PromptAnswer::Cancel) | PromptOutcome::Withdrawn => {
                    Err(AppError::new(ErrorCode::Cancelled))
                }
                PromptOutcome::Timeout => Err(AppError::new(ErrorCode::PromptTimeout)),
                PromptOutcome::Answer(_) => {
                    Err(AppError::new(ErrorCode::HostKeyRejected { host, port }))
                }
            }
        }
    }
}

/// Append the key to the learn target (§5.3 Writing). Failures are logged; returns success.
async fn learn(policy: &HostKeyPolicy, key_type: &str, blob: &[u8]) -> bool {
    let Some(file) = policy.learn_target.clone() else {
        return false;
    };
    let lookup = policy.lookup.clone();
    let key_type = key_type.to_owned();
    let blob = blob.to_vec();
    let hash = policy.hash_known_hosts;
    let profile = policy.profile_root.clone();
    let result = tokio::task::spawn_blocking(move || {
        known_hosts::learn(
            &file,
            &lookup,
            HostKeyRef {
                key_type: &key_type,
                blob: &blob,
            },
            hash,
            profile.as_deref(),
        )
    })
    .await;
    match result {
        Ok(Ok(())) => true,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "could not add the host key to known_hosts");
            false
        }
        Err(e) => {
            tracing::warn!(error = %e, "known_hosts learn task failed");
            false
        }
    }
}

macro_rules! reject_channel {
    ($reply:expr) => {{
        $reply
            .reject(ChannelOpenFailure::AdministrativelyProhibited)
            .await;
        Ok(())
    }};
}

impl Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let decision = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => match key.to_bytes() {
                Ok(blob) => {
                    let key_type = key.algorithm().to_string();
                    debug_assert_eq!(
                        known_hosts::fingerprint_sha256(&blob),
                        key.fingerprint(HashAlg::Sha256).to_string()
                    );
                    decide(&self.policy, &self.broker, &key_type, &blob).await
                }
                // Fail closed; an empty blob would be checked, pinned and trusted as
                // the key with fingerprint SHA256("").
                Err(_) => Err(AppError::new(ErrorCode::HostKeyRejected {
                    host: self.policy.hop.host.clone(),
                    port: self.policy.hop.port,
                })),
            },
            // Host certificates are never negotiated (§1.1); fail closed if one arrives.
            _ => Err(AppError::new(ErrorCode::HostKeyRejected {
                host: self.policy.hop.host.clone(),
                port: self.policy.hop.port,
            })),
        };
        match decision {
            Ok(info) => {
                self.shared.set_host_key(HostKeyOutcome::Accepted(info));
                Ok(true)
            }
            Err(err) => {
                self.shared.set_host_key(HostKeyOutcome::Rejected(err));
                Ok(false)
            }
        }
    }

    async fn auth_banner(
        &mut self,
        banner: &str,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let n = self
            .shared
            .banners
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if n >= MAX_BANNERS_PER_HOP {
            return Ok(());
        }
        let text = truncate_server_text(banner, MAX_BANNER_BYTES);
        self.broker.sink().send(SessionEvent::Banner {
            hop: self.policy.hop.clone(),
            text,
        });
        Ok(())
    }

    async fn disconnected(
        &mut self,
        reason: DisconnectReason<Self::Error>,
    ) -> Result<(), Self::Error> {
        let result = match reason {
            DisconnectReason::ReceivedDisconnect(info) => {
                *self
                    .shared
                    .disconnect_message
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) =
                    Some(truncate_server_text(&info.message, MAX_BANNER_BYTES));
                self.shared.set_loss(DisconnectKind::ServerClosed);
                Ok(())
            }
            DisconnectReason::Error(e) => {
                let kind = match &e {
                    russh::Error::KeepaliveTimeout => DisconnectKind::KeepaliveTimeout,
                    russh::Error::IO(io)
                        if matches!(
                            io.kind(),
                            std::io::ErrorKind::ConnectionReset
                                | std::io::ErrorKind::BrokenPipe
                                | std::io::ErrorKind::UnexpectedEof
                                | std::io::ErrorKind::ConnectionAborted
                        ) =>
                    {
                        DisconnectKind::ConnectionReset
                    }
                    _ => DisconnectKind::NetworkError,
                };
                self.shared.set_loss(kind);
                Err(e)
            }
        };
        self.shared.closed.notify_waiters();
        result
    }

    // §9.5: every server-initiated channel is rejected (russh's defaults would accept).
    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        _channel: Channel<Msg>,
        _connected_address: &str,
        _connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reject_channel!(reply)
    }

    async fn server_channel_open_forwarded_streamlocal(
        &mut self,
        _channel: Channel<Msg>,
        _socket_path: &str,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reject_channel!(reply)
    }

    async fn server_channel_open_agent_forward(
        &mut self,
        _channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reject_channel!(reply)
    }

    async fn server_channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reject_channel!(reply)
    }

    async fn server_channel_open_direct_tcpip(
        &mut self,
        _channel: Channel<Msg>,
        _host_to_connect: &str,
        _port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reject_channel!(reply)
    }

    async fn server_channel_open_direct_streamlocal(
        &mut self,
        _channel: Channel<Msg>,
        _socket_path: &str,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reject_channel!(reply)
    }

    async fn server_channel_open_x11(
        &mut self,
        _channel: Channel<Msg>,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reject_channel!(reply)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use std::time::Duration;

    use base64::Engine as _;

    use super::*;
    use crate::known_hosts::Origin;
    use crate::ssh::prompt::test_support::{RecordingSink, prompt_id};

    /// SSH wire blob for an ed25519 key with 32 bytes of `fill`.
    fn ed_blob(fill: u8) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&11u32.to_be_bytes());
        b.extend_from_slice(b"ssh-ed25519");
        b.extend_from_slice(&32u32.to_be_bytes());
        b.extend_from_slice(&[fill; 32]);
        b
    }

    fn line(host: &str, blob: &[u8]) -> String {
        format!(
            "{host} ssh-ed25519 {}\n",
            base64::engine::general_purpose::STANDARD.encode(blob)
        )
    }

    fn policy(known: &str, strict: StrictMode, learn: Option<PathBuf>) -> HostKeyPolicy {
        let (file, _) = known_hosts::parse(
            std::path::Path::new("/home/alice/.ssh/known_hosts"),
            "~/.ssh/known_hosts",
            Origin::User,
            known.as_bytes(),
        );
        HostKeyPolicy {
            hop: HopRef {
                index: 0,
                label: "h".into(),
                user: "alice".into(),
                host: "example.com".into(),
                port: 22,
            },
            lookup: "example.com".into(),
            files: vec![file].into(),
            learn_target: learn,
            profile_root: None,
            strict,
            hash_known_hosts: false,
            pinned: None,
            approvals: Arc::new(TrustOnceApprovals::default()),
        }
    }

    fn broker() -> (Arc<RecordingSink>, Arc<PromptBroker>) {
        let sink = Arc::new(RecordingSink::default());
        (
            sink.clone(),
            Arc::new(PromptBroker::new(sink, Duration::from_secs(5))),
        )
    }

    /// Answer the first prompt that appears with `answer`.
    fn answer_with(sink: Arc<RecordingSink>, broker: Arc<PromptBroker>, answer: PromptAnswer) {
        tokio::spawn(async move {
            loop {
                let id = sink.events.lock().unwrap().iter().find_map(|e| match e {
                    SessionEvent::Prompt { prompt } => Some(prompt_id(prompt).to_owned()),
                    _ => None,
                });
                if let Some(id) = id {
                    broker.answer(&id, answer).unwrap();
                    return;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        });
    }

    #[tokio::test]
    async fn trusted_and_fatal_verdicts() {
        let (_, b) = broker();
        let key = ed_blob(1);
        let p = policy(&line("example.com", &key), StrictMode::Ask, None);
        let ok = decide(&p, &b, "ssh-ed25519", &key).await.unwrap();
        assert_eq!(ok.verification, HostKeyVerification::KnownHosts);
        assert_eq!(ok.fingerprint, known_hosts::fingerprint_sha256(&key));

        // Changed: same type, different key.
        let err = decide(&p, &b, "ssh-ed25519", &ed_blob(2))
            .await
            .unwrap_err();
        assert!(
            matches!(err.kind, ErrorCode::HostKeyChanged { line: 1, .. }),
            "{err:?}"
        );
        // Revoked wins over a trusted plain entry.
        let p = policy(
            &format!(
                "{}@revoked {}",
                line("example.com", &key),
                line("*", &key).trim_end()
            ),
            StrictMode::No,
            None,
        );
        let err = decide(&p, &b, "ssh-ed25519", &key).await.unwrap_err();
        assert!(
            matches!(err.kind, ErrorCode::HostKeyRevoked { .. }),
            "{err:?}"
        );
        // CA-managed host without a trusting plain entry.
        let p = policy(
            &format!(
                "@cert-authority *.com {}",
                line("", &ed_blob(9)).trim_start()
            ),
            StrictMode::No,
            None,
        );
        let err = decide(&p, &b, "ssh-ed25519", &key).await.unwrap_err();
        assert!(
            matches!(err.kind, ErrorCode::HostKeyCaManaged { .. }),
            "{err:?}"
        );
    }

    /// §10.2: an in-process russh server peer opens session / agent / x11 / forwarded-tcpip
    /// channels towards the client after auth; every one is rejected (§9.5).
    #[tokio::test]
    async fn server_initiated_channels_are_rejected() {
        use russh::keys::PrivateKey;
        use russh::keys::ssh_key::private::Ed25519Keypair;
        use russh::server::{self, Auth};

        struct Peer {
            tx: Option<tokio::sync::oneshot::Sender<server::Handle>>,
        }
        impl server::Handler for Peer {
            type Error = russh::Error;
            async fn auth_none(&mut self, _user: &str) -> Result<Auth, Self::Error> {
                Ok(Auth::Accept)
            }
            async fn auth_succeeded(
                &mut self,
                session: &mut server::Session,
            ) -> Result<(), Self::Error> {
                if let Some(tx) = self.tx.take() {
                    let _ = tx.send(session.handle());
                }
                Ok(())
            }
        }

        let host_key = PrivateKey::from(Ed25519Keypair::from_seed(&[7u8; 32]));
        let cfg = Arc::new(server::Config {
            keys: vec![host_key],
            ..Default::default()
        });
        let (client_io, server_io) = tokio::io::duplex(1 << 16);
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _server = tokio::spawn(server::run_stream(cfg, server_io, Peer { tx: Some(tx) }));

        let (_, b) = broker();
        let handler = ClientHandler {
            policy: policy("", StrictMode::No, None),
            broker: b,
            shared: Arc::new(HopShared::default()),
        };
        let mut client = russh::client::connect_stream(
            Arc::new(russh::client::Config::default()),
            client_io,
            handler,
        )
        .await
        .unwrap();
        let auth = client.authenticate_none("alice").await.unwrap();
        assert!(auth.success());
        let peer = tokio::time::timeout(Duration::from_secs(5), rx)
            .await
            .unwrap()
            .unwrap();

        assert!(
            peer.channel_open_session().await.is_err(),
            "session channel accepted"
        );
        assert!(
            peer.channel_open_agent().await.is_err(),
            "agent channel accepted"
        );
        assert!(
            peer.channel_open_x11("127.0.0.1", 6000).await.is_err(),
            "x11 channel accepted"
        );
        assert!(
            peer.channel_open_forwarded_tcpip("127.0.0.1", 8080, "192.0.2.1", 1234)
                .await
                .is_err(),
            "forwarded-tcpip channel accepted"
        );
        // The connection survives the rejections.
        assert!(!client.is_closed());
    }

    /// `hostKeyChanged` carries a backend-built `removeHint` with the real
    /// known_hosts entry name (here a HostKeyAlias), quoted for the local shell.
    #[tokio::test]
    async fn changed_key_carries_remove_hint_with_lookup_name() {
        let (_, b) = broker();
        let key = ed_blob(1);
        let mut p = policy(&line("it-alias", &key), StrictMode::Ask, None);
        p.lookup = "it-alias".into();
        let err = decide(&p, &b, "ssh-ed25519", &ed_blob(2))
            .await
            .unwrap_err();
        match err.kind {
            ErrorCode::HostKeyChanged {
                remove_hint: Some(hint),
                remove_hint_shell,
                known_hosts_file,
                ..
            } => {
                assert_eq!(remove_hint_shell, HintShell::local());
                assert!(hint.starts_with("ssh-keygen -R 'it-alias' -f '"), "{hint}");
                assert!(hint.contains(&known_hosts_file), "{hint}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn unknown_by_strict_mode() {
        let (_, b) = broker();
        let key = ed_blob(3);
        let err = decide(&policy("", StrictMode::Yes, None), &b, "ssh-ed25519", &key)
            .await
            .unwrap_err();
        assert!(matches!(err.kind, ErrorCode::HostKeyUnknownStrict { .. }));
        let ok = decide(&policy("", StrictMode::No, None), &b, "ssh-ed25519", &key)
            .await
            .unwrap();
        assert_eq!(ok.verification, HostKeyVerification::AutoAccepted);

        // accept-new learns into the target file.
        let dir = tempfile::tempdir().unwrap();
        let kh = dir.path().join("known_hosts");
        let ok = decide(
            &policy("", StrictMode::AcceptNew, Some(kh.clone())),
            &b,
            "ssh-ed25519",
            &key,
        )
        .await
        .unwrap();
        assert_eq!(ok.verification, HostKeyVerification::AutoAccepted);
        assert!(
            std::fs::read_to_string(&kh)
                .unwrap()
                .starts_with("example.com ssh-ed25519 ")
        );
    }

    #[tokio::test]
    async fn ask_prompt_answers() {
        let key = ed_blob(4);
        let dir = tempfile::tempdir().unwrap();
        let kh = dir.path().join("known_hosts");

        // accept + remember ⇒ learned.
        let (sink, b) = broker();
        answer_with(
            sink.clone(),
            b.clone(),
            PromptAnswer::HostKey {
                accept: true,
                remember: true,
            },
        );
        let p = policy("", StrictMode::Ask, Some(kh.clone()));
        let ok = decide(&p, &b, "ssh-ed25519", &key).await.unwrap();
        assert_eq!(ok.verification, HostKeyVerification::UserConfirmed);
        assert!(
            std::fs::read_to_string(&kh)
                .unwrap()
                .contains("ssh-ed25519")
        );
        let prompt = sink.events.lock().unwrap().iter().find_map(|e| match e {
            SessionEvent::Prompt {
                prompt:
                    Prompt::HostKeyUnknown {
                        can_remember,
                        known_hosts_name,
                        ..
                    },
            } => Some((*can_remember, known_hosts_name.clone())),
            _ => None,
        });
        assert_eq!(prompt, Some((true, "example.com".to_owned())));

        // accept once ⇒ trust-once approval, no file write; a re-dial is accepted silently.
        let (sink, b) = broker();
        answer_with(
            sink,
            b.clone(),
            PromptAnswer::HostKey {
                accept: true,
                remember: false,
            },
        );
        let p = policy("", StrictMode::Ask, None);
        decide(&p, &b, "ssh-ed25519", &key).await.unwrap();
        let again = decide(&p, &b, "ssh-ed25519", &key).await.unwrap();
        assert_eq!(again.verification, HostKeyVerification::UserConfirmed);

        // reject ⇒ hostKeyRejected; cancel ⇒ cancelled.
        let (sink, b) = broker();
        answer_with(
            sink,
            b.clone(),
            PromptAnswer::HostKey {
                accept: false,
                remember: false,
            },
        );
        let err = decide(&policy("", StrictMode::Ask, None), &b, "ssh-ed25519", &key)
            .await
            .unwrap_err();
        assert!(matches!(err.kind, ErrorCode::HostKeyRejected { .. }));
        let (sink, b) = broker();
        answer_with(sink, b.clone(), PromptAnswer::Cancel);
        let err = decide(&policy("", StrictMode::Ask, None), &b, "ssh-ed25519", &key)
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorCode::Cancelled);
    }

    #[tokio::test]
    async fn pinned_reconnect_cases() {
        let (_, b) = broker();
        let pinned_key = ed_blob(5);
        let pinned_fp = known_hosts::fingerprint_sha256(&pinned_key);
        let pin = |fp: &str| HostKeyPin {
            lookup: "example.com".into(),
            fingerprint: fp.to_owned(),
            verification: HostKeyVerification::AutoAccepted,
        };
        let mut p = policy("", StrictMode::No, None);
        p.pinned = Some(pin(&pinned_fp));
        // Same key ⇒ accepted, and an auto-accepted pin stays unverified.
        let same = decide(&p, &b, "ssh-ed25519", &pinned_key).await.unwrap();
        assert_eq!(same.verification, HostKeyVerification::AutoAccepted);
        // Different untrusted key ⇒ mismatch (even with StrictHostKeyChecking no).
        let err = decide(&p, &b, "ssh-ed25519", &ed_blob(6))
            .await
            .unwrap_err();
        assert!(
            matches!(err.kind, ErrorCode::HostKeyMismatchOnReconnect { .. }),
            "{err:?}"
        );
        // Different key that known_hosts trusts ⇒ re-pin.
        let other = ed_blob(7);
        let mut p2 = policy(&line("example.com", &other), StrictMode::Ask, None);
        p2.pinned = Some(pin(&pinned_fp));
        assert_eq!(
            decide(&p2, &b, "ssh-ed25519", &other)
                .await
                .unwrap()
                .verification,
            HostKeyVerification::KnownHosts
        );
        // Pinned key now revoked ⇒ revoked (fatal before pin compare).
        let mut p3 = policy(
            &format!("@revoked * {}", line("", &pinned_key).trim_start()),
            StrictMode::No,
            None,
        );
        p3.pinned = Some(pin(&pinned_fp));
        let err = decide(&p3, &b, "ssh-ed25519", &pinned_key)
            .await
            .unwrap_err();
        assert!(
            matches!(err.kind, ErrorCode::HostKeyRevoked { .. }),
            "{err:?}"
        );
    }

    /// `accept-new`/`no` only auto-accept while the user's known_hosts setup is
    /// honoured; `yes`/`ask` are never changed.
    #[test]
    fn effective_strict_mode() {
        use StrictMode::{AcceptNew, Ask, No, Yes};
        for s in [AcceptNew, No] {
            assert_eq!(effective_strict(s, false, true), s, "{s:?}");
            assert_eq!(effective_strict(s, true, true), Ask, "ignored user file");
            assert_eq!(effective_strict(s, false, false), Ask, "nowhere to record");
        }
        for s in [Yes, Ask] {
            assert_eq!(effective_strict(s, true, false), s);
        }
    }

    /// A host known under another key type asks even under `accept-new`/`no`.
    #[test]
    fn other_key_types_force_a_prompt() {
        use StrictMode::{AcceptNew, Ask, No, Yes};
        let other = vec!["ssh-ed25519".to_owned()];
        for s in [AcceptNew, No] {
            assert_eq!(unknown_key_mode(s, &[]), s, "{s:?}");
            assert_eq!(unknown_key_mode(s, &other), Ask, "{s:?}");
        }
        for s in [Yes, Ask] {
            assert_eq!(unknown_key_mode(s, &other), s, "{s:?}");
        }
    }

    #[tokio::test]
    async fn downgrade_to_unknown_type_prompts_under_accept_new() {
        let known = line("example.com", &ed_blob(1));
        let offered = b"not-an-ed25519-blob".to_vec();
        for strict in [StrictMode::AcceptNew, StrictMode::No] {
            let dir = tempfile::tempdir().unwrap();
            let kh = dir.path().join("known_hosts");
            let (sink, b) = broker();
            answer_with(sink.clone(), b.clone(), PromptAnswer::Cancel);
            let p = policy(&known, strict, Some(kh.clone()));
            let err = decide(&p, &b, "ecdsa-sha2-nistp256", &offered)
                .await
                .unwrap_err();
            assert_eq!(err.kind, ErrorCode::Cancelled, "{strict:?}");
            assert!(!kh.exists(), "nothing learned without consent");
            let listed = sink.events.lock().unwrap().iter().find_map(|e| match e {
                SessionEvent::Prompt {
                    prompt:
                        Prompt::HostKeyUnknown {
                            other_key_types_known,
                            ..
                        },
                } => Some(other_key_types_known.clone()),
                _ => None,
            });
            assert_eq!(listed, Some(vec!["ssh-ed25519".to_owned()]));
        }
        // A brand-new host is still auto-accepted.
        let (_, b) = broker();
        let ok = decide(
            &policy("", StrictMode::AcceptNew, None),
            &b,
            "ecdsa-sha2-nistp256",
            &offered,
        )
        .await
        .unwrap();
        assert_eq!(ok.verification, HostKeyVerification::AutoAccepted);
    }

    /// Server texts are cut on a char boundary.
    #[test]
    fn server_text_is_truncated_on_a_char_boundary() {
        assert_eq!(truncate_server_text("abc", 8), "abc");
        let s = "é".repeat(10); // 2 bytes each
        assert_eq!(truncate_server_text(&s, 5), "éé");
        let long = "x".repeat(MAX_BANNER_BYTES * 4);
        assert_eq!(
            truncate_server_text(&long, MAX_BANNER_BYTES).len(),
            MAX_BANNER_BYTES
        );
    }
}
