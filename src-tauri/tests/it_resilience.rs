//! Integration cases (§10.2) against the Docker sshd: automatic reconnect after
//! `docker pause`/`unpause` of the target (stages, attempts, terminals respawned, SFTP
//! re-initialised, an interrupted transfer fails `disconnected` and can be retried), host-key
//! regeneration during an outage (`hostKeyMismatchOnReconnect` for a trust-once key,
//! `hostKeyChanged` for a key saved to known_hosts), and reconnect disabled in the settings.
//!
//! These cases disturb the shared `target` container (pause, host keys, killed sessions), so they
//! live in their own test binary (cargo runs test binaries one after another) and are serialised
//! with a lock inside it. They use `tester3`, whose sessions may be killed.
#![cfg(feature = "it")]
#![allow(
    clippy::disallowed_methods,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::result_large_err
)]

mod support;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use shellfin_lib::error::{AppError, DisconnectKind, ErrorCode};
use shellfin_lib::ipc::dto::{
    ConflictPolicy, OpenSessionRequest, Prompt, PromptAnswer, ReconnectStage, SessionEvent,
    SessionState, Target, TermEvent, TermSize, TransferEvent,
};
use shellfin_lib::known_hosts::TrustOnceApprovals;
use shellfin_lib::session::manager::SessionManager;
use shellfin_lib::session::session::{ConnectCtxTemplate, SessionDeps, SessionHandle};
use shellfin_lib::sftp::client::Sftp;
use shellfin_lib::sftp::transfer::{TransferSink, TransferSpec, TreeSpec};
use shellfin_lib::ssh::prompt::EventSink;
use shellfin_lib::sshconfig::ConfigEnv;
use shellfin_lib::sshconfig::service::SshConfigService;
use shellfin_lib::storage::Stores;
use shellfin_lib::terminal::{self, TermOutput};
use tokio::sync::mpsc;

const USER: &str = support::USER_LOW_TRIES;

/// One case at a time inside this binary.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// ---------------------------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------------------------

struct ChanSink(mpsc::UnboundedSender<SessionEvent>);
impl EventSink for ChanSink {
    fn send(&self, event: SessionEvent) {
        let _ = self.0.send(event);
    }
}

struct Env {
    _dir: tempfile::TempDir,
    _key_dir: tempfile::TempDir,
    stores: Arc<Stores>,
    manager: SessionManager,
}

fn q(p: &Path) -> String {
    format!("\"{}\"", p.display().to_string().replace('\\', "/"))
}

/// `target` through `bastion`, with fast loss detection on the target hop (keepalive 1 s × 2)
/// and a 3 s connect timeout per hop.
fn env() -> Env {
    let (key_dir, key) = support::private_key_copy(support::KEY_RSA_OPENSSH);
    let s = support::sshd();
    let key = q(&key);
    let config = format!(
        "Host bastion\n  HostName {}\n  Port {}\n  User {USER}\n  IdentityFile {key}\n  \
         IdentitiesOnly yes\n  ConnectTimeout 3\n\
         Host target\n  HostName {}\n  Port {}\n  User {USER}\n  ProxyJump bastion\n  \
         IdentityFile {key}\n  IdentitiesOnly yes\n  ServerAliveInterval 1\n  \
         ServerAliveCountMax 2\n  ConnectTimeout 3\n\
         Host target-probe\n  HostName {}\n  Port {}\n  User {USER}\n  ProxyJump bastion\n  \
         IdentityFile {key}\n  IdentitiesOnly yes\n  ServerAliveInterval 300\n  \
         ConnectTimeout 3\n\
         Host *\n  IdentityAgent none\n",
        s.host, s.port, s.target_host, s.target_port, s.target_host, s.target_port
    );
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
        _dir: dir,
        _key_dir: key_dir,
    }
}

fn prompt_id(p: &Prompt) -> &str {
    match p {
        Prompt::HostKeyUnknown { id, .. }
        | Prompt::Passphrase { id, .. }
        | Prompt::Password { id, .. }
        | Prompt::KeyboardInteractive { id, .. } => id,
    }
}

struct Session {
    id: String,
    handle: Arc<SessionHandle>,
    events: mpsc::UnboundedReceiver<SessionEvent>,
    /// Every state seen so far (in order).
    states: Vec<SessionState>,
    /// Answer for host-key prompts (`remember`).
    remember_host_keys: bool,
}

impl Session {
    /// Read events until a state matches `pred` (host-key prompts are accepted); returns it.
    async fn until(
        &mut self,
        e: &Env,
        timeout: Duration,
        what: &str,
        pred: impl Fn(&SessionState) -> bool,
    ) -> SessionState {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let ev = tokio::time::timeout_at(deadline, self.events.recv())
                .await
                .unwrap_or_else(|_| {
                    panic!("timed out waiting for {what}; states: {:#?}", self.states)
                })
                .expect("event channel closed");
            match ev {
                SessionEvent::Prompt { prompt } => {
                    let answer = match &prompt {
                        Prompt::HostKeyUnknown { .. } => PromptAnswer::HostKey {
                            accept: true,
                            remember: self.remember_host_keys,
                        },
                        other => panic!("unexpected prompt {other:?}"),
                    };
                    e.manager
                        .answer_prompt(&self.id, prompt_id(&prompt), answer)
                        .unwrap();
                }
                SessionEvent::State { state } => {
                    self.states.push(state.clone());
                    if pred(&state) {
                        return state;
                    }
                    if matches!(state, SessionState::Closed) {
                        panic!(
                            "closed while waiting for {what}; states: {:#?}",
                            self.states
                        );
                    }
                }
                _ => {}
            }
        }
    }
}

async fn connect(e: &Env, remember_host_keys: bool) -> Session {
    connect_to(e, "target", remember_host_keys).await
}

async fn connect_to(e: &Env, alias: &str, remember_host_keys: bool) -> Session {
    let (tx, rx) = mpsc::unbounded_channel();
    let id = e
        .manager
        .open(
            OpenSessionRequest {
                target: Target::SshConfig {
                    alias: alias.into(),
                },
                probe_only: false,
            },
            Arc::new(ChanSink(tx)),
        )
        .await
        .expect("session_open");
    let handle = e.manager.get(&id).unwrap();
    let mut s = Session {
        id,
        handle,
        events: rx,
        states: Vec::new(),
        remember_host_keys,
    };
    let st = s
        .until(e, Duration::from_secs(60), "connected", |st| {
            matches!(
                st,
                SessionState::Connected { .. } | SessionState::Disconnected { .. }
            )
        })
        .await;
    assert!(matches!(st, SessionState::Connected { .. }), "{st:?}");
    s
}

#[derive(Default)]
struct TermRec(Mutex<(Vec<u8>, Vec<TermEvent>)>);
impl TermOutput for TermRec {
    fn data(&self, bytes: Vec<u8>) {
        self.0.lock().unwrap().0.extend(bytes);
    }
    fn event(&self, event: TermEvent) {
        self.0.lock().unwrap().1.push(event);
    }
}

#[derive(Default)]
struct TransferRec(Mutex<Vec<TransferEvent>>);
impl TransferSink for TransferRec {
    fn send(&self, event: TransferEvent) {
        self.0.lock().unwrap().push(event);
    }
}

impl TransferRec {
    async fn finished(&self, id: &str, timeout: Duration) -> TransferEvent {
        let deadline = Instant::now() + timeout;
        loop {
            let hit = self.0.lock().unwrap().iter().find_map(|e| match e {
                TransferEvent::Done { transfer_id, .. }
                | TransferEvent::Failed { transfer_id, .. }
                | TransferEvent::Cancelled { transfer_id }
                    if transfer_id == id =>
                {
                    Some(e.clone())
                }
                _ => None,
            });
            if let Some(ev) = hit {
                return ev;
            }
            assert!(Instant::now() < deadline, "transfer {id} did not finish");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

fn is_disconnected(st: &SessionState) -> bool {
    matches!(st, SessionState::Disconnected { .. })
}

fn error_of(st: &SessionState) -> &AppError {
    match st {
        SessionState::Disconnected { error: Some(e), .. } => e,
        other => panic!("not a disconnected state with an error: {other:?}"),
    }
}

// ---------------------------------------------------------------------------------------------
// cases
// ---------------------------------------------------------------------------------------------

/// `docker pause` of the target ⇒ loss by keepalive ⇒ `reconnecting{waiting}` /
/// `{connecting|authenticating, hop}` with growing attempts while it stays paused ⇒ after
/// `unpause` the session is `connected` again without user action; the running terminal gets
/// `reconnected` and a new shell; SFTP is re-initialised (the old channel answers `notConnected`);
/// an upload interrupted by the loss fails `disconnected` (retryable) and its retry succeeds.
#[tokio::test(flavor = "multi_thread")]
async fn docker_pause_reconnects_automatically() {
    let _serial = SERIAL.lock().await;
    let e = env();
    let mut s = connect(&e, false).await;
    let old_sftp = Sftp::for_session(&s.handle).await.unwrap();
    let home = format!("/home/{USER}");
    old_sftp.list(&home).await.unwrap();

    let rec = Arc::new(TermRec::default());
    let term_id = terminal::open(
        &s.handle,
        TermSize {
            cols: 80,
            rows: 24,
            pixel_width: 640,
            pixel_height: 384,
        },
        None,
        rec.clone(),
    )
    .await
    .unwrap();

    // An upload large enough to still be running when the target freezes.
    let local = tempfile::tempdir().unwrap();
    let big = local.path().join("resilience.bin");
    std::fs::write(&big, vec![5u8; 256 * 1024 * 1024]).unwrap();
    let transfers = e.manager.transfers().clone();
    let trec = Arc::new(TransferRec::default());
    let sink: Arc<dyn TransferSink> = trec.clone();
    let upload_dir = format!("/tmp/it-resilience-{}", uuid::Uuid::new_v4().simple());
    old_sftp.mkdir(&upload_dir).await.unwrap();
    let upload = transfers.start(
        s.handle.clone(),
        TransferSpec::Upload {
            local: big.clone(),
            remote_dir: upload_dir.clone(),
        },
        false,
        sink.clone(),
    );
    // A folder download running at the same time (folder_retry_after_pause).
    let folder = format!(
        "/tmp/it-resilience-folder-{}",
        uuid::Uuid::new_v4().simple()
    );
    support::docker_exec_ok(
        support::TARGET,
        &[
            "sh",
            "-c",
            &format!(
                "mkdir -p '{folder}/sub' && chmod -R 755 '{folder}' && for i in 1 2 3; do \
                 head -c 67108864 /dev/urandom > '{folder}/sub/f'$i; done && chmod -R a+r '{folder}'"
            ),
        ],
    );
    let folder_local = tempfile::tempdir().unwrap();
    let job = transfers.start_tree(
        s.handle.clone(),
        TreeSpec::download(
            folder.clone(),
            folder_local.path().to_path_buf(),
            "res".into(),
        ),
        ConflictPolicy::Ask,
        sink.clone(),
    );
    tokio::time::sleep(Duration::from_millis(200)).await;

    let paused_at = Instant::now();
    let pause = support::PauseGuard::pause(support::TARGET);
    let first = s
        .until(
            &e,
            Duration::from_secs(30),
            "reconnecting{waiting, 1}",
            |st| {
                matches!(
                    st,
                    SessionState::Reconnecting {
                        stage: ReconnectStage::Waiting,
                        attempt: 1,
                        ..
                    }
                ) || is_disconnected(st)
            },
        )
        .await;
    eprintln!("loss detected {:?} after pause", paused_at.elapsed());
    match &first {
        SessionState::Reconnecting {
            hop,
            max_attempts,
            next_attempt_at_ms,
            cause,
            ..
        } => {
            assert!(hop.is_none());
            assert_eq!(*max_attempts, 5);
            assert!(next_attempt_at_ms.is_some());
            assert!(
                matches!(
                    cause.kind,
                    ErrorCode::Disconnected {
                        reason: DisconnectKind::KeepaliveTimeout,
                        ..
                    }
                ),
                "{cause:?}"
            );
        }
        other => panic!("{other:?}"),
    }
    // In-flight work of the lost connection: SFTP ⇒ notConnected, the upload ⇒ disconnected.
    let err = old_sftp.list(&home).await.unwrap_err();
    assert!(
        matches!(err.kind, ErrorCode::NotConnected { .. }),
        "{err:?}"
    );
    match trec.finished(&upload, Duration::from_secs(30)).await {
        TransferEvent::Failed {
            error, retryable, ..
        } => {
            assert!(
                matches!(error.kind, ErrorCode::Disconnected { .. }),
                "{error:?}"
            );
            assert!(retryable);
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        terminal_status(&s, &term_id),
        Some(shellfin_lib::terminal::TermStatus::Lost)
    ));

    // An attempt reaches the target hop while it is still frozen and fails (transport error).
    s.until(
        &e,
        Duration::from_secs(30),
        "connecting to the target hop",
        |st| {
            matches!(
                st,
                SessionState::Reconnecting {
                    stage: ReconnectStage::Connecting,
                    hop: Some(h),
                    attempt: 1,
                    ..
                } if h.index == 1
            )
        },
    )
    .await;
    let second = s
        .until(
            &e,
            Duration::from_secs(30),
            "reconnecting{waiting, 2}",
            |st| {
                matches!(
                    st,
                    SessionState::Reconnecting {
                        stage: ReconnectStage::Waiting,
                        attempt: 2,
                        ..
                    }
                ) || is_disconnected(st)
            },
        )
        .await;
    match &second {
        SessionState::Reconnecting { cause, .. } => assert!(
            matches!(cause.kind, ErrorCode::ConnectTimeout { .. }),
            "the latest error is the cause: {cause:?}"
        ),
        other => panic!("{other:?}"),
    }
    pause.unpause();
    let connected = s
        .until(&e, Duration::from_secs(60), "connected again", |st| {
            matches!(st, SessionState::Connected { .. }) || is_disconnected(st)
        })
        .await;
    assert!(
        matches!(connected, SessionState::Connected { .. }),
        "{connected:?}"
    );
    eprintln!("reconnected {:?} after pause", paused_at.elapsed());

    // Every state between the loss and `connected` is `reconnecting`, attempts never decrease.
    let since_loss: Vec<&SessionState> = s
        .states
        .iter()
        .skip_while(|st| !matches!(st, SessionState::Reconnecting { .. }))
        .collect();
    let mut last_attempt = 0;
    for st in &since_loss[..since_loss.len() - 1] {
        match st {
            SessionState::Reconnecting {
                attempt,
                stage,
                hop,
                ..
            } => {
                assert!(*attempt >= last_attempt);
                last_attempt = *attempt;
                assert_eq!(*stage == ReconnectStage::Waiting, hop.is_none(), "{st:?}");
            }
            other => panic!("unexpected state during reconnect: {other:?}"),
        }
    }
    assert!(last_attempt >= 2);

    // Terminal: `reconnected`, then a working shell.
    let deadline = Instant::now() + Duration::from_secs(30);
    while !rec
        .0
        .lock()
        .unwrap()
        .1
        .iter()
        .any(|ev| matches!(ev, TermEvent::Reconnected))
    {
        assert!(Instant::now() < deadline, "no reconnected event");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let from = rec.0.lock().unwrap().0.len();
    let (_, term) = e.manager.terminal(&term_id).unwrap();
    term.write(b"echo re''connected-ok\r".to_vec())
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let seen = String::from_utf8_lossy(&rec.0.lock().unwrap().0[from..]).into_owned();
        if seen.contains("reconnected-ok") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "shell not responsive after reconnect"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // SFTP was re-initialised on the new connection.
    let new_sftp = Sftp::for_session(&s.handle).await.unwrap();
    assert!(!Arc::ptr_eq(&new_sftp.conn, &old_sftp.conn));
    new_sftp.list(&home).await.unwrap();

    // The interrupted upload can be retried now.
    let (_, spec) = transfers.take_failed(&upload).unwrap();
    let retry = transfers.start(s.handle.clone(), spec, true, sink.clone());
    assert!(matches!(
        trec.finished(&retry, Duration::from_secs(120)).await,
        TransferEvent::Done { .. }
    ));
    let entry = new_sftp
        .entry(&format!("{upload_dir}/resilience.bin"))
        .await
        .unwrap();
    assert_eq!(entry.size, Some(256 * 1024 * 1024));

    // The folder job failed with the connection (retryable) and its retry copies the rest.
    match trec.finished(&job, Duration::from_secs(60)).await {
        TransferEvent::Failed {
            error, retryable, ..
        } => {
            assert!(
                matches!(error.kind, ErrorCode::Disconnected { .. }),
                "{error:?}"
            );
            assert!(retryable);
        }
        // Small enough to finish before the freeze on a fast machine: nothing to retry then.
        TransferEvent::Done { .. } => {}
        other => panic!("{other:?}"),
    }
    if let Ok((_, TransferSpec::Tree(state))) = transfers.take_failed(&job) {
        let retry = transfers.restart_tree(
            s.handle.clone(),
            state,
            false,
            ConflictPolicy::Ask,
            sink.clone(),
        );
        match trec.finished(&retry, Duration::from_secs(120)).await {
            TransferEvent::Done {
                summary: Some(summary),
                ..
            } => assert_eq!(summary.failed, 0, "{summary:?}"),
            other => panic!("{other:?}"),
        }
    }
    for i in 1..=3 {
        let p = folder_local.path().join(format!("res/sub/f{i}"));
        assert_eq!(std::fs::metadata(&p).unwrap().len(), 67_108_864, "{p:?}");
    }
    let leftovers: Vec<_> = std::fs::read_dir(folder_local.path().join("res/sub"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".part"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    support::docker_exec_ok(support::TARGET, &["rm", "-rf", &folder]);

    support::docker_exec_ok(support::TARGET, &["rm", "-rf", &upload_dir]);
    e.manager.close(&s.id, None).await;
}

fn terminal_status(s: &Session, term_id: &str) -> Option<shellfin_lib::terminal::TermStatus> {
    s.handle.terminals.get(term_id).map(|t| t.status())
}

/// §5.3/§10.2: the target's host keys are regenerated during an outage. Precondition decides the
/// code: a key accepted **once** (not saved) ⇒ `hostKeyMismatchOnReconnect`; a key **saved** to
/// known_hosts ⇒ `hostKeyChanged` (known_hosts now disagrees, checked before the pin). Both are
/// fatal: no further attempt, `canRetry: false`.
#[tokio::test(flavor = "multi_thread")]
async fn host_key_regenerated_during_outage_is_fatal() {
    let _serial = SERIAL.lock().await;
    // The target's original host keys come back afterwards (also on failure).
    let _keys = support::HostKeyGuard::backup(support::TARGET);
    for remember in [false, true] {
        let e = env();
        let mut s = connect(&e, remember).await;
        support::regenerate_host_keys(support::TARGET);
        support::docker_exec_ok(support::TARGET, &["pkill", "-KILL", "-u", USER]);
        let end = s
            .until(&e, Duration::from_secs(60), "disconnected", is_disconnected)
            .await;
        let err = error_of(&end);
        if remember {
            // The same params as at connect time (file:line, removal hint).
            match &err.kind {
                ErrorCode::HostKeyChanged {
                    host,
                    port,
                    known_hosts_file,
                    line,
                    remove_hint,
                    remove_hint_shell,
                    ..
                } => {
                    assert_eq!((host.as_str(), *port), ("target", 22));
                    assert!(
                        known_hosts_file.ends_with("known_hosts"),
                        "{known_hosts_file}"
                    );
                    assert!(*line >= 1);
                    let hint = remove_hint.as_deref().expect("removeHint");
                    assert!(hint.starts_with("ssh-keygen -R"), "{hint}");
                    assert_eq!(*remove_hint_shell, shellfin_lib::error::HintShell::local());
                }
                _ => panic!("{err:?}"),
            }
        } else {
            assert!(
                matches!(err.kind, ErrorCode::HostKeyMismatchOnReconnect { .. }),
                "{err:?}"
            );
        }
        assert!(matches!(
            end,
            SessionState::Disconnected {
                can_retry: false,
                ..
            }
        ));
        // Exactly one attempt was made, and it never got past the target's key exchange.
        let attempts: Vec<u32> = s
            .states
            .iter()
            .filter_map(|st| match st {
                SessionState::Reconnecting { attempt, .. } => Some(*attempt),
                _ => None,
            })
            .collect();
        assert!(
            !attempts.is_empty() && attempts.iter().all(|a| *a == 1),
            "{attempts:?}"
        );
        assert!(!s.states.iter().any(|st| matches!(
            st,
            SessionState::Reconnecting {
                stage: ReconnectStage::Authenticating,
                hop: Some(h),
                ..
            } if h.index == 1
        )));
        e.manager.close(&s.id, None).await;
    }
}

/// `connection.reconnect.enabled = false`: a loss ends in `disconnected{canRetry: true}` without
/// `reconnecting`; `session_reconnect_now` still reconnects by hand.
#[tokio::test(flavor = "multi_thread")]
async fn reconnect_disabled_waits_for_the_user() {
    let _serial = SERIAL.lock().await;
    let e = env();
    let mut settings = e.stores.settings();
    settings.connection.reconnect.enabled = false;
    e.stores.update_settings(settings).unwrap();
    let mut s = connect(&e, false).await;
    support::docker_exec_ok(support::TARGET, &["pkill", "-KILL", "-u", USER]);
    let end = s
        .until(&e, Duration::from_secs(30), "disconnected", is_disconnected)
        .await;
    assert!(matches!(
        end,
        SessionState::Disconnected {
            can_retry: true,
            ..
        }
    ));
    assert!(matches!(
        error_of(&end).kind,
        ErrorCode::Disconnected { .. }
    ));
    assert!(
        !s.states
            .iter()
            .any(|st| matches!(st, SessionState::Reconnecting { .. }))
    );
    e.manager.reconnect_now(&s.id).unwrap();
    let st = s
        .until(&e, Duration::from_secs(30), "connected", |st| {
            matches!(st, SessionState::Connected { .. }) || is_disconnected(st)
        })
        .await;
    assert!(matches!(st, SessionState::Connected { .. }), "{st:?}");
    e.manager.close(&s.id, None).await;
}

/// `session_disconnect` ends the connection but keeps the session. State becomes
/// `disconnected{userRequested, canRetry: true}`, auto-reconnect (enabled) does not start, the
/// terminal is kept (lost), and `session_reconnect_now` reconnects: the terminal gets
/// `reconnected` and SFTP works again.
#[tokio::test(flavor = "multi_thread")]
async fn session_disconnect_keeps_the_session_and_reconnects_by_hand() {
    let _serial = SERIAL.lock().await;
    let e = env();
    let mut s = connect(&e, false).await;
    let rec = Arc::new(TermRec::default());
    let term_id = terminal::open(
        &s.handle,
        TermSize {
            cols: 80,
            rows: 24,
            pixel_width: 640,
            pixel_height: 384,
        },
        None,
        rec.clone(),
    )
    .await
    .unwrap();

    e.manager.disconnect(&s.id).await.unwrap();
    let end = s
        .until(&e, Duration::from_secs(15), "disconnected", is_disconnected)
        .await;
    assert!(matches!(
        end,
        SessionState::Disconnected {
            can_retry: true,
            ..
        }
    ));
    assert!(matches!(
        error_of(&end).kind,
        ErrorCode::Disconnected {
            reason: DisconnectKind::UserRequested,
            ..
        }
    ));
    // Idempotent, and no automatic reconnect follows (the default settings enable it).
    e.manager.disconnect(&s.id).await.unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        !s.states
            .iter()
            .any(|st| matches!(st, SessionState::Reconnecting { .. }))
    );
    assert!(e.manager.get(&s.id).is_some());
    assert!(terminal_status(&s, &term_id).is_some(), "terminal kept");

    e.manager.reconnect_now(&s.id).unwrap();
    let st = s
        .until(&e, Duration::from_secs(30), "connected", |st| {
            matches!(st, SessionState::Connected { .. }) || is_disconnected(st)
        })
        .await;
    assert!(matches!(st, SessionState::Connected { .. }), "{st:?}");
    let deadline = Instant::now() + Duration::from_secs(15);
    while !rec
        .0
        .lock()
        .unwrap()
        .1
        .iter()
        .any(|ev| matches!(ev, TermEvent::Reconnected))
    {
        assert!(Instant::now() < deadline, "terminal never reconnected");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let sftp = Sftp::for_session(&s.handle).await.unwrap();
    sftp.list(&format!("/home/{USER}")).await.unwrap();
    e.manager.close(&s.id, None).await;
}

/// §5.7: with keepalives effectively off (300 s), only `session_probe_all` can notice a
/// frozen target: two 5 s ping timeouts declare the session dead (`keepaliveTimeout`), then
/// auto-reconnect takes over and reconnects after `unpause`. A second probe while reconnecting
/// declares nothing (no connection to probe).
#[tokio::test(flavor = "multi_thread")]
async fn probe_declares_loss_then_auto_reconnects() {
    let _serial = SERIAL.lock().await;
    let e = env();
    let mut s = connect_to(&e, "target-probe", false).await;
    let pause = support::PauseGuard::pause(support::TARGET);
    let started = Instant::now();
    let lost = e.manager.probe_all().await;
    eprintln!("probe declared {lost:?} after {:?}", started.elapsed());
    assert_eq!(lost, vec![s.id.clone()]);
    assert!(started.elapsed() >= Duration::from_secs(10));
    let first = s
        .until(
            &e,
            Duration::from_secs(15),
            "reconnecting{waiting, 1}",
            |st| {
                matches!(
                    st,
                    SessionState::Reconnecting {
                        stage: ReconnectStage::Waiting,
                        attempt: 1,
                        ..
                    }
                ) || is_disconnected(st)
            },
        )
        .await;
    match &first {
        SessionState::Reconnecting { cause, .. } => assert!(
            matches!(
                cause.kind,
                ErrorCode::Disconnected {
                    reason: DisconnectKind::KeepaliveTimeout,
                    ..
                }
            ),
            "{cause:?}"
        ),
        other => panic!("{other:?}"),
    }
    assert!(e.manager.probe_all().await.is_empty());
    pause.unpause();
    let st = s
        .until(&e, Duration::from_secs(60), "connected again", |st| {
            matches!(st, SessionState::Connected { .. }) || is_disconnected(st)
        })
        .await;
    assert!(matches!(st, SessionState::Connected { .. }), "{st:?}");
    assert!(e.manager.probe_all().await.is_empty());
    e.manager.close(&s.id, None).await;
}
