//! Integration cases (§10.2) against the Docker sshd: PTY size and resize
//! (`stty size`), binary safety, exit status/signal, restart, `terminalCount`, hostile `cwd`
//! names for bash/fish/tcsh login shells, respawn after a lost connection, and the flood test
//! (a–d).
//! Headless: `SessionManager` + in-memory sinks, private temp home (as `it_session.rs`).
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
use std::sync::Arc;
use std::time::{Duration, Instant};

use shellfin_lib::error::{AppError, ErrorCode};
use shellfin_lib::ipc::dto::{
    OpenSessionRequest, Prompt, PromptAnswer, SessionEvent, SessionState, Target, TermEvent,
    TermSize,
};
use shellfin_lib::known_hosts::TrustOnceApprovals;
use shellfin_lib::session::manager::SessionManager;
use shellfin_lib::session::session::{ConnectCtxTemplate, SessionDeps, SessionHandle};
use shellfin_lib::ssh::prompt::EventSink;
use shellfin_lib::sshconfig::ConfigEnv;
use shellfin_lib::sshconfig::service::SshConfigService;
use shellfin_lib::storage::Stores;
use shellfin_lib::terminal::{self, TermOutput, TermStatus, Terminal};
use tokio::sync::mpsc;

const STEP: Duration = Duration::from_secs(30);

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

/// A private home whose ssh config has `Host bastion` for `user` with the RSA test key.
fn env(user: &str) -> Env {
    let (key_dir, key) = support::private_key_copy(support::KEY_RSA_OPENSSH);
    let s = support::sshd();
    let config = format!(
        "Host bastion\n  HostName {}\n  Port {}\n  User {user}\n  IdentityFile {}\n  \
         IdentitiesOnly yes\nHost *\n  IdentityAgent none\n",
        s.host,
        s.port,
        q(&key)
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

struct Session {
    id: String,
    handle: Arc<SessionHandle>,
    events: mpsc::UnboundedReceiver<SessionEvent>,
}

fn prompt_id(p: &Prompt) -> &str {
    match p {
        Prompt::HostKeyUnknown { id, .. }
        | Prompt::Passphrase { id, .. }
        | Prompt::Password { id, .. }
        | Prompt::KeyboardInteractive { id, .. } => id,
    }
}

/// Drive session events until `connected` (host keys accepted once); panics otherwise.
async fn until_connected(e: &Env, s: &mut Session) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    loop {
        let ev = tokio::time::timeout_at(deadline, s.events.recv())
            .await
            .expect("timed out waiting for connected")
            .expect("event channel closed");
        match ev {
            SessionEvent::Prompt { prompt } => {
                let answer = match &prompt {
                    Prompt::HostKeyUnknown { .. } => PromptAnswer::HostKey {
                        accept: true,
                        remember: false,
                    },
                    other => panic!("unexpected prompt {other:?}"),
                };
                e.manager
                    .answer_prompt(&s.id, prompt_id(&prompt), answer)
                    .unwrap();
            }
            SessionEvent::State {
                state: SessionState::Connected { .. },
            } => return,
            SessionEvent::State {
                state: SessionState::Disconnected { error, .. },
            } => panic!("disconnected: {error:?}"),
            _ => {}
        }
    }
}

/// Wait for a `disconnected` state.
async fn until_disconnected(s: &mut Session) -> Option<AppError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let ev = tokio::time::timeout_at(deadline, s.events.recv())
            .await
            .expect("timed out waiting for disconnected")
            .expect("event channel closed");
        if let SessionEvent::State {
            state: SessionState::Disconnected { error, .. },
        } = ev
        {
            return error;
        }
    }
}

async fn connect(e: &Env) -> Session {
    let (tx, rx) = mpsc::unbounded_channel();
    let id = e
        .manager
        .open(
            OpenSessionRequest {
                target: Target::SshConfig {
                    alias: "bastion".into(),
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
    };
    until_connected(e, &mut s).await;
    s
}

#[derive(Debug)]
enum Msg {
    Data(Vec<u8>),
    Event(TermEvent),
}

struct Rec(mpsc::UnboundedSender<Msg>);
impl TermOutput for Rec {
    fn data(&self, bytes: Vec<u8>) {
        let _ = self.0.send(Msg::Data(bytes));
    }
    fn event(&self, event: TermEvent) {
        let _ = self.0.send(Msg::Event(event));
    }
}

/// One terminal as the frontend sees it: its output stream, acked as it is consumed (unless
/// `ack` is off).
struct Term {
    id: String,
    term: Arc<Terminal>,
    rx: mpsc::UnboundedReceiver<Msg>,
    /// Everything received so far.
    seen: Vec<u8>,
    events: Vec<TermEvent>,
    ack: bool,
}

fn size(cols: u32, rows: u32) -> TermSize {
    TermSize {
        cols,
        rows,
        pixel_width: cols * 8,
        pixel_height: rows * 16,
    }
}

async fn open_term(
    e: &Env,
    s: &Session,
    sz: TermSize,
    cwd: Option<&str>,
) -> Result<Term, AppError> {
    let (tx, rx) = mpsc::unbounded_channel();
    let id = terminal::open(&s.handle, sz, cwd, Arc::new(Rec(tx))).await?;
    let (_, term) = e.manager.terminal(&id).unwrap();
    Ok(Term {
        id,
        term,
        rx,
        seen: Vec::new(),
        events: Vec::new(),
        ack: true,
    })
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

impl Term {
    async fn write(&self, data: &[u8]) {
        self.term.write(data.to_vec()).await.expect("term_write");
    }

    /// Receive one message into `seen`/`events` (acking data when `ack` is on, as the frontend
    /// does from its write callbacks); `false` on timeout.
    async fn recv(&mut self, timeout: Duration) -> bool {
        let Ok(Some(msg)) = tokio::time::timeout(timeout, self.rx.recv()).await else {
            return false;
        };
        match msg {
            Msg::Data(d) => {
                if self.ack {
                    self.term.ack(d.len() as u64);
                }
                self.seen.extend(d);
            }
            Msg::Event(ev) => self.events.push(ev),
        }
        true
    }

    /// Read until `needle` appears in the output received after `from`.
    async fn wait_for(&mut self, from: usize, needle: &[u8], timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while !contains(&self.seen[from.min(self.seen.len())..], needle) {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(
                self.recv(left).await,
                "timed out waiting for {:?}; tail: {:?}",
                String::from_utf8_lossy(needle),
                String::from_utf8_lossy(&self.seen[self.seen.len().saturating_sub(400)..])
            );
        }
    }

    /// Read until an event matching `pred` arrives; returns it.
    async fn wait_event(
        &mut self,
        timeout: Duration,
        pred: impl Fn(&TermEvent) -> bool,
    ) -> TermEvent {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(ev) = self.events.iter().find(|e| pred(e)) {
                return ev.clone();
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(
                self.recv(left).await,
                "timed out waiting for an event; events: {:?}",
                self.events
            );
        }
    }

    /// Wait for a shell prompt-ish quiet point: echo a unique marker and wait for it.
    async fn sync(&mut self, tag: &str) {
        let marker = format!("sync-{tag}-{}", uuid::Uuid::new_v4().simple());
        let from = self.seen.len();
        // `printf` would differ across shells; `echo` with a plain word works everywhere. The
        // marker is split in the command line so the echoed input cannot match.
        let (a, b) = marker.split_at(5);
        self.write(format!("echo {a}''{b}\r").as_bytes()).await;
        self.wait_for(from, marker.as_bytes(), STEP).await;
    }
}

// ---------------------------------------------------------------------------------------------
// cases
// ---------------------------------------------------------------------------------------------

/// `stty size` follows resizes; binary-safe output and input; exit status and signal;
/// restart; writes after exit are `notConnected`; `session_close` ends running terminals with
/// `closed{error:null}` and records `terminalCount`.
#[tokio::test(flavor = "multi_thread")]
async fn pty_size_resize_binary_exit_restart_and_close() {
    let e = env(support::USER);
    let s = connect(&e).await;
    let mut t = open_term(&e, &s, size(80, 24), None).await.unwrap();
    let mut keep = open_term(&e, &s, size(100, 30), None).await.unwrap();
    assert_eq!(t.term.status(), TermStatus::Running);

    // Initial size, then a resize.
    t.sync("start").await;
    let from = t.seen.len();
    t.write(b"stty size\r").await;
    t.wait_for(from, b"24 80", STEP).await;
    t.term.resize(size(132, 40));
    let from = t.seen.len();
    t.write(b"stty size\r").await;
    t.wait_for(from, b"40 132", STEP).await;
    assert_eq!(t.term.size(), size(132, 40));

    // Raw bytes out (0xFF 0xFE NUL 0x01 survive), UTF-8 in.
    let from = t.seen.len();
    t.write(b"printf '\\377\\376\\000\\001|\\n'\r").await;
    t.wait_for(from, b"\xff\xfe\x00\x01|", STEP).await;
    let from = t.seen.len();
    t.write("echo é | od -An -tx1\r".as_bytes()).await;
    t.wait_for(from, b"c3 a9 0a", STEP).await;

    // Exit status ⇒ `exit{code}`; then writes fail with notConnected.
    t.write(b"exit 3\r").await;
    let ev = t
        .wait_event(STEP, |e| matches!(e, TermEvent::Exit { .. }))
        .await;
    assert!(
        matches!(
            ev,
            TermEvent::Exit {
                code: Some(3),
                signal: None
            }
        ),
        "{ev:?}"
    );
    assert_eq!(t.term.status(), TermStatus::Exited);
    let err = t.term.write(b"x".to_vec()).await.unwrap_err();
    assert!(
        matches!(err.kind, ErrorCode::NotConnected { .. }),
        "{err:?}"
    );

    // Restart: a new shell with the stored size.
    terminal::restart(&s.handle, &t.term).await.unwrap();
    assert_eq!(t.term.status(), TermStatus::Running);
    t.sync("restarted").await;
    let from = t.seen.len();
    t.write(b"stty size\r").await;
    t.wait_for(from, b"40 132", STEP).await;

    // Killed by a signal ⇒ `exit{signal}`.
    t.events.clear();
    t.write(b"kill -KILL $$\r").await;
    let ev = t
        .wait_event(STEP, |e| matches!(e, TermEvent::Exit { .. }))
        .await;
    assert!(
        matches!(&ev, TermEvent::Exit { code: None, signal: Some(sig) } if sig == "KILL"),
        "{ev:?}"
    );

    // Close the session: the running terminal gets `closed{error:null}` last; recents count it.
    keep.sync("keep").await;
    e.manager.close(&s.id, Some("/tmp".into())).await;
    let ev = keep
        .wait_event(STEP, |e| matches!(e, TermEvent::Closed { .. }))
        .await;
    assert!(matches!(ev, TermEvent::Closed { error: None }));
    let recents = e.stores.lock_recents().list();
    assert_eq!(recents.len(), 1);
    assert_eq!(recents[0].terminal_count, 1, "one shell was still running");
    assert!(e.manager.find_terminal(&keep.id).is_none());
    assert!(e.manager.find_terminal(&t.id).is_none());
}

/// §4.5 / §10.2: "Open terminal here" with hostile directory names, for bash, fish and tcsh
/// login shells: the shell starts in the right directory, and nothing is executed.
#[tokio::test(flavor = "multi_thread")]
async fn cwd_with_hostile_names_for_bash_fish_and_tcsh() {
    let names: &[&str] = &[
        "it's",
        r"a\'b",
        r"a\\'b",
        "$(touch pwned-1)",
        "`touch pwned-2`",
        "a&b;touch pwned-3",
        "q\"uote",
        r"back\slash",
        "bang!x",
        "Українська ✓",
        " two  spaces ",
        "-dash",
        "%s${HOME}",
        // Printf directives and escapes stay literal (`\c` would end printf's output).
        "100%%",
        "%b%n%d",
        r"a\cb",
        r"x\ny\t",
        r"\0101\x41",
    ];
    // Create the directories as `tester` over SFTP (path strings pass through unchanged).
    let base = format!("/tmp/shellfin-cwd-{}", uuid::Uuid::new_v4().simple());
    {
        let e = env(support::USER);
        let s = connect(&e).await;
        let conn = s.handle.connection.read().await.clone().unwrap();
        let sftp = conn.sftp.clone().expect("sftp");
        let mut attrs = russh_sftp::protocol::FileAttributes::empty();
        attrs.permissions = Some(0o755);
        sftp.mkdir(base.clone(), attrs.clone()).await.unwrap();
        for n in names {
            sftp.mkdir(format!("{base}/{n}"), attrs.clone())
                .await
                .unwrap_or_else(|err| panic!("mkdir {n:?}: {err:?}"));
        }
        e.manager.close(&s.id, None).await;
    }

    for user in [support::USER, support::USER_FISH, support::USER_TCSH] {
        let e = env(user);
        let s = connect(&e).await;
        for n in names {
            let path = format!("{base}/{n}");
            let mut t = open_term(&e, &s, size(400, 24), Some(&path)).await.unwrap();
            let expected: String = path
                .bytes()
                .chain(std::iter::once(b'\n'))
                .map(|b| format!("{b:02x}"))
                .collect();
            let from = t.seen.len();
            // External `pwd`: fish's builtin re-encodes non-ASCII bytes under the C locale.
            t.write(b"/bin/pwd | od -An -tx1 | tr -d ' \\n'; echo\r")
                .await;
            t.wait_for(from, expected.as_bytes(), STEP).await;
            terminal::close(&s.handle, &t.id).await;
        }
        // A directory that does not exist: the shell is not started, the channel ends.
        let mut t = open_term(&e, &s, size(80, 24), Some(&format!("{base}/missing")))
            .await
            .unwrap();
        let ev = t
            .wait_event(STEP, |e| matches!(e, TermEvent::Exit { .. }))
            .await;
        assert!(
            matches!(ev, TermEvent::Exit { code: Some(c), signal: None } if c != 0),
            "{user}: {ev:?}"
        );
        e.manager.close(&s.id, None).await;
    }

    let found = support::docker_exec_ok(
        support::BASTION,
        &["find", "/tmp", "/home", "-name", "pwned-*"],
    );
    assert!(found.trim().is_empty(), "injection markers found: {found}");
}

/// `terminal.termType` is the PTY `TERM` (also after a restart), and
/// `newTerminalCwd = currentFolder` (`cwdFallback`) enters hostile folder names safely and falls
/// back to home for a missing folder or an unusable path, for bash, fish and tcsh.
#[tokio::test(flavor = "multi_thread")]
async fn term_type_and_cwd_fallback() {
    let names: &[&str] = &[
        "it's",
        "$(touch pwned-v3-1)",
        "a&b;touch pwned-v3-2",
        "`touch pwned-v3-3`",
        "Українська ✓",
    ];
    let base = format!("/tmp/shellfin-cwd3-{}", uuid::Uuid::new_v4().simple());
    {
        let e = env(support::USER);
        let s = connect(&e).await;
        let conn = s.handle.connection.read().await.clone().unwrap();
        let sftp = conn.sftp.clone().expect("sftp");
        let mut attrs = russh_sftp::protocol::FileAttributes::empty();
        attrs.permissions = Some(0o755);
        sftp.mkdir(base.clone(), attrs.clone()).await.unwrap();
        for n in names {
            sftp.mkdir(format!("{base}/{n}"), attrs.clone())
                .await
                .unwrap();
        }
        e.manager.close(&s.id, None).await;
    }

    let open = |s: &Session, cwd: Option<String>, term_type: shellfin_lib::ipc::dto::TermType| {
        let handle = s.handle.clone();
        async move {
            let (tx, rx) = mpsc::unbounded_channel();
            let options = terminal::OpenOptions {
                cwd,
                cwd_fallback: true,
                term_type,
            };
            let id = terminal::open_with(&handle, size(400, 24), &options, Arc::new(Rec(tx)))
                .await
                .expect("term_open with cwdFallback never fails because of the folder");
            (id, rx)
        }
    };
    let term = |e: &Env, id: String, rx: mpsc::UnboundedReceiver<Msg>| {
        let (_, t) = e.manager.terminal(&id).unwrap();
        Term {
            id,
            term: t,
            rx,
            seen: Vec::new(),
            events: Vec::new(),
            ack: true,
        }
    };

    for user in [support::USER, support::USER_FISH, support::USER_TCSH] {
        let e = env(user);
        let s = connect(&e).await;

        // TERM follows the setting, also after a restart.
        let (id, rx) = open(&s, None, shellfin_lib::ipc::dto::TermType::Vt100).await;
        let mut t = term(&e, id, rx);
        let from = t.seen.len();
        t.write(b"sh -c 'echo term-$TERM-end'\r").await;
        t.wait_for(from, b"term-vt100-end", STEP).await;
        terminal::restart(&s.handle, &t.term).await.unwrap();
        t.sync("restarted").await;
        let from = t.seen.len();
        t.write(b"sh -c 'echo term-$TERM-end'\r").await;
        t.wait_for(from, b"term-vt100-end", STEP).await;
        terminal::close(&s.handle, &t.id).await;

        // Hostile folder names are entered exactly.
        for n in names {
            let path = format!("{base}/{n}");
            let (id, rx) = open(
                &s,
                Some(path.clone()),
                shellfin_lib::ipc::dto::TermType::Xterm256Color,
            )
            .await;
            let mut t = term(&e, id, rx);
            let expected: String = path
                .bytes()
                .chain(std::iter::once(b'\n'))
                .map(|b| format!("{b:02x}"))
                .collect();
            let from = t.seen.len();
            t.write(b"/bin/pwd | od -An -tx1 | tr -d ' \\n'; echo\r")
                .await;
            t.wait_for(from, expected.as_bytes(), STEP).await;
            terminal::close(&s.handle, &t.id).await;
        }

        // A missing folder, a path with a newline and a relative path: a shell in home.
        for cwd in [
            format!("{base}/missing"),
            format!("{base}/x\ntouch pwned-v3-4"),
            "relative/dir".to_owned(),
        ] {
            let (id, rx) = open(
                &s,
                Some(cwd.clone()),
                shellfin_lib::ipc::dto::TermType::Xterm256Color,
            )
            .await;
            let mut t = term(&e, id, rx);
            let from = t.seen.len();
            t.write(b"sh -c 'test \"$(pwd)\" = \"$HOME\" && echo home-$((40+2))'\r")
                .await;
            t.wait_for(from, b"home-42", STEP).await;
            assert!(
                !t.events
                    .iter()
                    .any(|ev| matches!(ev, TermEvent::Exit { .. })),
                "{user} {cwd:?}: {:?}",
                t.events
            );
            terminal::close(&s.handle, &t.id).await;
        }
        e.manager.close(&s.id, None).await;
    }

    let found = support::docker_exec_ok(
        support::BASTION,
        &["find", "/tmp", "/home", "-name", "pwned-v3-*"],
    );
    assert!(found.trim().is_empty(), "injection markers found: {found}");
}

/// A lost connection keeps the terminal (`lost`, writes are `notConnected`); after the session
/// reconnects, the shell is respawned with the same size and `reconnected` comes first (§5.8).
#[tokio::test(flavor = "multi_thread")]
async fn lost_connection_respawns_terminal_after_reconnect() {
    // Dedicated user: the test kills every process of it in the container.
    let e = env(support::USER_LOW_TRIES);
    // The manual path (`disconnected` → `session_reconnect_now`); the automatic one is
    // covered by `it_resilience`.
    let mut settings = e.stores.settings();
    settings.connection.reconnect.enabled = false;
    e.stores.update_settings(settings).unwrap();
    let mut s = connect(&e).await;
    let mut t = open_term(&e, &s, size(90, 33), None).await.unwrap();
    t.sync("before").await;

    support::docker_exec_ok(
        support::BASTION,
        &["pkill", "-KILL", "-u", support::USER_LOW_TRIES],
    );
    let err = until_disconnected(&mut s).await;
    assert!(
        matches!(
            err.as_ref().map(|e| &e.kind),
            Some(ErrorCode::Disconnected { .. })
        ),
        "{err:?}"
    );
    assert_eq!(t.term.status(), TermStatus::Lost);
    let werr = t.term.write(b"x".to_vec()).await.unwrap_err();
    assert!(matches!(werr.kind, ErrorCode::NotConnected { .. }));
    // The UI may resize while disconnected; the respawned shell uses the latest size.
    t.term.resize(size(91, 34));

    t.events.clear();
    e.manager.reconnect_now(&s.id).unwrap();
    until_connected(&e, &mut s).await;
    let ev = t
        .wait_event(STEP, |e| matches!(e, TermEvent::Reconnected))
        .await;
    assert!(matches!(ev, TermEvent::Reconnected));
    assert!(
        matches!(t.events.first(), Some(TermEvent::Reconnected)),
        "reconnected is the first message: {:?}",
        t.events
    );
    let reconnected_at = t.seen.len();
    t.sync("after").await;
    let from = t.seen.len().max(reconnected_at);
    t.write(b"stty size\r").await;
    t.wait_for(from, b"34 91", STEP).await;
    e.manager.close(&s.id, None).await;
}

/// §10.2 flood test: `yes` in terminal 1 with acks withheld ⇒ (a) `session_probe_all` does not
/// declare loss, (b) an SFTP write succeeds, (c) terminal 2 echoes within 1 s, (d)
/// `outputDropped` arrives (once the UI catches up), before any newer output.
#[tokio::test(flavor = "multi_thread")]
async fn flood_does_not_block_the_session() {
    let e = env(support::USER);
    let mut s = connect(&e).await;
    let mut t1 = open_term(&e, &s, size(80, 24), None).await.unwrap();
    let mut t2 = open_term(&e, &s, size(80, 24), None).await.unwrap();
    t1.sync("t1").await;
    t2.sync("t2").await;

    // Flood with acks withheld until the backlog overflows.
    t1.ack = false;
    t1.write(b"yes\r").await;
    let deadline = Instant::now() + Duration::from_secs(60);
    while !t1.term.flow_stats().dropping {
        assert!(
            Instant::now() < deadline,
            "no overflow: {:?}",
            t1.term.flow_stats()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let stats = t1.term.flow_stats();
    assert!(stats.unacked >= 4 * 1024 * 1024, "{stats:?}");
    assert!(stats.queued <= 8 * 1024 * 1024, "{stats:?}");

    // (a) the probe does not declare loss.
    let started = Instant::now();
    let lost = e.manager.probe_all().await;
    assert!(lost.is_empty(), "probe declared loss under flood");
    eprintln!("flood: probe_all took {:?}", started.elapsed());
    assert!(s.events.try_recv().map_or(true, |ev| !matches!(
        ev,
        SessionEvent::State {
            state: SessionState::Disconnected { .. }
        }
    )));

    // (b) SFTP works (write, then read back).
    let conn = s.handle.connection.read().await.clone().unwrap();
    let sftp = conn.sftp.clone().expect("sftp");
    let started = Instant::now();
    let path = format!("/tmp/shellfin-flood-{}", uuid::Uuid::new_v4().simple());
    let payload: Vec<u8> = (0..65_536u32).map(|i| (i % 256) as u8).collect();
    let sftp_round_trip = async {
        use russh_sftp::protocol::{FileAttributes, OpenFlags};
        let h = sftp
            .open(
                path.clone(),
                OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNCATE,
                FileAttributes::empty(),
            )
            .await
            .unwrap();
        sftp.write(h.handle.clone(), 0, payload.clone())
            .await
            .unwrap();
        sftp.close(h.handle).await.unwrap();
        let h = sftp
            .open(path.clone(), OpenFlags::READ, FileAttributes::empty())
            .await
            .unwrap();
        let mut back = Vec::new();
        while back.len() < payload.len() {
            let d = sftp
                .read(h.handle.clone(), back.len() as u64, 32_768)
                .await
                .unwrap();
            back.extend(d.data);
        }
        sftp.close(h.handle).await.unwrap();
        sftp.remove(path.clone()).await.unwrap();
        back
    };
    let back = tokio::time::timeout(Duration::from_secs(10), sftp_round_trip)
        .await
        .expect("SFTP stalled under flood");
    assert!(back == payload);
    eprintln!("flood: SFTP round trip took {:?}", started.elapsed());

    // (c) terminal 2 echoes within 1 s.
    for i in 0..3 {
        let marker = format!("echo-{i}-{}", uuid::Uuid::new_v4().simple());
        let from = t2.seen.len();
        let started = Instant::now();
        t2.write(marker.as_bytes()).await;
        t2.wait_for(from, marker.as_bytes(), Duration::from_secs(1))
            .await;
        eprintln!("flood: terminal 2 echo took {:?}", started.elapsed());
        t2.write(b"\x15").await; // Ctrl+U: drop the typed line
    }
    assert!(t1.term.flow_stats().dropping, "still flooding during a–c");

    // (d) stop `yes`, let the UI catch up: `outputDropped` arrives after all kept output and
    // before anything newer.
    t1.write(b"\x03").await;
    // From now on everything consumed is acked (the 4 MiB delivered meanwhile first).
    t1.ack = true;
    let ev = t1
        .wait_event(STEP, |e| matches!(e, TermEvent::OutputDropped { .. }))
        .await;
    match ev {
        TermEvent::OutputDropped { bytes } => {
            assert!(bytes > 0);
            eprintln!("flood: {bytes} bytes discarded");
        }
        _ => unreachable!(),
    }
    let after_marker = t1.seen.len();
    t1.sync("after-flood").await;
    assert!(t1.seen.len() > after_marker);
    assert!(!t1.term.flow_stats().dropping);
    e.manager.close(&s.id, None).await;
}

/// A paste larger than 64 KiB, sent as the UI does (64 KiB chunks plus a short tail, the
/// commands not awaited, a resize in between), reaches the shell intact and in order: the
/// remote `sha256sum` of exactly that many raw bytes matches.
#[tokio::test(flavor = "multi_thread")]
async fn large_paste_arrives_in_order() {
    let e = env(support::USER);
    let s = connect(&e).await;
    let mut t = open_term(&e, &s, size(80, 24), None).await.unwrap();
    t.sync("paste").await;

    // Printable, position-dependent content (a reordered chunk changes the hash).
    let paste: Vec<u8> = (0..(4 * 64 * 1024 + 123))
        .map(|i: usize| b'a' + u8::try_from((i / 7 + i / 4099) % 26).unwrap())
        .collect();
    let digest = ring::digest::digest(&ring::digest::SHA256, &paste);
    let hex: String = digest.as_ref().iter().map(|b| format!("{b:02x}")).collect();

    // Raw mode (no line-length limit, no echo); "ready" only once stty has run.
    let from = t.seen.len();
    t.write(
        format!(
            "stty raw -echo; echo rea''dy; head -c {} | sha256sum; stty sane\r",
            paste.len()
        )
        .as_bytes(),
    )
    .await;
    t.wait_for(from, b"ready", STEP).await;

    let from = t.seen.len();
    for (n, chunk) in paste.chunks(64 * 1024).enumerate() {
        // The command path: queue and return, no waiting between chunks.
        t.term.enqueue_write(chunk.to_vec()).expect("term_write");
        if n == 1 {
            t.term.resize(size(100, 30));
        }
    }
    t.wait_for(from, hex.as_bytes(), STEP).await;
    t.sync("paste-done").await;
    e.manager.close(&s.id, None).await;
}
