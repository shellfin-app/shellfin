//! Integration cases (§10.2) against the Docker sshd (`tests/sshd/compose.yml`).
//! Headless: `SessionManager` + an in-memory event sink + a scripted prompt answerer, in a
//! private temp home (own ssh config / known_hosts; no OS keychain; `IdentityAgent none` unless a
//! test uses an agent).
#![cfg(feature = "it")]
// Test code: process spawning (docker, ssh-agent) and unwraps are fine here.
#![allow(
    clippy::disallowed_methods,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use shellfin_lib::error::{AppError, ErrorCode};
use shellfin_lib::ipc::dto::{
    AuthMethodUsed, HostKeyVerification, OpenSessionRequest, Prompt, PromptAnswer,
    PromptClosedReason, RecentTarget, SessionEvent, SessionInfo, SessionState, Target,
};
use shellfin_lib::known_hosts::TrustOnceApprovals;
use shellfin_lib::session::manager::SessionManager;
use shellfin_lib::session::session::{ConnectCtxTemplate, SessionDeps};
use shellfin_lib::ssh::prompt::EventSink;
use shellfin_lib::sshconfig::ConfigEnv;
use shellfin_lib::sshconfig::service::SshConfigService;
use shellfin_lib::storage::Stores;
use tokio::sync::mpsc;

const RUN_TIMEOUT: Duration = Duration::from_secs(90);

struct ChanSink(mpsc::UnboundedSender<SessionEvent>);
impl EventSink for ChanSink {
    fn send(&self, event: SessionEvent) {
        let _ = self.0.send(event);
    }
}

/// A private temp home with its own ssh config and app stores.
struct Env {
    _dir: tempfile::TempDir,
    home: PathBuf,
    stores: Arc<Stores>,
    manager: SessionManager,
}

/// Config path quoting: forward slashes (fine on Windows), quoted for spaces.
fn q(p: &Path) -> String {
    format!("\"{}\"", p.display().to_string().replace('\\', "/"))
}

fn env(config: &str) -> Env {
    let dir = support::private_tempdir();
    let home = dir.path().join("home");
    std::fs::create_dir_all(home.join(".ssh")).unwrap();
    support::write_private_file(&home.join(".ssh"), "config", config.as_bytes(), 0o600);
    let mut cfg_env = ConfigEnv::new(
        home.clone(),
        "alice".into(),
        home.join("etc"),
        home.join("etc/ssh_config"),
    );
    cfg_env.env = Arc::new(|_| None);
    let service = Arc::new(SshConfigService::new(cfg_env.clone()));
    let (stores, _) = Stores::open(&dir.path().join("app-config"));
    let stores = Arc::new(stores);
    let deps = Arc::new(SessionDeps {
        stores: stores.clone(),
        approvals: Arc::new(TrustOnceApprovals::default()),
        ctx_template: ConnectCtxTemplate {
            profile_root: cfg!(windows).then(|| home.clone()),
            home: home.clone(),
            user_ssh_dir: home.join(".ssh"),
            env: cfg_env.env.clone(),
            keychain_available: false,
        },
        recents_changed: Arc::new(|_| {}),
        notifier: Arc::new(shellfin_lib::notify::NoopNotifier),
    });
    Env {
        manager: SessionManager::new(deps, service),
        stores,
        home,
        _dir: dir,
    }
}

/// `Host bastion` block for the published bastion, with extra lines.
fn bastion_block(user: &str, extra: &str) -> String {
    let s = support::sshd();
    format!(
        "Host bastion\n  HostName {}\n  Port {}\n  User {user}\n{extra}\nHost *\n  IdentityAgent none\n",
        s.host, s.port
    )
}

fn prompt_id(p: &Prompt) -> &str {
    match p {
        Prompt::HostKeyUnknown { id, .. }
        | Prompt::Passphrase { id, .. }
        | Prompt::Password { id, .. }
        | Prompt::KeyboardInteractive { id, .. } => id,
    }
}

struct Run {
    session_id: String,
    events: Vec<SessionEvent>,
    result: Result<SessionInfo, AppError>,
}

impl Run {
    fn info(&self) -> &SessionInfo {
        self.result
            .as_ref()
            .unwrap_or_else(|e| panic!("not connected: {e:?}\nevents: {:#?}", self.events))
    }

    fn error(&self) -> &AppError {
        match &self.result {
            Err(e) => e,
            Ok(_) => panic!("unexpectedly connected; events: {:#?}", self.events),
        }
    }
}

/// Open `target` and drive prompts with `answer` until `connected` or `disconnected`.
async fn run(
    e: &Env,
    target: Target,
    probe_only: bool,
    mut answer: impl FnMut(&Prompt) -> PromptAnswer,
) -> Run {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let session_id = e
        .manager
        .open(
            OpenSessionRequest { target, probe_only },
            Arc::new(ChanSink(tx)),
        )
        .await
        .expect("session_open failed");
    let mut events = Vec::new();
    let deadline = tokio::time::Instant::now() + RUN_TIMEOUT;
    loop {
        let ev = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .unwrap_or_else(|_| panic!("timed out; events: {events:#?}"))
            .expect("event channel closed");
        match &ev {
            SessionEvent::Prompt { prompt } => {
                let a = answer(prompt);
                e.manager
                    .answer_prompt(&session_id, prompt_id(prompt), a)
                    .unwrap();
            }
            SessionEvent::State {
                state: SessionState::Connected { info },
            } => {
                let info = info.clone();
                events.push(ev);
                return Run {
                    session_id,
                    events,
                    result: Ok(info),
                };
            }
            SessionEvent::State {
                state: SessionState::Disconnected { error, .. },
            } => {
                let err = error
                    .clone()
                    .unwrap_or_else(|| AppError::internal("disconnected without error"));
                events.push(ev);
                return Run {
                    session_id,
                    events,
                    result: Err(err),
                };
            }
            _ => {}
        }
        events.push(ev);
    }
}

fn accept_once(p: &Prompt) -> Option<PromptAnswer> {
    matches!(p, Prompt::HostKeyUnknown { .. }).then_some(PromptAnswer::HostKey {
        accept: true,
        remember: false,
    })
}

fn known_hosts(e: &Env) -> PathBuf {
    e.home.join(".ssh/known_hosts")
}

fn bastion_lookup() -> String {
    let s = support::sshd();
    if s.port == 22 {
        s.host.clone()
    } else {
        format!("[{}]:{}", s.host, s.port)
    }
}

fn random_ed25519_line(host: &str) -> String {
    use base64::Engine as _;
    let mut blob = Vec::new();
    blob.extend_from_slice(&11u32.to_be_bytes());
    blob.extend_from_slice(b"ssh-ed25519");
    blob.extend_from_slice(&32u32.to_be_bytes());
    blob.extend_from_slice(&[0x5a; 32]);
    format!(
        "{host} ssh-ed25519 {}\n",
        base64::engine::general_purpose::STANDARD.encode(blob)
    )
}

fn sshconfig(alias: &str) -> Target {
    Target::SshConfig {
        alias: alias.into(),
    }
}

// ---------------------------------------------------------------------------------------------
// host keys
// ---------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn host_key_unknown_learn_then_known_and_close_records_recents() {
    let (_kd, key) = support::private_key_copy(support::KEY_RSA_OPENSSH);
    let e = env(&bastion_block(
        support::USER,
        &format!("  IdentityFile {}\n  IdentitiesOnly yes", q(&key)),
    ));

    let first = run(&e, sshconfig("bastion"), false, |p| match p {
        Prompt::HostKeyUnknown {
            can_remember,
            known_hosts_name,
            ..
        } => {
            assert!(*can_remember);
            assert_eq!(*known_hosts_name, bastion_lookup());
            PromptAnswer::HostKey {
                accept: true,
                remember: true,
            }
        }
        other => panic!("unexpected prompt {other:?}"),
    })
    .await;
    let info = first.info().clone();
    assert_eq!(
        info.hops[0].host_key.verification,
        HostKeyVerification::UserConfirmed
    );
    assert_eq!(info.auth.method, AuthMethodUsed::Publickey);
    assert_eq!(info.auth.key_type.as_deref(), Some("ssh-rsa"));
    assert!(info.sftp_available, "SFTP should init eagerly");
    assert_eq!(info.home_dir.as_deref(), Some("/home/tester"));
    assert!(
        first.events.iter().any(
            |ev| matches!(ev, SessionEvent::Banner { text, .. } if text.contains(support::BANNER))
        ),
        "banner event missing"
    );
    let kh = std::fs::read_to_string(known_hosts(&e)).unwrap();
    assert!(kh.starts_with(&format!("{} ", bastion_lookup())), "{kh}");

    e.manager
        .close(&first.session_id, Some("/srv".into()))
        .await;
    let recents = e.stores.lock_recents().list();
    assert_eq!(recents.len(), 1);
    assert_eq!(
        recents[0].target,
        RecentTarget::SshConfig {
            alias: "bastion".into()
        }
    );
    assert_eq!(recents[0].last_dir.as_deref(), Some("/srv"));

    // Known now: no prompt, verified by known_hosts.
    let second = run(&e, sshconfig("bastion"), false, |p| {
        panic!("unexpected prompt {p:?}")
    })
    .await;
    assert_eq!(
        second.info().hops[0].host_key.verification,
        HostKeyVerification::KnownHosts
    );
    e.manager.close(&second.session_id, None).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn host_key_changed_is_fatal() {
    let e = env(&bastion_block(support::USER, ""));
    std::fs::write(known_hosts(&e), random_ed25519_line(&bastion_lookup())).unwrap();
    let r = run(&e, sshconfig("bastion"), false, |p| {
        panic!("unexpected prompt {p:?}")
    })
    .await;
    assert!(
        matches!(&r.error().kind, ErrorCode::HostKeyChanged { line: 1, key_type, .. } if key_type == "ssh-ed25519"),
        "{:?}",
        r.error()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn host_key_ca_managed_and_strict_and_rejected() {
    let e = env(&bastion_block(support::USER, ""));
    let line = random_ed25519_line("x");
    let key_part = line.trim_start_matches("x ");
    std::fs::write(
        known_hosts(&e),
        format!("@cert-authority {} {key_part}", bastion_lookup()),
    )
    .unwrap();
    let r = run(&e, sshconfig("bastion"), false, |p| {
        panic!("unexpected prompt {p:?}")
    })
    .await;
    assert!(
        matches!(r.error().kind, ErrorCode::HostKeyCaManaged { .. }),
        "{:?}",
        r.error()
    );

    let e = env(&bastion_block(support::USER, "  StrictHostKeyChecking yes"));
    let r = run(&e, sshconfig("bastion"), false, |p| {
        panic!("unexpected prompt {p:?}")
    })
    .await;
    assert!(
        matches!(r.error().kind, ErrorCode::HostKeyUnknownStrict { .. }),
        "{:?}",
        r.error()
    );

    let e = env(&bastion_block(support::USER, ""));
    let r = run(&e, sshconfig("bastion"), false, |_| PromptAnswer::HostKey {
        accept: false,
        remember: false,
    })
    .await;
    assert!(
        matches!(r.error().kind, ErrorCode::HostKeyRejected { .. }),
        "{:?}",
        r.error()
    );
    assert!(
        !known_hosts(&e).exists(),
        "a rejected key must not be learned"
    );
}

/// `accept-new` asks instead of auto-accepting when the user's
/// known_hosts was ignored (here: over the 4 MiB cap, reported as `knownHostsIgnored`) or when
/// the host is recorded under another key type only (possible algorithm downgrade).
#[tokio::test(flavor = "multi_thread")]
async fn accept_new_asks_for_ignored_files_and_other_key_types() {
    let cancel = |p: &Prompt| match p {
        Prompt::HostKeyUnknown { .. } => PromptAnswer::Cancel,
        other => panic!("unexpected prompt {other:?}"),
    };

    let e = env(&bastion_block(
        support::USER,
        "  StrictHostKeyChecking accept-new",
    ));
    support::write_private_file(
        &e.home.join(".ssh"),
        "known_hosts",
        &vec![b'#'; 5 * 1024 * 1024],
        0o600,
    );
    let r = run(&e, sshconfig("bastion"), false, cancel).await;
    assert_eq!(
        r.error().kind,
        ErrorCode::Cancelled,
        "asked, not auto-accepted"
    );
    let ignored = r.events.iter().any(|ev| {
        matches!(ev, SessionEvent::Warning { warning }
            if matches!(warning.kind, ErrorCode::KnownHostsIgnored { .. }))
    });
    assert!(ignored, "events: {:#?}", r.events);

    // Known only under a key type the server does not offer.
    use base64::Engine as _;
    let mut blob = Vec::new();
    blob.extend_from_slice(&7u32.to_be_bytes());
    blob.extend_from_slice(b"ssh-dss");
    blob.extend_from_slice(&[1u8; 40]);
    let e = env(&bastion_block(
        support::USER,
        "  StrictHostKeyChecking accept-new",
    ));
    let line = format!(
        "{} ssh-dss {}\n",
        bastion_lookup(),
        base64::engine::general_purpose::STANDARD.encode(blob)
    );
    std::fs::write(known_hosts(&e), &line).unwrap();
    let mut listed = Vec::new();
    let r = run(&e, sshconfig("bastion"), false, |p| {
        if let Prompt::HostKeyUnknown {
            other_key_types_known,
            ..
        } = p
        {
            listed.clone_from(other_key_types_known);
        }
        cancel(p)
    })
    .await;
    assert_eq!(r.error().kind, ErrorCode::Cancelled);
    assert_eq!(listed, vec!["ssh-dss".to_owned()]);
    assert_eq!(
        std::fs::read_to_string(known_hosts(&e)).unwrap(),
        line,
        "nothing learned without consent"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn host_key_alias_learned_as_plain_alias() {
    let (_kd, key) = support::private_key_copy(support::KEY_RSA_OPENSSH);
    let e = env(&bastion_block(
        support::USER,
        &format!(
            "  HostKeyAlias it-alias\n  IdentityFile {}\n  IdentitiesOnly yes",
            q(&key)
        ),
    ));
    let r = run(&e, sshconfig("bastion"), false, |p| match p {
        Prompt::HostKeyUnknown {
            known_hosts_name, ..
        } => {
            assert_eq!(known_hosts_name, "it-alias");
            PromptAnswer::HostKey {
                accept: true,
                remember: true,
            }
        }
        other => panic!("unexpected prompt {other:?}"),
    })
    .await;
    r.info();
    e.manager.close(&r.session_id, None).await;
    let kh = std::fs::read(known_hosts(&e)).unwrap();
    assert!(String::from_utf8_lossy(&kh).starts_with("it-alias ssh-"));
    // OpenSSH agrees: `ssh-keygen -F it-alias` finds the learned line.
    let out = support::docker_exec_stdin_ok(
        support::BASTION,
        &[
            "sh",
            "-c",
            "cat > /tmp/kh-alias && ssh-keygen -F it-alias -f /tmp/kh-alias",
        ],
        &kh,
    );
    assert!(out.contains("it-alias"), "{out}");
}

// ---------------------------------------------------------------------------------------------
// authentication
// ---------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn password_wrong_then_right() {
    let e = env(&bastion_block(support::USER, "  PubkeyAuthentication no"));
    let mut tries = 0;
    let r = run(&e, sshconfig("bastion"), false, |p| {
        accept_once(p).unwrap_or_else(|| match p {
            Prompt::Password {
                attempts_left,
                last_error,
                ..
            } => {
                tries += 1;
                if tries == 1 {
                    assert_eq!(*attempts_left, 3);
                    PromptAnswer::Password {
                        password: secret("wrong-password"),
                        remember: false,
                    }
                } else {
                    assert!(last_error.is_some());
                    PromptAnswer::Password {
                        password: secret(support::USER_PASSWORD),
                        remember: false,
                    }
                }
            }
            other => panic!("unexpected prompt {other:?}"),
        })
    })
    .await;
    let info = r.info();
    assert!(matches!(
        info.auth.method,
        AuthMethodUsed::Password | AuthMethodUsed::KeyboardInteractive
    ));
    assert_eq!(tries, 2);
    e.manager.close(&r.session_id, None).await;
}

/// Secrets are inbound-only (`Deserialize`), so tests build them the way the frontend does.
fn secret(s: &str) -> shellfin_lib::ipc::secret::Secret<String> {
    serde_json::from_value(serde_json::Value::String(s.into())).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn encrypted_key_wrong_then_right_and_skip() {
    let (_kd, key) = support::private_key_copy(support::KEY_ED25519_ENC);
    let cfg = format!("  IdentityFile {}\n  IdentitiesOnly yes", q(&key));

    let e = env(&bastion_block(support::USER, &cfg));
    let mut asked = 0;
    let r = run(&e, sshconfig("bastion"), false, |p| {
        accept_once(p).unwrap_or_else(|| match p {
            Prompt::Passphrase { attempts_left, .. } => {
                asked += 1;
                if asked == 1 {
                    assert_eq!(*attempts_left, 3);
                    PromptAnswer::Passphrase {
                        passphrase: secret("not-it"),
                        add_to_agent: false,
                    }
                } else {
                    PromptAnswer::Passphrase {
                        passphrase: secret(support::KEY_PASSPHRASE),
                        add_to_agent: false,
                    }
                }
            }
            other => panic!("unexpected prompt {other:?}"),
        })
    })
    .await;
    assert_eq!(r.info().auth.key_type.as_deref(), Some("ssh-ed25519"));
    e.manager.close(&r.session_id, None).await;

    // Skip the key ⇒ auth continues with the password.
    let e = env(&bastion_block(support::USER, &cfg));
    let r = run(&e, sshconfig("bastion"), false, |p| {
        accept_once(p).unwrap_or_else(|| match p {
            Prompt::Passphrase { .. } => PromptAnswer::Skip,
            Prompt::Password { .. } => PromptAnswer::Password {
                password: secret(support::USER_PASSWORD),
                remember: false,
            },
            other => panic!("unexpected prompt {other:?}"),
        })
    })
    .await;
    assert_ne!(r.info().auth.method, AuthMethodUsed::Publickey);
    e.manager.close(&r.session_id, None).await;

    // Cancel ⇒ the connection is aborted with `cancelled`.
    let e = env(&bastion_block(support::USER, &cfg));
    let r = run(&e, sshconfig("bastion"), false, |p| {
        accept_once(p).unwrap_or(PromptAnswer::Cancel)
    })
    .await;
    assert_eq!(r.error().kind, ErrorCode::Cancelled);
}

#[tokio::test(flavor = "multi_thread")]
async fn rsa_sha2_with_openssh_and_pem_keys() {
    for name in [support::KEY_RSA_OPENSSH, support::KEY_RSA_PEM] {
        let (_kd, key) = support::private_key_copy(name);
        let e = env(&bastion_block(
            support::USER,
            &format!(
                "  IdentityFile {}\n  IdentitiesOnly yes\n  PasswordAuthentication no\n  KbdInteractiveAuthentication no",
                q(&key)
            ),
        ));
        let r = run(&e, sshconfig("bastion"), false, |p| {
            accept_once(p).unwrap_or_else(|| panic!("{p:?}"))
        })
        .await;
        let info = r.info();
        assert_eq!(info.auth.method, AuthMethodUsed::Publickey, "{name}");
        assert_eq!(info.auth.key_type.as_deref(), Some("ssh-rsa"), "{name}");
        e.manager.close(&r.session_id, None).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn two_factor_publickey_password_and_totp() {
    let (_kd, key) = support::private_key_copy(support::KEY_ED25519);
    let e = env(&bastion_block(
        support::USER_2FA,
        &format!("  IdentityFile {}\n  IdentitiesOnly yes", q(&key)),
    ));
    let mut saw_code = false;
    let r = run(&e, sshconfig("bastion"), false, |p| {
        accept_once(p).unwrap_or_else(|| match p {
            Prompt::Password { .. } => PromptAnswer::Password {
                password: secret(support::USER_2FA_PASSWORD),
                remember: false,
            },
            Prompt::KeyboardInteractive { fields, .. } => {
                // "Verification code:" is never treated as a password prompt.
                assert_eq!(fields.len(), 1, "{fields:?}");
                assert!(
                    fields[0]
                        .label
                        .to_ascii_lowercase()
                        .contains("verification"),
                    "{fields:?}"
                );
                saw_code = true;
                PromptAnswer::KeyboardInteractive {
                    responses: vec![secret(&support::totp_now(support::TOTP_SECRET))],
                }
            }
            other => panic!("unexpected prompt {other:?}"),
        })
    })
    .await;
    r.info();
    assert!(saw_code);
    e.manager.close(&r.session_id, None).await;
}

/// Server `MaxAuthTries 3` < our budget: the server disconnects after 3 publickey failures, so
/// the session re-dials once without publickey and reaches the password (§5.6).
#[tokio::test(flavor = "multi_thread")]
async fn low_max_auth_tries_redials_without_publickey() {
    let mut dirs = Vec::new();
    let mut lines = String::new();
    for name in &support::AGENT_EXTRA_KEYS[..4] {
        let (d, key) = support::private_key_copy(name);
        lines.push_str(&format!("  IdentityFile {}\n", q(&key)));
        dirs.push(d);
    }
    lines.push_str("  IdentitiesOnly yes");
    let e = env(&bastion_block(support::USER_LOW_TRIES, &lines));
    let mut host_key_prompts = 0;
    let r = run(&e, sshconfig("bastion"), false, |p| match p {
        Prompt::HostKeyUnknown { .. } => {
            host_key_prompts += 1;
            PromptAnswer::HostKey {
                accept: true,
                remember: false,
            }
        }
        Prompt::Password { .. } => PromptAnswer::Password {
            password: secret(support::USER_LOW_TRIES_PASSWORD),
            remember: false,
        },
        other => panic!("unexpected prompt {other:?}"),
    })
    .await;
    assert_ne!(r.info().auth.method, AuthMethodUsed::Publickey);
    assert_eq!(
        host_key_prompts, 1,
        "the re-dial must reuse the pinned host key"
    );
    e.manager.close(&r.session_id, None).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn proxyjump_bastion_to_target_and_probe_mode() {
    let s = support::sshd();
    let (_kd, key) = support::private_key_copy(support::KEY_RSA_OPENSSH);
    let id = format!("  IdentityFile {}\n  IdentitiesOnly yes\n", q(&key));
    let cfg = format!(
        "Host bastion\n  HostName {}\n  Port {}\n  User tester\n{id}\nHost target\n  HostName {}\n  Port {}\n  User tester\n  ProxyJump bastion\n{id}\nHost *\n  IdentityAgent none\n",
        s.host, s.port, s.target_host, s.target_port
    );
    let e = env(&cfg);
    let r = run(&e, sshconfig("target"), false, |p| {
        accept_once(p).unwrap_or_else(|| panic!("{p:?}"))
    })
    .await;
    let info = r.info();
    let labels: Vec<&str> = info.hops.iter().map(|h| h.hop.label.as_str()).collect();
    assert_eq!(labels, ["bastion", "target"]);
    assert_eq!(info.hops[1].hop.host, s.target_host);
    assert!(info.sftp_available);
    e.manager.close(&r.session_id, None).await;

    // Probe ("Test connection"): connected, then closed immediately, no recents entry.
    let e = env(&cfg);
    let (tx, mut rx) = mpsc::unbounded_channel();
    e.manager
        .open(
            OpenSessionRequest {
                target: sshconfig("target"),
                probe_only: true,
            },
            Arc::new(ChanSink(tx)),
        )
        .await
        .unwrap();
    let mut connected = false;
    let id_for_answers = e.manager.ids();
    loop {
        let ev = tokio::time::timeout(RUN_TIMEOUT, rx.recv())
            .await
            .unwrap()
            .unwrap();
        match ev {
            SessionEvent::Prompt { prompt } => {
                let a = accept_once(&prompt).unwrap();
                e.manager
                    .answer_prompt(&id_for_answers[0], prompt_id(&prompt), a)
                    .unwrap();
            }
            SessionEvent::State {
                state: SessionState::Connected { .. },
            } => connected = true,
            SessionEvent::State {
                state: SessionState::Closed,
            } => break,
            SessionEvent::State {
                state: SessionState::Disconnected { error, .. },
            } => panic!("{error:?}"),
            _ => {}
        }
    }
    assert!(connected);
    assert!(e.stores.lock_recents().list().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn resolution_errors_reject_session_open() {
    let e = env("Host loop1\n  ProxyJump loop2\nHost loop2\n  ProxyJump loop1\n");
    let (tx, _rx) = mpsc::unbounded_channel();
    let err = e
        .manager
        .open(
            OpenSessionRequest {
                target: sshconfig("loop1"),
                probe_only: false,
            },
            Arc::new(ChanSink(tx.clone())),
        )
        .await
        .unwrap_err();
    assert!(matches!(err.kind, ErrorCode::ProxyJumpCycle { .. }));
    let err = e
        .manager
        .open(
            OpenSessionRequest {
                target: Target::Quick {
                    spec: "bad host".into(),
                },
                probe_only: false,
            },
            Arc::new(ChanSink(tx)),
        )
        .await
        .unwrap_err();
    assert!(matches!(err.kind, ErrorCode::InvalidHostSpec { .. }));
}

// ---------------------------------------------------------------------------------------------
// agents
// ---------------------------------------------------------------------------------------------

/// RSA key via an owner-verified Unix `ssh-agent` (RSA SHA-2 signing through the agent), and the
/// budget case: 8 non-matching agent keys + password still reaches the password (§10.2).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn agent_rsa_and_budget_reach_password() {
    use std::process::{Child, Command, Stdio};

    /// Kills (and reaps) the spawned agent on drop — also when the test panics — so it can never
    /// outlive the test or hold the test binary's stdout/stderr open.
    struct AgentGuard(Child);
    impl Drop for AgentGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    let agent_dir = support::private_tempdir();
    let sock = agent_dir.path().join("agent.sock");
    let _agent = AgentGuard(
        Command::new("ssh-agent")
            .args(["-D", "-a"])
            .arg(&sock)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("ssh-agent"),
    );
    for _ in 0..50 {
        if sock.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let ssh_add = |args: &[&std::ffi::OsStr]| {
        Command::new("ssh-add")
            .args(args)
            .env("SSH_AUTH_SOCK", &sock)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
    };
    let add = |name: &str| {
        let (_d, key) = support::private_key_copy(name);
        assert!(ssh_add(&[key.as_os_str()]).success(), "ssh-add {name}");
    };

    // 1. RSA key held only by the agent.
    add(support::KEY_RSA_OPENSSH);
    let e = env(&format!(
        "Host bastion\n  HostName {}\n  Port {}\n  User tester\n  IdentityAgent {}\n  PasswordAuthentication no\n  KbdInteractiveAuthentication no\n",
        support::sshd().host,
        support::sshd().port,
        q(&sock)
    ));
    let r = run(&e, sshconfig("bastion"), false, |p| {
        accept_once(p).unwrap_or_else(|| panic!("{p:?}"))
    })
    .await;
    let info = r.info();
    assert_eq!(info.auth.method, AuthMethodUsed::Agent);
    assert_eq!(info.auth.key_type.as_deref(), Some("ssh-rsa"));
    e.manager.close(&r.session_id, None).await;

    // 2. Budget: 8 unauthorized agent keys, then the password.
    assert!(ssh_add(&[std::ffi::OsStr::new("-D")]).success());
    for name in support::AGENT_EXTRA_KEYS {
        add(name);
    }
    let e = env(&format!(
        "Host bastion\n  HostName {}\n  Port {}\n  User tester\n  IdentityAgent {}\n",
        support::sshd().host,
        support::sshd().port,
        q(&sock)
    ));
    let r = run(&e, sshconfig("bastion"), false, |p| {
        accept_once(p).unwrap_or_else(|| match p {
            Prompt::Password { .. } => PromptAnswer::Password {
                password: secret(support::USER_PASSWORD),
                remember: false,
            },
            other => panic!("unexpected prompt {other:?}"),
        })
    })
    .await;
    assert_ne!(r.info().auth.method, AuthMethodUsed::Agent);
    e.manager.close(&r.session_id, None).await;
}

// ----- shared Unix agent helpers (kill-on-drop, no inherited stdio) ---------------------------

#[cfg(unix)]
mod unix_agent {
    use std::path::Path;
    use std::process::{Child, Command, Stdio};
    use std::time::Duration;

    /// Kills and reaps the agent on drop (also on panic); never holds the test's stdout.
    pub struct AgentGuard(pub Child);
    impl Drop for AgentGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn quiet(cmd: &mut Command) -> &mut Command {
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
    }

    /// `ssh-agent -D -a <sock>`, optionally as another (unprivileged) user. `None` if that is
    /// not possible here (not root and no passwordless sudo).
    pub fn spawn_agent(sock: &Path, as_user: Option<&str>) -> Option<AgentGuard> {
        let mut cmd = match as_user {
            None => Command::new("ssh-agent"),
            // Running as root: drop to the other user directly.
            Some(user) if unsafe { libc::geteuid() } == 0 => {
                let mut c = Command::new("setpriv");
                c.args([
                    "--reuid",
                    user,
                    "--regid",
                    "nogroup",
                    "--clear-groups",
                    "ssh-agent",
                ]);
                c
            }
            Some(user) => {
                let ok = quiet(Command::new("sudo").args(["-n", "true"]))
                    .status()
                    .is_ok_and(|s| s.success());
                if !ok {
                    return None;
                }
                let mut c = Command::new("sudo");
                c.args(["-n", "-u", user, "ssh-agent"]);
                c
            }
        };
        cmd.args(["-D", "-a"]).arg(sock);
        let child = quiet(&mut cmd).spawn().ok()?;
        let guard = AgentGuard(child);
        for _ in 0..50 {
            if sock.exists() {
                // ssh-agent creates its socket 0600. For another user's agent, open it up so the connect succeeds
                // and the owner check (not plain EACCES) is what rejects it.
                if let Some(user) = as_user {
                    let opened = if unsafe { libc::geteuid() } == 0 {
                        std::fs::set_permissions(
                            sock,
                            std::os::unix::fs::PermissionsExt::from_mode(0o666),
                        )
                        .is_ok()
                    } else {
                        quiet(
                            Command::new("sudo")
                                .args(["-n", "-u", user, "chmod", "666"])
                                .arg(sock),
                        )
                        .status()
                        .is_ok_and(|s| s.success())
                    };
                    if !opened {
                        return None;
                    }
                }
                return Some(guard);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        None
    }

    pub fn ssh_add(sock: &Path, args: &[&std::ffi::OsStr]) -> std::process::Output {
        Command::new("ssh-add")
            .args(args)
            .env("SSH_AUTH_SOCK", sock)
            .stdin(Stdio::null())
            .output()
            .expect("ssh-add")
    }
}

/// May the owner-check IT skip when it cannot switch users? Only on a local run:
/// never when `CI` is set (GitHub sets `CI=true`) or `SHELLFIN_IT_REQUIRE_OWNER_TEST` is set
/// (to anything but empty/`0`/`false`).
fn owner_test_required(ci: Option<&str>, require: Option<&str>) -> bool {
    let set = |v: Option<&str>| {
        v.is_some_and(|v| {
            let v = v.trim();
            !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false")
        })
    };
    set(ci) || set(require)
}

#[test]
fn owner_test_is_required_in_ci() {
    assert!(owner_test_required(Some("true"), None));
    assert!(owner_test_required(Some("1"), None));
    assert!(owner_test_required(None, Some("1")));
    assert!(owner_test_required(Some("false"), Some("1")));
    assert!(!owner_test_required(None, None));
    assert!(!owner_test_required(Some(""), Some("0")));
    assert!(!owner_test_required(Some("false"), None));
}

/// §10.2: an agent socket owned by another uid is rejected (`agentUntrusted`
/// warning) and authentication continues without it (here: password).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn agent_socket_of_another_user_is_untrusted() {
    use shellfin_lib::ssh::agent::{self, AgentEndpoint, AgentError};

    // A world-writable directory so the other user can create its socket there.
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(
        dir.path(),
        std::os::unix::fs::PermissionsExt::from_mode(0o777),
    )
    .unwrap();
    let sock = dir.path().join("foreign.sock");
    let Some(_agent) = unix_agent::spawn_agent(&sock, Some("nobody")) else {
        let why = "cannot run an agent as another user here (needs root or passwordless sudo)";
        // In CI (or when required) the §10.2 evidence must not vanish silently.
        assert!(
            !owner_test_required(
                std::env::var("CI").ok().as_deref(),
                std::env::var("SHELLFIN_IT_REQUIRE_OWNER_TEST")
                    .ok()
                    .as_deref()
            ),
            "{why}; this test is required in CI"
        );
        eprintln!("skipping (local run only): {why}");
        return;
    };
    let err = agent::connect_verified(&AgentEndpoint::Socket(sock.clone()))
        .await
        .err();
    assert!(matches!(err, Some(AgentError::Untrusted(_))), "{err:?}");

    let e = env(&format!(
        "Host bastion\n  HostName {}\n  Port {}\n  User tester\n  IdentityAgent {}\n",
        support::sshd().host,
        support::sshd().port,
        q(&sock)
    ));
    let r = run(&e, sshconfig("bastion"), false, |p| {
        accept_once(p).unwrap_or_else(|| match p {
            Prompt::Password { .. } => PromptAnswer::Password {
                password: secret(support::USER_PASSWORD),
                remember: false,
            },
            other => panic!("unexpected prompt {other:?}"),
        })
    })
    .await;
    r.info();
    assert!(
        r.events.iter().any(|ev| matches!(ev, SessionEvent::Warning { warning } if warning.kind == ErrorCode::AgentUntrusted)),
        "agentUntrusted warning missing: {:#?}",
        r.events
    );
    e.manager.close(&r.session_id, None).await;
}

/// §10.2: "Add to ssh-agent" adds the decrypted key to the Unix `ssh-agent` with the
/// KeyLifetime constraint (accepted there), and the key is gone after `session_close`.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn agent_add_with_lifetime_and_removed_on_close() {
    let dir = support::private_tempdir();
    let sock = dir.path().join("agent.sock");
    let _agent = unix_agent::spawn_agent(&sock, None).expect("ssh-agent");
    let (_kd, key) = support::private_key_copy(support::KEY_ED25519_ENC);
    let e = env(&format!(
        "Host bastion\n  HostName {}\n  Port {}\n  User tester\n  IdentityFile {}\n  IdentitiesOnly yes\n  IdentityAgent {}\n  PasswordAuthentication no\n  KbdInteractiveAuthentication no\n",
        support::sshd().host,
        support::sshd().port,
        q(&key),
        q(&sock)
    ));
    let r = run(&e, sshconfig("bastion"), false, |p| {
        accept_once(p).unwrap_or_else(|| match p {
            Prompt::Passphrase {
                add_to_agent_supported,
                ..
            } => {
                assert!(*add_to_agent_supported);
                PromptAnswer::Passphrase {
                    passphrase: secret(support::KEY_PASSPHRASE),
                    add_to_agent: true,
                }
            }
            other => panic!("unexpected prompt {other:?}"),
        })
    })
    .await;
    assert_eq!(r.info().auth.method, AuthMethodUsed::Publickey);
    let listed = unix_agent::ssh_add(&sock, &[std::ffi::OsStr::new("-l")]);
    assert!(
        String::from_utf8_lossy(&listed.stdout).contains("ED25519"),
        "key not in the agent"
    );
    assert!(
        !r.events
            .iter()
            .any(|ev| matches!(ev, SessionEvent::Warning { .. })),
        "the Unix agent accepts KeyLifetime: no agentConstraintUnsupported warning"
    );
    e.manager.close(&r.session_id, None).await;
    let listed = unix_agent::ssh_add(&sock, &[std::ffi::OsStr::new("-l")]);
    assert!(
        !String::from_utf8_lossy(&listed.stdout).contains("ED25519"),
        "the session's key must be removed on close"
    );
}

/// A wrong verification code re-prompts the CODE; the password is asked only once.
#[tokio::test(flavor = "multi_thread")]
async fn two_factor_wrong_code_is_retried_without_reasking_password() {
    let (_kd, key) = support::private_key_copy(support::KEY_ED25519);
    let e = env(&bastion_block(
        support::USER_2FA,
        &format!("  IdentityFile {}\n  IdentitiesOnly yes", q(&key)),
    ));
    let (mut password_prompts, mut code_prompts) = (0, 0);
    let r = run(&e, sshconfig("bastion"), false, |p| {
        accept_once(p).unwrap_or_else(|| match p {
            Prompt::Password { last_error, .. } => {
                password_prompts += 1;
                assert!(
                    last_error.is_none(),
                    "a wrong code must not be reported as a wrong password"
                );
                PromptAnswer::Password {
                    password: secret(support::USER_2FA_PASSWORD),
                    remember: false,
                }
            }
            Prompt::KeyboardInteractive { .. } => {
                code_prompts += 1;
                let code = if code_prompts == 1 {
                    "000000".to_owned()
                } else {
                    support::totp_now(support::TOTP_SECRET)
                };
                PromptAnswer::KeyboardInteractive {
                    responses: vec![secret(&code)],
                }
            }
            other => panic!("unexpected prompt {other:?}"),
        })
    })
    .await;
    r.info();
    assert_eq!(password_prompts, 1);
    assert_eq!(code_prompts, 2);
    e.manager.close(&r.session_id, None).await;
}

/// / §5.4: the connection dies while the password prompt is open ⇒ the prompt is
/// withdrawn with `promptClosed{connectionLost}` (not after 110 s), the session re-dials ONCE
/// (same pinned host key, no second host-key prompt) and asks again; never
/// `tooManyAuthFailures`.
#[tokio::test(flavor = "multi_thread")]
async fn connection_lost_during_prompt_redials_once() {
    let e = env(&bastion_block(
        support::USER_TCSH,
        "  PubkeyAuthentication no\n  KbdInteractiveAuthentication no",
    ));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let session_id = e
        .manager
        .open(
            OpenSessionRequest {
                target: sshconfig("bastion"),
                probe_only: false,
            },
            Arc::new(ChanSink(tx)),
        )
        .await
        .unwrap();
    let (mut password_prompts, mut host_key_prompts, mut lost_closed) = (0, 0, 0);
    let deadline = tokio::time::Instant::now() + RUN_TIMEOUT;
    let info = loop {
        let ev = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("timed out")
            .unwrap();
        match ev {
            SessionEvent::Prompt { prompt } => match &prompt {
                Prompt::HostKeyUnknown { .. } => {
                    host_key_prompts += 1;
                    e.manager
                        .answer_prompt(
                            &session_id,
                            prompt_id(&prompt),
                            PromptAnswer::HostKey {
                                accept: true,
                                remember: false,
                            },
                        )
                        .unwrap();
                }
                Prompt::Password { .. } => {
                    password_prompts += 1;
                    if password_prompts == 1 {
                        // Kill this user's sshd processes: the connection drops mid-prompt.
                        let _ = support::docker_exec(
                            support::BASTION,
                            &["pkill", "-f", "sshd: tester-tcsh"],
                        );
                    } else {
                        e.manager
                            .answer_prompt(
                                &session_id,
                                prompt_id(&prompt),
                                PromptAnswer::Password {
                                    password: secret(support::USER_TCSH_PASSWORD),
                                    remember: false,
                                },
                            )
                            .unwrap();
                    }
                }
                other => panic!("unexpected prompt {other:?}"),
            },
            SessionEvent::PromptClosed {
                reason: PromptClosedReason::ConnectionLost,
                ..
            } => lost_closed += 1,
            SessionEvent::State {
                state: SessionState::Connected { info },
            } => break info,
            SessionEvent::State {
                state: SessionState::Disconnected { error, .. },
            } => panic!("{error:?}"),
            _ => {}
        }
    };
    assert_eq!(
        lost_closed, 1,
        "the open prompt must be withdrawn with connectionLost"
    );
    assert_eq!(password_prompts, 2);
    assert_eq!(
        host_key_prompts, 1,
        "the re-dial reuses the pinned host key"
    );
    assert_ne!(info.auth.method, AuthMethodUsed::Publickey);
    e.manager.close(&session_id, None).await;
}

/// Manual (Windows): the real Windows OpenSSH `ssh-agent` service pipe is accepted by the owner
/// check (owner LocalSystem) and signs for authentication.
/// Adds the throwaway fixture RSA key with `ssh-add` and removes it again afterwards.
/// Run: `cargo test --features it --test it_session windows_openssh_agent -- --ignored --nocapture`.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "uses the real Windows OpenSSH agent service (adds and removes a throwaway key)"]
async fn windows_openssh_agent_owner_check_and_auth() {
    use shellfin_lib::ssh::agent::{self, AgentEndpoint, OPENSSH_PIPE};
    use std::process::Command;

    let endpoint = AgentEndpoint::Pipe(OPENSSH_PIPE.into());
    let mut verified = agent::connect_verified(&endpoint)
        .await
        .expect("owner check rejected the real agent pipe");
    let before = agent::list_identities(&mut verified).await.unwrap().len();
    println!("real agent accepted by the pipe-owner check; {before} identities");

    // The Windows OpenSSH client (not e.g. Git's ssh-add, which talks to SSH_AUTH_SOCK).
    let win_ssh_add = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
        .join(r"System32\OpenSSH\ssh-add.exe");
    let (_d, key) = support::private_key_copy(support::KEY_RSA_OPENSSH);
    let st = Command::new(&win_ssh_add)
        .arg(&key)
        .status()
        .expect("ssh-add");
    assert!(st.success());
    let cleanup = || {
        let pubkey = key.with_extension("pub");
        let _ = Command::new(&win_ssh_add).arg("-d").arg(&pubkey).status();
    };
    let e = env(&format!(
        "Host bastion\n  HostName {}\n  Port {}\n  User tester\n  IdentityAgent //./pipe/openssh-ssh-agent\n  PasswordAuthentication no\n  KbdInteractiveAuthentication no\n",
        support::sshd().host,
        support::sshd().port
    ));
    let r = run(&e, sshconfig("bastion"), false, |p| {
        accept_once(p).unwrap_or_else(|| panic!("{p:?}"))
    })
    .await;
    cleanup();
    let info = r.info();
    assert_eq!(info.auth.method, AuthMethodUsed::Agent);
    assert_eq!(info.auth.key_type.as_deref(), Some("ssh-rsa"));
    e.manager.close(&r.session_id, None).await;
}
