//! Integration cases (§10.2) against the Docker sshd: listing (truncation, symlinks,
//! lossy names), file operations, text reads (encodings, binary, size limit), saves (atomic with
//! mode kept, in place when the owner differs, group kept or in place, rename fallback, symlink,
//! conflict detection), uploads (conflict → `transfer_retry` with overwrite, cancel cleanup) and
//! downloads (`.part` + rename, Mark-of-the-Web on Windows, cancel cleanup).
//! Headless: `SessionManager` + in-memory sinks, private temp home (as `it_terminal.rs`).
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
use std::time::Duration;

use shellfin_lib::error::{AppError, ErrorCode};
use shellfin_lib::ipc::dto::{
    Eol, FileKind, OpenSessionRequest, Prompt, PromptAnswer, SaveRequest, SaveStrategy,
    SessionEvent, SessionState, Target, TransferEvent,
};
use shellfin_lib::known_hosts::TrustOnceApprovals;
use shellfin_lib::session::manager::SessionManager;
use shellfin_lib::session::session::{ConnectCtxTemplate, SessionDeps, SessionHandle};
use shellfin_lib::sftp::client::Sftp;
use shellfin_lib::sftp::save;
use shellfin_lib::sftp::transfer::{TransferSink, TransferSpec};
use shellfin_lib::ssh::prompt::EventSink;
use shellfin_lib::sshconfig::ConfigEnv;
use shellfin_lib::sshconfig::service::SshConfigService;
use shellfin_lib::storage::Stores;
use tokio::sync::mpsc;

const LIMIT: u64 = 5 * 1024 * 1024;

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
    manager: SessionManager,
    stores: Arc<Stores>,
    key: std::path::PathBuf,
}

fn q(p: &Path) -> String {
    format!("\"{}\"", p.display().to_string().replace('\\', "/"))
}

fn env(user: &str) -> Env {
    let s = support::sshd();
    env_at(user, &s.host, s.port)
}

/// [`env`] with `bastion` reached at `host:port` (the throughput benchmark's delay proxy).
fn env_at(user: &str, host: &str, port: u16) -> Env {
    let (key_dir, key) = support::private_key_copy(support::KEY_RSA_OPENSSH);
    let config = format!(
        "Host bastion\n  HostName {host}\n  Port {port}\n  User {user}\n  IdentityFile {}\n  \
         IdentitiesOnly yes\nHost *\n  IdentityAgent none\n",
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
        _dir: dir,
        _key_dir: key_dir,
        stores,
        key,
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
}

async fn connect(e: &Env) -> Session {
    connect_target(
        e,
        Target::SshConfig {
            alias: "bastion".into(),
        },
    )
    .await
    .0
}

/// Connect to `target`; also returns the session's event receiver (for later events).
async fn connect_target(
    e: &Env,
    target: Target,
) -> (Session, mpsc::UnboundedReceiver<SessionEvent>) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let id = e
        .manager
        .open(
            OpenSessionRequest {
                target,
                probe_only: false,
            },
            Arc::new(ChanSink(tx)),
        )
        .await
        .expect("session_open");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    loop {
        let ev = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("timed out waiting for connected")
            .expect("event channel closed");
        match ev {
            SessionEvent::Prompt {
                prompt: p @ Prompt::HostKeyUnknown { .. },
            } => e
                .manager
                .answer_prompt(
                    &id,
                    prompt_id(&p),
                    PromptAnswer::HostKey {
                        accept: true,
                        remember: false,
                    },
                )
                .unwrap(),
            SessionEvent::State {
                state: SessionState::Connected { info },
            } => {
                assert!(info.sftp_available);
                // The fixed `id -u; id -G` exec reports the account's real ids.
                let ident = info.remote_identity.expect("remote identity");
                let want_uid: u32 = root_sh(&format!("id -u {}", support::USER))
                    .trim()
                    .parse()
                    .unwrap();
                let mut want_gids: Vec<u32> = root_sh(&format!("id -G {}", support::USER))
                    .split_whitespace()
                    .map(|g| g.parse().unwrap())
                    .collect();
                let mut got_gids = ident.gids.clone();
                want_gids.sort_unstable();
                got_gids.sort_unstable();
                assert_eq!((ident.uid, got_gids), (want_uid, want_gids));
                break;
            }
            SessionEvent::State {
                state: SessionState::Disconnected { error, .. },
            } => panic!("disconnected: {error:?}"),
            _ => {}
        }
    }
    let handle = e.manager.get(&id).unwrap();
    (Session { id, handle }, rx)
}

async fn sftp(s: &Session) -> Sftp {
    Sftp::for_session(&s.handle).await.expect("sftp")
}

/// A fresh directory in the user's home, owned by the user (created over SFTP).
async fn workdir(sftp: &Sftp, user: &str) -> String {
    let dir = format!("/home/{user}/it-sftp-{}", uuid::Uuid::new_v4().simple());
    sftp.mkdir(&dir).await.expect("mkdir workdir");
    dir
}

/// Run `cmd` with `sh -c` in the bastion as root.
fn root_sh(cmd: &str) -> String {
    support::docker_exec_ok(support::BASTION, &["sh", "-c", cmd])
}

/// Run `cmd` with `sh -c` in the bastion as `user`.
fn user_sh(user: &str, cmd: &str) -> String {
    support::docker_exec_ok(
        support::BASTION,
        &["runuser", "-u", user, "--", "sh", "-c", cmd],
    )
}

fn code(e: &AppError) -> &ErrorCode {
    &e.kind
}

async fn write_remote(sftp: &Sftp, path: &str, bytes: &[u8]) {
    let (target, cur) = save::resolve_target(sftp, path).await.unwrap();
    let mut progress = |_| {};
    let never = || false;
    let mut hooks = save::Hooks {
        progress: &mut progress,
        cancelled: &never,
        cancel_signal: None,
        window_pool: None,
    };
    save::write_file(
        sftp,
        &save::WriteSpec {
            target: &target,
            cur: cur.as_ref(),
            exists: cur.is_some(),
            new_mode: 0o644,
            replace: true,
            times: None,
            recheck: false,
        },
        &mut save::BytesSource::new(bytes),
        &mut hooks,
        &mut Vec::new(),
    )
    .await
    .unwrap();
}

fn save_req(path: &str, text: &str, expected: Option<shellfin_lib::ipc::dto::Etag>) -> SaveRequest {
    SaveRequest {
        path: path.to_owned(),
        text: text.to_owned(),
        encoding: "utf-8".into(),
        has_bom: false,
        eol: Eol::Lf,
        expected,
        force: false,
        expected_target: None,
    }
}

/// `stat -c '%U:%G %a'` of a remote path.
fn owner_mode(path: &str) -> String {
    root_sh(&format!("stat -c '%U:%G %a' '{path}'"))
        .trim()
        .to_owned()
}

/// Remote temp files left in `dir` (`.<name>.sshb-*.tmp`, `*.sshb-bak`).
fn leftovers(dir: &str) -> String {
    root_sh(&format!("ls -A '{dir}' | grep -E 'sshb-' || true"))
        .trim()
        .to_owned()
}

#[derive(Default)]
struct Recorder(Mutex<Vec<TransferEvent>>);
impl TransferSink for Recorder {
    fn send(&self, event: TransferEvent) {
        self.0.lock().unwrap().push(event);
    }
}

impl Recorder {
    /// Wait for the terminal event (`done`/`failed`/`cancelled`) of transfer `id`.
    async fn finished(&self, id: &str) -> TransferEvent {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            let hit = self.0.lock().unwrap().iter().find_map(|e| match e {
                TransferEvent::Done { transfer_id, .. }
                | TransferEvent::Failed { transfer_id, .. }
                | TransferEvent::Cancelled { transfer_id }
                | TransferEvent::Skipped { transfer_id }
                    if transfer_id == id =>
                {
                    Some(e.clone())
                }
                _ => None,
            });
            if let Some(ev) = hit {
                return ev;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "transfer {id} did not finish: {:?}",
                self.0.lock().unwrap()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn of(&self, id: &str) -> Vec<TransferEvent> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|e| match e {
                TransferEvent::Started { transfer_id, .. }
                | TransferEvent::Progress { transfer_id, .. }
                | TransferEvent::Done { transfer_id, .. }
                | TransferEvent::Failed { transfer_id, .. }
                | TransferEvent::Cancelled { transfer_id }
                | TransferEvent::Skipped { transfer_id } => transfer_id == id,
            })
            .cloned()
            .collect()
    }
}

// ---------------------------------------------------------------------------------------------
// cases
// ---------------------------------------------------------------------------------------------

/// §6.2 + §4.2 file ops: listing with symlinks (incl. broken), lossy names, `notADirectory`,
/// `remoteNotFound`; mkdir/create never replace; rename never overwrites; delete of a non-empty
/// directory is refused.
#[tokio::test(flavor = "multi_thread")]
async fn listing_and_file_operations() {
    let e = env(support::USER);
    let s = connect(&e).await;
    let sftp = sftp(&s).await;
    let dir = workdir(&sftp, support::USER).await;

    let f = sftp.create_file(&format!("{dir}/a.txt")).await.unwrap();
    assert_eq!((f.kind, f.size), (FileKind::File, Some(0)));
    assert_eq!(f.permissions.map(|p| p & 0o777), Some(0o644));
    let err = sftp.create_file(&format!("{dir}/a.txt")).await.unwrap_err();
    assert!(
        matches!(code(&err), ErrorCode::RemoteExists { .. }),
        "{err:?}"
    );
    let sub = sftp.mkdir(&format!("{dir}/sub")).await.unwrap();
    assert_eq!(sub.kind, FileKind::Dir);
    let err = sftp.mkdir(&format!("{dir}/sub")).await.unwrap_err();
    assert!(
        matches!(code(&err), ErrorCode::RemoteExists { .. }),
        "{err:?}"
    );

    // Symlinks (to a dir, to a file, dangling) and a non-UTF-8 name, made in the container.
    user_sh(
        support::USER,
        &format!(
            "cd '{dir}' && ln -s sub link-dir && ln -s a.txt link-file && ln -s nowhere dangling \
             && touch \"$(printf 'bad-\\377-name')\""
        ),
    );
    let listing = sftp.list(&dir).await.unwrap();
    assert!(!listing.truncated);
    let by_name = |n: &str| listing.entries.iter().find(|e| e.name == n).cloned();
    assert!(by_name(".").is_none() && by_name("..").is_none());
    let l = by_name("link-dir").unwrap();
    assert_eq!(
        (l.kind, l.link_target_kind),
        (FileKind::Symlink, Some(FileKind::Dir))
    );
    assert_eq!(
        by_name("link-file").unwrap().link_target_kind,
        Some(FileKind::File)
    );
    assert_eq!(
        by_name("dangling").unwrap().link_target_kind,
        Some(FileKind::Other)
    );
    assert_eq!(by_name("a.txt").unwrap().path, format!("{dir}/a.txt"));
    let lossy: Vec<_> = listing.entries.iter().filter(|e| e.lossy_name).collect();
    assert_eq!(lossy.len(), 1, "{:?}", listing.entries);
    assert!(lossy[0].name.contains('\u{FFFD}'));
    // Operations on lossy paths are refused by the command layer's path check.
    assert!(shellfin_lib::sftp::path::validate_exact(&lossy[0].path, "path").is_err());

    let err = sftp.list(&format!("{dir}/a.txt")).await.unwrap_err();
    assert!(
        matches!(code(&err), ErrorCode::NotADirectory { .. }),
        "{err:?}"
    );
    let err = sftp.list(&format!("{dir}/missing")).await.unwrap_err();
    assert!(
        matches!(code(&err), ErrorCode::RemoteNotFound { .. }),
        "{err:?}"
    );
    let err = sftp.entry(&format!("{dir}/missing")).await.unwrap_err();
    assert!(
        matches!(code(&err), ErrorCode::RemoteNotFound { .. }),
        "{err:?}"
    );

    // Rename never overwrites.
    sftp.create_file(&format!("{dir}/b.txt")).await.unwrap();
    let err = sftp
        .rename(&format!("{dir}/a.txt"), &format!("{dir}/b.txt"))
        .await
        .unwrap_err();
    assert!(
        matches!(code(&err), ErrorCode::RemoteExists { .. }),
        "{err:?}"
    );
    let moved = sftp
        .rename(&format!("{dir}/a.txt"), &format!("{dir}/c.txt"))
        .await
        .unwrap();
    assert_eq!(moved.name, "c.txt");

    // Delete: file, symlink (not its target), empty dir; non-empty dir refused.
    sftp.delete(&format!("{dir}/link-file")).await.unwrap();
    assert!(sftp.entry(&format!("{dir}/c.txt")).await.is_ok());
    sftp.create_file(&format!("{dir}/sub/inner")).await.unwrap();
    let err = sftp.delete(&format!("{dir}/sub")).await.unwrap_err();
    assert!(
        matches!(code(&err), ErrorCode::DirectoryNotEmpty { .. }),
        "{err:?}"
    );
    sftp.delete(&format!("{dir}/sub/inner")).await.unwrap();
    sftp.delete(&format!("{dir}/sub")).await.unwrap();
    let real = sftp.realpath(&format!("{dir}/./c.txt")).await.unwrap();
    assert_eq!(real, format!("{dir}/c.txt"));

    root_sh(&format!("rm -rf '{dir}'"));
    e.manager.close(&s.id, None).await;
}

/// §6.2: a directory of 100 001 entries is listed up to 100 000 with `truncated: true`.
#[tokio::test(flavor = "multi_thread")]
async fn listing_is_truncated_at_100k_entries() {
    let e = env(support::USER);
    let s = connect(&e).await;
    let sftp = sftp(&s).await;
    let dir = format!("/tmp/it-big-{}", uuid::Uuid::new_v4().simple());
    user_sh(
        support::USER,
        &format!("mkdir '{dir}' && cd '{dir}' && seq -w 1 100001 | xargs touch"),
    );
    let started = std::time::Instant::now();
    let listing = sftp.list(&dir).await.unwrap();
    eprintln!("100 001 entries listed in {:?}", started.elapsed());
    assert!(listing.truncated);
    assert_eq!(listing.entries.len(), 100_000);
    root_sh(&format!("rm -rf '{dir}'"));
    e.manager.close(&s.id, None).await;
}

/// §6.3: encodings, BOM, EOL, binary, size limit, directories, etag.
#[tokio::test(flavor = "multi_thread")]
async fn read_text_variants() {
    let e = env(support::USER);
    let s = connect(&e).await;
    let sftp = sftp(&s).await;
    let dir = workdir(&sftp, support::USER).await;
    let p = |n: &str| format!("{dir}/{n}");

    write_remote(&sftp, &p("crlf.txt"), b"a\r\nb\r\nc\n").await;
    let doc = sftp.read_text(&p("crlf.txt"), None, LIMIT).await.unwrap();
    assert_eq!(doc.text, "a\nb\nc\n");
    assert_eq!(
        (doc.eol, doc.mixed_eol, doc.has_bom),
        (Eol::Crlf, true, false)
    );
    assert_eq!(doc.etag.size, 8);
    assert_eq!(doc.etag.sha256.len(), 64);
    assert_eq!(doc.permissions.map(|m| m & 0o777), Some(0o644));

    write_remote(&sftp, &p("bom.txt"), b"\xEF\xBB\xBFhello").await;
    let doc = sftp.read_text(&p("bom.txt"), None, LIMIT).await.unwrap();
    assert_eq!((doc.text.as_str(), doc.has_bom), ("hello", true));

    write_remote(&sftp, &p("bin"), b"\x7fELF\0\0\x01").await;
    let err = sftp.read_text(&p("bin"), None, LIMIT).await.unwrap_err();
    assert!(
        matches!(code(&err), ErrorCode::FileIsBinary { .. }),
        "{err:?}"
    );

    let cp1251 = b"\xCF\xF0\xE8\xE2\xB3\xF2\n";
    write_remote(&sftp, &p("ua.txt"), cp1251).await;
    let err = sftp.read_text(&p("ua.txt"), None, LIMIT).await.unwrap_err();
    assert!(
        matches!(code(&err), ErrorCode::DecodeFailed { encoding, .. } if encoding == "utf-8"),
        "{err:?}"
    );
    let doc = sftp
        .read_text(&p("ua.txt"), Some("windows-1251"), LIMIT)
        .await
        .unwrap();
    assert_eq!(
        (doc.text.as_str(), doc.encoding.as_str()),
        ("Привіт\n", "windows-1251")
    );

    // 300 KiB file (pipelined reads across many chunks) against a 256 KiB limit.
    let big: Vec<u8> = (0..300 * 1024).map(|i| b'a' + (i % 26) as u8).collect();
    write_remote(&sftp, &p("big.txt"), &big).await;
    let doc = sftp.read_text(&p("big.txt"), None, LIMIT).await.unwrap();
    assert_eq!(doc.text.as_bytes(), &big[..]);
    let err = sftp
        .read_text(&p("big.txt"), None, 256 * 1024)
        .await
        .unwrap_err();
    assert!(
        matches!(code(&err), ErrorCode::FileTooLarge { size, limit, .. } if *size == 300 * 1024 && *limit == 256 * 1024),
        "{err:?}"
    );
    let err = sftp.read_text(&dir, None, LIMIT).await.unwrap_err();
    assert!(
        matches!(code(&err), ErrorCode::IsADirectory { .. }),
        "{err:?}"
    );

    root_sh(&format!("rm -rf '{dir}'"));
    e.manager.close(&s.id, None).await;
}

/// §6.4: atomic save keeps the mode (fsetstat before close), works through a symlink, detects a
/// conflict (size/mtime + SHA-256), ignores a touch-only change, honours `force`; the rename
/// fallback (no posix-rename) leaves no backup behind.
#[tokio::test(flavor = "multi_thread")]
async fn save_atomic_conflicts_symlink_and_fallback() {
    let e = env(support::USER);
    let s = connect(&e).await;
    let mut sftp = sftp(&s).await;
    assert!(sftp.posix_rename && sftp.fsync);
    let dir = workdir(&sftp, support::USER).await;
    let path = format!("{dir}/app.yml");
    write_remote(&sftp, &path, b"v: 1\n").await;
    user_sh(support::USER, &format!("chmod 0640 '{path}'"));

    let doc = sftp.read_text(&path, None, LIMIT).await.unwrap();
    let res = save::save_text(
        &sftp,
        &save_req(&path, "v: 2\n", Some(doc.etag.clone())),
        LIMIT,
    )
    .await
    .unwrap();
    assert_eq!(res.strategy, SaveStrategy::Atomic);
    assert!(res.warnings.is_empty());
    assert_eq!(user_sh(support::USER, &format!("cat '{path}'")), "v: 2\n");
    assert_eq!(owner_mode(&path), "tester:tester 640");
    assert_eq!(leftovers(&dir), "");

    // Conflict: changed on the server since it was read.
    user_sh(
        support::USER,
        &format!("printf 'v: 3 from server\\n' > '{path}'"),
    );
    let err = save::save_text(
        &sftp,
        &save_req(&path, "v: 4\n", Some(res.etag.clone())),
        LIMIT,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(code(&err), ErrorCode::RemoteChanged { .. }),
        "{err:?}"
    );
    let mut forced = save_req(&path, "v: 4\n", Some(res.etag.clone()));
    forced.force = true;
    let res = save::save_text(&sftp, &forced, LIMIT).await.unwrap();
    // Touch only (mtime changes, same bytes): no conflict.
    user_sh(support::USER, &format!("touch -d '2001-01-01' '{path}'"));
    save::save_text(&sftp, &save_req(&path, "v: 5\n", Some(res.etag)), LIMIT)
        .await
        .unwrap();
    assert_eq!(user_sh(support::USER, &format!("cat '{path}'")), "v: 5\n");

    // A same-size edit within the same second (size and mtime unchanged) is a conflict.
    let doc = sftp.read_text(&path, None, LIMIT).await.unwrap();
    user_sh(
        support::USER,
        &format!(
            "m=$(stat -c %Y '{path}') && printf 'v: X\\n' > '{path}' && touch -d @$m '{path}'"
        ),
    );
    let now = sftp.stat(&path).await.unwrap();
    assert_eq!(
        (now.size, now.mtime.map(i64::from)),
        (Some(doc.etag.size), Some(doc.etag.mtime))
    );
    let err = save::save_text(
        &sftp,
        &save_req(&path, "v: 6\n", Some(doc.etag.clone())),
        LIMIT,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(code(&err), ErrorCode::RemoteChanged { .. }),
        "{err:?}"
    );
    assert_eq!(user_sh(support::USER, &format!("cat '{path}'")), "v: X\n");
    // A change after the conflict check, before the rename, is caught by the re-stat;
    // the server's version stays and no temp file is left.
    let doc = sftp.read_text(&path, None, LIMIT).await.unwrap();
    let edit = format!("printf 'v: server edit\\n' > '{path}'");
    sftp.commit_hook = Some(Arc::new(move || {
        user_sh(support::USER, &edit);
    }));
    let err = save::save_text(&sftp, &save_req(&path, "v: 6\n", Some(doc.etag)), LIMIT)
        .await
        .unwrap_err();
    sftp.commit_hook = None;
    assert!(
        matches!(code(&err), ErrorCode::RemoteChanged { .. }),
        "{err:?}"
    );
    assert_eq!(
        user_sh(support::USER, &format!("cat '{path}'")),
        "v: server edit\n"
    );
    assert_eq!(leftovers(&dir), "");
    let doc = sftp.read_text(&path, None, LIMIT).await.unwrap();
    let res = save::save_text(&sftp, &save_req(&path, "v: 5\n", Some(doc.etag)), LIMIT)
        .await
        .unwrap();
    assert_eq!(res.strategy, SaveStrategy::Atomic);
    assert_eq!(user_sh(support::USER, &format!("cat '{path}'")), "v: 5\n");

    // Through a symlink: the target changes, the link stays a link.
    user_sh(
        support::USER,
        &format!("cd '{dir}' && ln -s app.yml link.yml"),
    );
    let link = format!("{dir}/link.yml");
    let doc = sftp.read_text(&link, None, LIMIT).await.unwrap();
    // Without `expectedTarget` the backend expects `path` itself, which a link is not
    // (fail-safe); the editor sends the `resolvedPath` it got from the read.
    let mut req = save_req(&link, "v: 6\n", Some(doc.etag));
    let err = save::save_text(&sftp, &req, LIMIT).await.unwrap_err();
    assert!(
        matches!(code(&err), ErrorCode::RemoteChanged { .. }),
        "{err:?}"
    );
    req.expected_target = doc.resolved_path.clone();
    save::save_text(&sftp, &req, LIMIT).await.unwrap();
    assert_eq!(user_sh(support::USER, &format!("cat '{path}'")), "v: 6\n");
    assert_eq!(
        root_sh(&format!("stat -c %F '{link}'")).trim(),
        "symbolic link"
    );

    // Rename fallback (server without posix-rename): target → bak, tmp → target, bak removed.
    sftp.posix_rename = false;
    let doc = sftp.read_text(&path, None, LIMIT).await.unwrap();
    let res = save::save_text(&sftp, &save_req(&path, "v: 7\n", Some(doc.etag)), LIMIT)
        .await
        .unwrap();
    assert_eq!(res.strategy, SaveStrategy::RenameFallback);
    assert_eq!(user_sh(support::USER, &format!("cat '{path}'")), "v: 7\n");
    assert_eq!(owner_mode(&path), "tester:tester 640");
    assert_eq!(leftovers(&dir), "");

    // A stale backup under the old fixed name (one that could not be removed) does not
    // block later fallback saves, and a 250-byte name still fits (random, capped backup name).
    let stale = format!("{path}.sshb-bak");
    root_sh(&format!(
        "printf 'stale\\n' > '{stale}' && chmod 0444 '{stale}'"
    ));
    for v in ["v: 8\n", "v: 9\n"] {
        let doc = sftp.read_text(&path, None, LIMIT).await.unwrap();
        let res = save::save_text(&sftp, &save_req(&path, v, Some(doc.etag)), LIMIT)
            .await
            .unwrap();
        assert_eq!(res.strategy, SaveStrategy::RenameFallback);
        assert_eq!(user_sh(support::USER, &format!("cat '{path}'")), v);
    }
    assert_eq!(root_sh(&format!("cat '{stale}'")), "stale\n");
    root_sh(&format!("rm -f '{stale}'"));
    let long = format!("{dir}/{}", "n".repeat(250));
    user_sh(support::USER, &format!("printf 'l1\\n' > '{long}'"));
    let doc = sftp.read_text(&long, None, LIMIT).await.unwrap();
    let res = save::save_text(&sftp, &save_req(&long, "l2\n", Some(doc.etag)), LIMIT)
        .await
        .unwrap();
    assert_eq!(res.strategy, SaveStrategy::RenameFallback);
    assert_eq!(user_sh(support::USER, &format!("cat '{long}'")), "l2\n");
    assert_eq!(leftovers(&dir), "");
    // The returned etag is the saved file's (size we wrote, its mtime).
    let after = sftp.stat(&long).await.unwrap();
    assert_eq!(res.etag.size, 3);
    assert_eq!(Some(res.etag.mtime), after.mtime.map(i64::from));
    user_sh(support::USER, &format!("rm -f '{long}'"));

    // A new file (expected = null) is created exclusively; the file deleted meanwhile with an
    // etag ⇒ remoteChanged.
    let fresh = format!("{dir}/new.txt");
    let res = save::save_text(&sftp, &save_req(&fresh, "x\n", None), LIMIT)
        .await
        .unwrap();
    assert_eq!(owner_mode(&fresh), "tester:tester 644");
    user_sh(support::USER, &format!("rm '{fresh}'"));
    let err = save::save_text(&sftp, &save_req(&fresh, "y\n", Some(res.etag)), LIMIT)
        .await
        .unwrap_err();
    assert!(
        matches!(
            code(&err),
            ErrorCode::RemoteChanged {
                size: 0,
                mtime: 0,
                ..
            }
        ),
        "{err:?}"
    );

    root_sh(&format!("rm -rf '{dir}'"));
    e.manager.close(&s.id, None).await;
}

/// §6.4 ownership: a file owned by another user (group-writable) is written in place and keeps
/// its owner; a file in a secondary group of the user keeps that group (atomic); a group the user
/// is not in ⇒ in place, group kept.
#[tokio::test(flavor = "multi_thread")]
async fn save_keeps_owner_and_group() {
    let e = env(support::USER);
    let s = connect(&e).await;
    let sftp = sftp(&s).await;
    let dir = workdir(&sftp, support::USER).await;

    let foreign = format!("{dir}/foreign.conf");
    root_sh(&format!(
        "printf 'a\\n' > '{foreign}' && chown {}:tester '{foreign}' && chmod 0664 '{foreign}'",
        support::USER_2FA
    ));
    let doc = sftp.read_text(&foreign, None, LIMIT).await.unwrap();
    let res = save::save_text(&sftp, &save_req(&foreign, "b\n", Some(doc.etag)), LIMIT)
        .await
        .unwrap();
    assert_eq!(res.strategy, SaveStrategy::InPlace);
    assert_eq!(
        owner_mode(&foreign),
        format!("{}:tester 664", support::USER_2FA)
    );
    assert_eq!(root_sh(&format!("cat '{foreign}'")), "b\n");

    let shared = format!("{dir}/shared.conf");
    root_sh(&format!(
        "printf 'a\\n' > '{shared}' && chown tester:shellfin-shared '{shared}' && chmod 0660 '{shared}'"
    ));
    let doc = sftp.read_text(&shared, None, LIMIT).await.unwrap();
    let res = save::save_text(&sftp, &save_req(&shared, "b\n", Some(doc.etag)), LIMIT)
        .await
        .unwrap();
    assert_eq!(res.strategy, SaveStrategy::Atomic);
    assert_eq!(owner_mode(&shared), "tester:shellfin-shared 660");

    let rootgrp = format!("{dir}/rootgroup.conf");
    root_sh(&format!(
        "printf 'a\\n' > '{rootgrp}' && chown tester:root '{rootgrp}' && chmod 0644 '{rootgrp}'"
    ));
    let doc = sftp.read_text(&rootgrp, None, LIMIT).await.unwrap();
    let res = save::save_text(&sftp, &save_req(&rootgrp, "b\n", Some(doc.etag)), LIMIT)
        .await
        .unwrap();
    assert_eq!(res.strategy, SaveStrategy::InPlace);
    assert_eq!(owner_mode(&rootgrp), "tester:root 644");
    assert_eq!(root_sh(&format!("cat '{rootgrp}'")), "b\n");
    assert_eq!(leftovers(&dir), "");

    root_sh(&format!("rm -rf '{dir}'"));
    e.manager.close(&s.id, None).await;
}

/// Uploads set the remote mtime/atime from the local file and
/// downloads the local ones from the remote file (unless `preserveTimestamps` is off); a
/// download chosen in the Save dialog is never skipped by `onConflict = skip`; listings carry
/// the owner and group names.
#[tokio::test(flavor = "multi_thread")]
async fn settings_v3_timestamps_skip_fix_and_owner_names() {
    use shellfin_lib::commands::transfer::download_conflict;
    use shellfin_lib::ipc::dto::ConflictPolicy;
    use std::time::{Duration as StdDuration, UNIX_EPOCH};

    let e = env(support::USER);
    let s = connect(&e).await;
    let sftp = sftp(&s).await;
    let dir = workdir(&sftp, support::USER).await;
    let local = tempfile::tempdir().unwrap();
    let transfers = e.manager.transfers().clone();
    let rec = Arc::new(Recorder::default());
    let sink: Arc<dyn TransferSink> = rec.clone();
    let remote_mtime = |p: &str| -> u64 {
        root_sh(&format!("stat -c %Y '{p}'"))
            .trim()
            .parse()
            .unwrap()
    };
    let remote_atime = |p: &str| -> u64 {
        root_sh(&format!("stat -c %X '{p}'"))
            .trim()
            .parse()
            .unwrap()
    };
    let local_mtime = |p: &std::path::Path| -> u64 {
        std::fs::metadata(p)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    };

    // Upload (new file, atomic path): remote times = local times.
    let src = local.path().join("old.txt");
    std::fs::write(&src, b"from 2001\n").unwrap();
    let at = |s: u64| UNIX_EPOCH + StdDuration::from_secs(s);
    std::fs::File::options()
        .write(true)
        .open(&src)
        .unwrap()
        .set_times(
            std::fs::FileTimes::new()
                .set_modified(at(1_000_000_000))
                .set_accessed(at(1_000_000_100)),
        )
        .unwrap();
    let id = transfers.start(
        s.handle.clone(),
        TransferSpec::Upload {
            local: src.clone(),
            remote_dir: dir.clone(),
        },
        false,
        sink.clone(),
    );
    match rec.finished(&id).await {
        TransferEvent::Done { entry, .. } => {
            assert_eq!(entry.unwrap().mtime, Some(1_000_000_000));
        }
        other => panic!("{other:?}"),
    }
    let remote = format!("{dir}/old.txt");
    assert_eq!(remote_mtime(&remote), 1_000_000_000);
    assert_eq!(remote_atime(&remote), 1_000_000_100);

    // Overwrite of the existing file (atomic replace): the times follow again.
    let id = transfers.start(
        s.handle.clone(),
        TransferSpec::Upload {
            local: src.clone(),
            remote_dir: dir.clone(),
        },
        true,
        sink.clone(),
    );
    assert!(matches!(
        rec.finished(&id).await,
        TransferEvent::Done { .. }
    ));
    assert_eq!(remote_mtime(&remote), 1_000_000_000);

    // Download: local times = remote times.
    let theirs = format!("{dir}/remote.txt");
    user_sh(
        support::USER,
        &format!("printf 'r' > '{theirs}' && touch -m -d @1234567890 '{theirs}'"),
    );
    let chosen = local.path().join("remote.txt");
    let id = transfers.start(
        s.handle.clone(),
        TransferSpec::Download {
            remote: theirs.clone(),
            local: chosen.clone(),
        },
        true,
        sink.clone(),
    );
    assert!(matches!(
        rec.finished(&id).await,
        TransferEvent::Done { .. }
    ));
    assert_eq!(local_mtime(&chosen), 1_234_567_890);

    // Bug fix: a dialog download over an existing file with `onConflict = skip` is written.
    std::fs::write(&chosen, b"old local").unwrap();
    let id = transfers.start_with(
        s.handle.clone(),
        TransferSpec::Download {
            remote: theirs.clone(),
            local: chosen.clone(),
        },
        download_conflict(true, ConflictPolicy::Skip),
        sink.clone(),
    );
    assert!(
        matches!(
            rec.finished(&id).await,
            TransferEvent::Done { bytes: 1, .. }
        ),
        "{:?}",
        rec.of(&id)
    );
    assert_eq!(std::fs::read(&chosen).unwrap(), b"r");

    // Off: neither direction touches the times.
    transfers.set_preserve_timestamps(false);
    let src2 = local.path().join("fresh.txt");
    std::fs::write(&src2, b"x").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&src2)
        .unwrap()
        .set_modified(at(1_000_000_000))
        .unwrap();
    let id = transfers.start(
        s.handle.clone(),
        TransferSpec::Upload {
            local: src2,
            remote_dir: dir.clone(),
        },
        false,
        sink.clone(),
    );
    assert!(matches!(
        rec.finished(&id).await,
        TransferEvent::Done { .. }
    ));
    assert!(remote_mtime(&format!("{dir}/fresh.txt")) > 1_700_000_000);
    let chosen2 = local.path().join("remote2.txt");
    let id = transfers.start(
        s.handle.clone(),
        TransferSpec::Download {
            remote: theirs,
            local: chosen2.clone(),
        },
        true,
        sink.clone(),
    );
    assert!(matches!(
        rec.finished(&id).await,
        TransferEvent::Done { .. }
    ));
    assert!(local_mtime(&chosen2) > 1_700_000_000);
    transfers.set_preserve_timestamps(true);

    // Listings carry owner/group names from the long name; stat has none.
    let listing = sftp.list(&dir).await.unwrap();
    let entry = listing
        .entries
        .iter()
        .find(|x| x.name == "old.txt")
        .unwrap();
    assert_eq!(entry.owner.as_deref(), Some(support::USER));
    assert_eq!(entry.group.as_deref(), Some(support::USER));
    assert!(entry.uid.is_some() && entry.gid.is_some());
    let stat = sftp.entry(&remote).await.unwrap();
    assert_eq!((stat.owner, stat.group), (None, None));
    e.manager.close(&s.id, None).await;
}

/// §6.6: upload without overwrite onto an existing name ⇒ `failed{remoteExists, retryable}`;
/// `transfer_retry(overwrite: true)` replaces it (mode kept); a new upload gets 0644; a directory
/// source ⇒ `localIsADirectory`; download writes `.part` then renames (no `.part` left; MOTW on
/// Windows); cancelling either direction cleans up.
#[tokio::test(flavor = "multi_thread")]
async fn uploads_downloads_retry_and_cancel() {
    let e = env(support::USER);
    let s = connect(&e).await;
    let sftp = sftp(&s).await;
    let dir = workdir(&sftp, support::USER).await;
    let local = tempfile::tempdir().unwrap();
    let transfers = e.manager.transfers().clone();
    let rec = Arc::new(Recorder::default());
    let sink: Arc<dyn TransferSink> = rec.clone();

    // New upload.
    let src = local.path().join("notes.txt");
    std::fs::write(&src, b"local v1\n").unwrap();
    let id = transfers.start(
        s.handle.clone(),
        TransferSpec::Upload {
            local: src.clone(),
            remote_dir: dir.clone(),
        },
        false,
        sink.clone(),
    );
    match rec.finished(&id).await {
        TransferEvent::Done { bytes, entry, .. } => {
            assert_eq!(bytes, 9);
            let entry = entry.unwrap();
            assert_eq!((entry.name.as_str(), entry.size), ("notes.txt", Some(9)));
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        rec.of(&id).first(),
        Some(TransferEvent::Started { total: Some(9), .. })
    ));
    assert_eq!(owner_mode(&format!("{dir}/notes.txt")), "tester:tester 644");

    // Conflict, then retry with overwrite (only this file is resent; mode kept).
    user_sh(support::USER, &format!("chmod 0600 '{dir}/notes.txt'"));
    std::fs::write(&src, b"local v2 longer\n").unwrap();
    let id = transfers.start(
        s.handle.clone(),
        TransferSpec::Upload {
            local: src.clone(),
            remote_dir: dir.clone(),
        },
        false,
        sink.clone(),
    );
    match rec.finished(&id).await {
        TransferEvent::Failed {
            error, retryable, ..
        } => {
            assert!(
                matches!(code(&error), ErrorCode::RemoteExists { .. }),
                "{error:?}"
            );
            assert!(retryable);
        }
        other => panic!("{other:?}"),
    }
    let (session_id, spec) = transfers.take_failed(&id).unwrap();
    assert_eq!(session_id, s.id);
    let retry = transfers.start(s.handle.clone(), spec, true, sink.clone());
    assert!(matches!(
        rec.finished(&retry).await,
        TransferEvent::Done { bytes: 16, .. }
    ));
    assert_eq!(
        user_sh(support::USER, &format!("cat '{dir}/notes.txt'")),
        "local v2 longer\n"
    );
    assert_eq!(owner_mode(&format!("{dir}/notes.txt")), "tester:tester 600");
    assert!(transfers.take_failed(&id).is_err(), "consumed by the retry");

    // A directory as the source.
    let id = transfers.start(
        s.handle.clone(),
        TransferSpec::Upload {
            local: local.path().to_path_buf(),
            remote_dir: dir.clone(),
        },
        false,
        sink.clone(),
    );
    match rec.finished(&id).await {
        TransferEvent::Failed {
            error, retryable, ..
        } => {
            assert!(
                matches!(code(&error), ErrorCode::LocalIsADirectory { .. }),
                "{error:?}"
            );
            assert!(!retryable);
        }
        other => panic!("{other:?}"),
    }

    // Download of a hostile name to a sanitized local name.
    let hostile = format!("{dir}/CON.txt");
    user_sh(
        support::USER,
        &format!("printf 'remote data' > '{hostile}'"),
    );
    let chosen = local
        .path()
        .join(shellfin_lib::sftp::names::sanitize_local_name(&hostile));
    assert!(chosen.ends_with("CON_.txt"));
    let id = transfers.start(
        s.handle.clone(),
        TransferSpec::Download {
            remote: hostile.clone(),
            local: chosen.clone(),
        },
        true,
        sink.clone(),
    );
    assert!(matches!(
        rec.finished(&id).await,
        TransferEvent::Done {
            bytes: 11,
            entry: None,
            ..
        }
    ));
    assert_eq!(std::fs::read(&chosen).unwrap(), b"remote data");
    let names: Vec<String> = std::fs::read_dir(local.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(!names.iter().any(|n| n.ends_with(".part")), "{names:?}");
    #[cfg(windows)]
    {
        let mut ads = chosen.as_os_str().to_owned();
        ads.push(":Zone.Identifier");
        let zone = std::fs::read_to_string(std::path::PathBuf::from(ads)).unwrap();
        assert!(zone.contains("ZoneId=3"));
    }

    // Cancelled download of a large file: no `.part` left.
    let large = format!("{dir}/large.bin");
    user_sh(
        support::USER,
        &format!("head -c 134217728 /dev/urandom > '{large}'"),
    );
    let target = local.path().join("large.bin");
    let id = transfers.start(
        s.handle.clone(),
        TransferSpec::Download {
            remote: large.clone(),
            local: target.clone(),
        },
        true,
        sink.clone(),
    );
    tokio::time::sleep(Duration::from_millis(150)).await;
    transfers.cancel(&id);
    assert!(matches!(
        rec.finished(&id).await,
        TransferEvent::Cancelled { .. }
    ));
    let names: Vec<String> = std::fs::read_dir(local.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        !names.iter().any(|n| n.starts_with("large.bin")),
        "{names:?}"
    );

    // Cancelled upload of a large file: no remote temp file left.
    let big = local.path().join("big-upload.bin");
    std::fs::write(&big, vec![7u8; 128 * 1024 * 1024]).unwrap();
    let id = transfers.start(
        s.handle.clone(),
        TransferSpec::Upload {
            local: big.clone(),
            remote_dir: dir.clone(),
        },
        false,
        sink.clone(),
    );
    tokio::time::sleep(Duration::from_millis(150)).await;
    transfers.cancel(&id);
    assert!(matches!(
        rec.finished(&id).await,
        TransferEvent::Cancelled { .. }
    ));
    assert_eq!(leftovers(&dir), "");
    assert!(sftp.entry(&format!("{dir}/big-upload.bin")).await.is_err());

    // session_close cancels a running upload and removes its temp file first.
    let id = transfers.start(
        s.handle.clone(),
        TransferSpec::Upload {
            local: big.clone(),
            remote_dir: dir.clone(),
        },
        false,
        sink.clone(),
    );
    tokio::time::sleep(Duration::from_millis(150)).await;
    e.manager.close(&s.id, None).await;
    assert!(matches!(
        rec.finished(&id).await,
        TransferEvent::Cancelled { .. }
    ));
    assert_eq!(leftovers(&dir), "");
    assert_eq!(transfers.running_count(), 0);

    root_sh(&format!("rm -rf '{dir}'"));
}

/// An in-place save never writes into a file swapped in after resolution (the entry is
/// replaced by a symlink to the user's own file right before the in-place open) ⇒ `remoteChanged`,
/// the link target untouched. A re-pointed symlink is a conflict (`expectedTarget`), with the new
/// target in the error; `force` writes there. Uploads replace a symlink entry instead of writing
/// through it.
#[tokio::test(flavor = "multi_thread")]
async fn symlink_swaps_are_detected() {
    let e = env(support::USER);
    let s = connect(&e).await;
    let mut sftp = sftp(&s).await;
    let dir = workdir(&sftp, support::USER).await;

    // In-place path: a group-writable file owned by another user.
    let foreign = format!("{dir}/deploy.sh");
    let victim = format!("{dir}/authorized_keys");
    root_sh(&format!(
        "printf 'echo v1\\n' > '{foreign}' && chown {}:tester '{foreign}' && chmod 0664 '{foreign}' \
         && printf 'ssh-ed25519 AAAA mine\\n' > '{victim}' && chown tester:tester '{victim}'",
        support::USER_2FA
    ));
    let doc = sftp.read_text(&foreign, None, LIMIT).await.unwrap();
    assert!(doc.resolved_path.is_none());
    let swap = format!("cd '{dir}' && ln -s authorized_keys .swap && mv -T .swap deploy.sh");
    sftp.in_place_hook = Some(Arc::new(move || {
        root_sh(&swap);
    }));
    let mut req = save_req(&foreign, "ssh-ed25519 AAAA attacker\n", Some(doc.etag));
    req.expected_target = Some(foreign.clone());
    let err = save::save_text(&sftp, &req, LIMIT).await.unwrap_err();
    assert!(
        matches!(code(&err), ErrorCode::RemoteChanged { .. }),
        "{err:?}"
    );
    assert_eq!(
        root_sh(&format!("cat '{victim}'")),
        "ssh-ed25519 AAAA mine\n"
    );
    assert_eq!(leftovers(&dir), "");
    sftp.in_place_hook = None;

    // Re-pointed link: conflict naming the new target; force writes it.
    user_sh(
        support::USER,
        &format!(
            "cd '{dir}' && printf 'a\\n' > real.txt && printf 'b\\n' > other.txt && ln -s real.txt link.txt"
        ),
    );
    let link = format!("{dir}/link.txt");
    let doc = sftp.read_text(&link, None, LIMIT).await.unwrap();
    assert_eq!(
        doc.resolved_path.as_deref(),
        Some(format!("{dir}/real.txt").as_str())
    );
    user_sh(
        support::USER,
        &format!("cd '{dir}' && ln -sfn other.txt link.txt"),
    );
    let mut req = save_req(&link, "c\n", Some(doc.etag.clone()));
    req.expected_target = doc.resolved_path.clone();
    let err = save::save_text(&sftp, &req, LIMIT).await.unwrap_err();
    match code(&err) {
        ErrorCode::RemoteChanged { path, target, .. } => {
            assert_eq!(path, &link);
            assert_eq!(target.as_deref(), Some(format!("{dir}/other.txt").as_str()));
        }
        other => panic!("{other:?}"),
    }
    req.force = true;
    save::save_text(&sftp, &req, LIMIT).await.unwrap();
    assert_eq!(
        user_sh(support::USER, &format!("cat '{dir}/other.txt'")),
        "c\n"
    );
    assert_eq!(
        user_sh(support::USER, &format!("cat '{dir}/real.txt'")),
        "a\n"
    );

    // A regular file at open, a symlink at save: conflict.
    let plain = format!("{dir}/plain.txt");
    user_sh(support::USER, &format!("printf 'p\\n' > '{plain}'"));
    let doc = sftp.read_text(&plain, None, LIMIT).await.unwrap();
    user_sh(
        support::USER,
        &format!("cd '{dir}' && ln -sfn real.txt plain.txt"),
    );
    let mut req = save_req(&plain, "q\n", Some(doc.etag));
    req.expected_target = Some(plain.clone());
    let err = save::save_text(&sftp, &req, LIMIT).await.unwrap_err();
    assert!(
        matches!(
            code(&err),
            ErrorCode::RemoteChanged {
                target: Some(_),
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(
        user_sh(support::USER, &format!("cat '{dir}/real.txt'")),
        "a\n"
    );

    // The same swap when the frontend omits `expectedTarget` (a buffer read from a
    // regular file has `resolvedPath: null`): the backend expects `path` itself. The decoy has
    // the original's size and mtime, so only the target check can catch it.
    let regular = format!("{dir}/regular.txt");
    user_sh(
        support::USER,
        &format!(
            "cd '{dir}' && printf 'r\\n' > regular.txt && printf 'x\\n' > decoy.txt \
             && touch -r regular.txt decoy.txt"
        ),
    );
    let doc = sftp.read_text(&regular, None, LIMIT).await.unwrap();
    assert!(doc.resolved_path.is_none());
    user_sh(
        support::USER,
        &format!("cd '{dir}' && ln -sfn decoy.txt regular.txt"),
    );
    let req = save_req(&regular, "secret\n", Some(doc.etag));
    assert!(req.expected_target.is_none());
    let err = save::save_text(&sftp, &req, LIMIT).await.unwrap_err();
    assert!(
        matches!(
            code(&err),
            ErrorCode::RemoteChanged {
                target: Some(_),
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(
        user_sh(support::USER, &format!("cat '{dir}/decoy.txt'")),
        "x\n"
    );
    // A regular file that stayed regular still saves without `expectedTarget`.
    let steady = format!("{dir}/steady.txt");
    user_sh(support::USER, &format!("printf 's\\n' > '{steady}'"));
    let doc = sftp.read_text(&steady, None, LIMIT).await.unwrap();
    save::save_text(&sftp, &save_req(&steady, "t\n", Some(doc.etag)), LIMIT)
        .await
        .unwrap();
    assert_eq!(user_sh(support::USER, &format!("cat '{steady}'")), "t\n");

    // Upload "Replace" on a symlink replaces the link, never its target.
    user_sh(
        support::USER,
        &format!("cd '{dir}' && printf 'keep\\n' > precious && ln -s precious up.txt"),
    );
    let local = tempfile::tempdir().unwrap();
    let src = local.path().join("up.txt");
    std::fs::write(&src, b"uploaded\n").unwrap();
    let transfers = e.manager.transfers().clone();
    let rec = Arc::new(Recorder::default());
    let id = transfers.start(
        s.handle.clone(),
        TransferSpec::Upload {
            local: src.clone(),
            remote_dir: dir.clone(),
        },
        true,
        rec.clone(),
    );
    assert!(matches!(
        rec.finished(&id).await,
        TransferEvent::Done { .. }
    ));
    assert_eq!(
        user_sh(support::USER, &format!("cat '{dir}/precious'")),
        "keep\n"
    );
    assert_eq!(
        root_sh(&format!("stat -c %F '{dir}/up.txt'")).trim(),
        "regular file"
    );
    assert_eq!(
        user_sh(support::USER, &format!("cat '{dir}/up.txt'")),
        "uploaded\n"
    );

    root_sh(&format!("rm -rf '{dir}'"));
    e.manager.close(&s.id, None).await;
}

/// FIFOs and devices (directly or behind a symlink) are refused for read, save and
/// download with `notARegularFile`, before any request that could block the single-threaded
/// sftp-server or stream forever; the session keeps working afterwards.
#[tokio::test(flavor = "multi_thread")]
async fn fifos_and_devices_are_refused() {
    let e = env(support::USER);
    let s = connect(&e).await;
    let sftp = sftp(&s).await;
    let dir = workdir(&sftp, support::USER).await;
    user_sh(
        support::USER,
        &format!(
            "cd '{dir}' && mkfifo pipe && ln -s pipe pipe-link && ln -s /dev/zero zero \
             && printf 'ok\\n' > plain.txt"
        ),
    );
    let not_regular = |err: &AppError, p: &str| {
        assert!(
            matches!(code(err), ErrorCode::NotARegularFile { path } if path == p),
            "{err:?}"
        );
    };
    let quick = Duration::from_secs(10);
    for name in ["pipe", "pipe-link", "zero"] {
        let p = format!("{dir}/{name}");
        let err = tokio::time::timeout(quick, sftp.read_text(&p, None, LIMIT))
            .await
            .expect("read_text must not block")
            .unwrap_err();
        not_regular(&err, &p);
        let err = tokio::time::timeout(
            quick,
            save::save_text(&sftp, &save_req(&p, "x\n", None), LIMIT),
        )
        .await
        .expect("save must not block")
        .unwrap_err();
        not_regular(&err, &p);
    }
    assert_eq!(
        sftp.read_text("/dev/zero", None, LIMIT)
            .await
            .unwrap_err()
            .kind,
        ErrorCode::NotARegularFile {
            path: "/dev/zero".into()
        }
    );

    // Downloads (the task's own check; the command checks before the Save dialog too).
    let local = tempfile::tempdir().unwrap();
    let transfers = e.manager.transfers().clone();
    let rec = Arc::new(Recorder::default());
    for name in ["pipe-link", "zero"] {
        let target = local.path().join(name);
        let id = transfers.start(
            s.handle.clone(),
            TransferSpec::Download {
                remote: format!("{dir}/{name}"),
                local: target.clone(),
            },
            true,
            rec.clone(),
        );
        match rec.finished(&id).await {
            TransferEvent::Failed {
                error, retryable, ..
            } => {
                not_regular(&error, &format!("{dir}/{name}"));
                assert!(!retryable);
            }
            other => panic!("{other:?}"),
        }
        assert!(!target.exists());
        assert_eq!(
            std::fs::read_dir(local.path()).unwrap().count(),
            0,
            "no .part left"
        );
    }

    // The session still answers.
    let doc = sftp
        .read_text(&format!("{dir}/plain.txt"), None, LIMIT)
        .await
        .unwrap();
    assert_eq!(doc.text, "ok\n");
    root_sh(&format!("rm -rf '{dir}'"));
    e.manager.close(&s.id, None).await;
}

/// `expected: null` onto an existing file ⇒ `remoteChanged` (force writes).
/// Note: oversized text ⇒ `fileTooLarge`. a move into a missing folder names
/// that folder. a burst of uploads runs ≤ 3 at a time and the tree keeps working.
#[tokio::test(flavor = "multi_thread")]
async fn review_3b_save_rename_and_burst() {
    let e = env(support::USER);
    let s = connect(&e).await;
    let sftp = sftp(&s).await;
    let dir = workdir(&sftp, support::USER).await;

    let path = format!("{dir}/exists.txt");
    user_sh(support::USER, &format!("printf 'server\\n' > '{path}'"));
    let err = save::save_text(&sftp, &save_req(&path, "mine\n", None), LIMIT)
        .await
        .unwrap_err();
    match code(&err) {
        ErrorCode::RemoteChanged {
            size,
            mtime,
            target,
            ..
        } => {
            assert_eq!(*size, 7);
            assert!(*mtime > 0);
            assert!(target.is_none());
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(user_sh(support::USER, &format!("cat '{path}'")), "server\n");
    let mut forced = save_req(&path, "mine\n", None);
    forced.force = true;
    save::save_text(&sftp, &forced, LIMIT).await.unwrap();
    assert_eq!(user_sh(support::USER, &format!("cat '{path}'")), "mine\n");

    let err = save::save_text(
        &sftp,
        &save_req(&format!("{dir}/big.txt"), &"x".repeat(2048), None),
        1024,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(
            code(&err),
            ErrorCode::FileTooLarge {
                size: 2048,
                limit: 1024,
                ..
            }
        ),
        "{err:?}"
    );
    // UTF-16 doubles the size: the encoded size is checked too.
    let mut wide = save_req(&format!("{dir}/wide.txt"), &"x".repeat(600), None);
    wide.encoding = "utf-16le".into();
    let err = save::save_text(&sftp, &wide, 1024).await.unwrap_err();
    assert!(
        matches!(code(&err), ErrorCode::FileTooLarge { size: 1200, .. }),
        "{err:?}"
    );

    let err = sftp
        .rename(&path, &format!("{dir}/no-such-dir/exists.txt"))
        .await
        .unwrap_err();
    match code(&err) {
        ErrorCode::RemoteNotFound { path } => assert_eq!(path, &format!("{dir}/no-such-dir")),
        other => panic!("{other:?}"),
    }

    // Burst: 12 uploads of 4 MiB each.
    let local = tempfile::tempdir().unwrap();
    let transfers = e.manager.transfers().clone();
    let rec = Arc::new(Recorder::default());
    let mut ids = Vec::new();
    for i in 0..12 {
        let f = local.path().join(format!("burst-{i:02}.bin"));
        std::fs::write(&f, vec![i as u8; 4 * 1024 * 1024]).unwrap();
        ids.push(transfers.start(
            s.handle.clone(),
            TransferSpec::Upload {
                local: f,
                remote_dir: dir.clone(),
            },
            false,
            rec.clone(),
        ));
    }
    // Every transfer reports `started` at once, also the queued ones.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let started = ids
        .iter()
        .filter(|id| matches!(rec.of(id).first(), Some(TransferEvent::Started { .. })))
        .count();
    assert_eq!(started, 12);
    // The tree keeps working during the burst; at most 3 temp files exist at any time.
    let mut max_tmp = 0;
    for _ in 0..10 {
        let listing = sftp.list(&dir).await.unwrap();
        let tmp = listing
            .entries
            .iter()
            .filter(|e| e.name.contains(".sshb-"))
            .count();
        max_tmp = max_tmp.max(tmp);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(max_tmp <= 3, "{max_tmp} temp files at once");
    for id in &ids {
        assert!(matches!(rec.finished(id).await, TransferEvent::Done { .. }));
    }
    let listing = sftp.list(&dir).await.unwrap();
    assert_eq!(
        listing
            .entries
            .iter()
            .filter(|e| e.name.starts_with("burst-"))
            .count(),
        12
    );

    root_sh(&format!("rm -rf '{dir}'"));
    e.manager.close(&s.id, None).await;
}

/// Per-direction bandwidth limits shared by a session's transfers, applied live.
#[tokio::test(flavor = "multi_thread")]
async fn bandwidth_limits_apply_and_update_live() {
    let e = env(support::USER);
    let s = connect(&e).await;
    let sftp = sftp(&s).await;
    let dir = workdir(&sftp, support::USER).await;
    let local = tempfile::tempdir().unwrap();
    let transfers = e.manager.transfers().clone();
    let rec = Arc::new(Recorder::default());
    let set_limits = |up: u32, down: u32| {
        let mut st = e.manager.settings();
        st.transfers.upload_limit_kibps = up;
        st.transfers.download_limit_kibps = down;
        e.manager.apply_settings(&st);
    };
    let src = local.path().join("limited.bin");
    std::fs::write(&src, vec![7u8; 512 * 1024]).unwrap();
    let upload = |name: &str| {
        let f = local.path().join(name);
        std::fs::copy(&src, &f).unwrap();
        transfers.start(
            s.handle.clone(),
            TransferSpec::Upload {
                local: f,
                remote_dir: dir.clone(),
            },
            true,
            rec.clone(),
        )
    };

    // Unlimited baseline is fast.
    let t = std::time::Instant::now();
    let id = upload("fast.bin");
    assert!(matches!(
        rec.finished(&id).await,
        TransferEvent::Done { bytes: 524_288, .. }
    ));
    let fast = t.elapsed();

    // 128 KiB/s: 512 KiB takes about 3 s (the first chunk is free).
    set_limits(128, 0);
    assert_eq!(s.handle.throttle.upload.kibps(), 128);
    let t = std::time::Instant::now();
    let id = upload("slow.bin");
    assert!(matches!(
        rec.finished(&id).await,
        TransferEvent::Done { bytes: 524_288, .. }
    ));
    let slow = t.elapsed();
    assert!(
        slow >= Duration::from_millis(2500) && slow > fast * 2,
        "slow {slow:?} vs fast {fast:?}"
    );

    // A running transfer follows a changed limit: 16 KiB/s would need 32 s; unlimiting after
    // one second lets it finish at once.
    set_limits(16, 0);
    let t = std::time::Instant::now();
    let id = upload("live.bin");
    tokio::time::sleep(Duration::from_secs(1)).await;
    set_limits(0, 0);
    assert!(matches!(
        rec.finished(&id).await,
        TransferEvent::Done { .. }
    ));
    assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());

    // Downloads have their own limiter and cancel interrupts the wait.
    set_limits(0, 32);
    let target = local.path().join("down.bin");
    let id = transfers.start(
        s.handle.clone(),
        TransferSpec::Download {
            remote: format!("{dir}/fast.bin"),
            local: target.clone(),
        },
        true,
        rec.clone(),
    );
    tokio::time::sleep(Duration::from_millis(800)).await;
    transfers.cancel(&id);
    assert!(matches!(
        rec.finished(&id).await,
        TransferEvent::Cancelled { .. }
    ));
    assert!(!target.exists());
    set_limits(0, 0);
    e.manager.close(&s.id, None).await;
}

/// Bookmarks round: saving a connected bookmark applies its bandwidth overrides to the running
/// session at once and emits `bookmarkUpdated`; connection fields only set `needsReconnect`.
#[tokio::test(flavor = "multi_thread")]
async fn bookmark_edit_applies_limits_live_and_flags_reconnect() {
    use shellfin_lib::ipc::dto::{AuthInput, BookmarkColor, BookmarkInput};
    let e = env(support::USER);
    let sshd = support::sshd();
    let input = BookmarkInput {
        id: None,
        name: "live".into(),
        host: sshd.host.clone(),
        port: Some(sshd.port),
        user: Some(support::USER.into()),
        auth: AuthInput::Key {
            identity_file: e.key.display().to_string(),
        },
        proxy_jump: None,
        group_id: None,
        tags: vec![],
        start_dir: None,
        use_ssh_config: false,
        color: None,
        upload_limit_kibps: None,
        download_limit_kibps: None,
        confirm_connect: None,
        startup_command: None,
        notes: None,
    };
    let saved = e
        .stores
        .lock_bookmarks()
        .save(input.clone(), 1)
        .unwrap()
        .bookmark;
    let (s, mut rx) = connect_target(
        &e,
        Target::Bookmark {
            id: saved.id.clone(),
        },
    )
    .await;
    assert_eq!(s.handle.bookmark_id.as_deref(), Some(saved.id.as_str()));
    assert_eq!(s.handle.throttle.upload.kibps(), 0);

    // Edit as `bookmark_save` does: store, then apply to the sessions.
    let apply = |edit: &dyn Fn(&mut BookmarkInput)| {
        let mut i = input.clone();
        i.id = Some(saved.id.clone());
        edit(&mut i);
        let b = e.stores.lock_bookmarks().save(i, 2).unwrap().bookmark;
        e.manager.apply_bookmark(&b);
    };
    let next_update = |rx: &mut mpsc::UnboundedReceiver<SessionEvent>| {
        let mut found = None;
        while let Ok(ev) = rx.try_recv() {
            if let SessionEvent::BookmarkUpdated {
                label,
                color,
                limit_overrides,
                needs_reconnect,
            } = ev
            {
                found = Some((label, color, limit_overrides, needs_reconnect));
            }
        }
        found.expect("bookmarkUpdated event")
    };

    apply(&|i| {
        i.name = "live renamed".into();
        i.color = Some(BookmarkColor::Teal);
        i.upload_limit_kibps = Some(64);
        i.download_limit_kibps = Some(0);
    });
    assert_eq!(s.handle.throttle.upload.kibps(), 64);
    assert_eq!(s.handle.throttle.download.kibps(), 0);
    let (label, color, lim, reconnect) = next_update(&mut rx);
    assert_eq!(label, "live renamed");
    assert_eq!(color, Some(BookmarkColor::Teal));
    assert_eq!((lim.upload_kibps, lim.download_kibps), (Some(64), Some(0)));
    assert!(!reconnect);

    // A connection field only raises the flag; the session stays connected and usable.
    apply(&|i| i.port = Some(2));
    assert!(next_update(&mut rx).3);
    let sftp = sftp(&s).await;
    sftp.stat("/").await.expect("session still works");
    // Undoing the edit and clearing the override follows the global setting again.
    apply(&|_| {});
    let (_, _, lim, reconnect) = next_update(&mut rx);
    assert!(!reconnect);
    assert_eq!((lim.upload_kibps, lim.download_kibps), (None, None));
    assert_eq!(s.handle.throttle.upload.kibps(), 0);
    e.manager.close(&s.id, None).await;
}

/// On an SFTP-only account (`ForceCommand internal-sftp`) the identity probe's `exec`
/// starts the SFTP server, which waits on stdin. The probe sends EOF, so connect is not delayed
/// by the probe budget and the identity is simply unknown; SFTP still works.
#[tokio::test(flavor = "multi_thread")]
async fn sftp_only_account_does_not_delay_connect_and_has_no_identity() {
    let e = env(support::USER_SFTP_ONLY);
    let started = std::time::Instant::now();
    let (tx, mut rx) = mpsc::unbounded_channel();
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
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    let info = loop {
        let ev = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("timed out waiting for connected")
            .expect("event channel closed");
        match ev {
            SessionEvent::Prompt {
                prompt: p @ Prompt::HostKeyUnknown { .. },
            } => e
                .manager
                .answer_prompt(
                    &id,
                    prompt_id(&p),
                    PromptAnswer::HostKey {
                        accept: true,
                        remember: false,
                    },
                )
                .unwrap(),
            SessionEvent::State {
                state: SessionState::Connected { info },
            } => break info,
            SessionEvent::State {
                state: SessionState::Disconnected { error, .. },
            } => panic!("disconnected: {error:?}"),
            _ => {}
        }
    };
    let elapsed = started.elapsed();
    assert!(info.remote_identity.is_none());
    assert!(info.sftp_available);
    assert!(
        elapsed < shellfin_lib::ssh::identity::PROBE_TIMEOUT,
        "connect took {elapsed:?}"
    );
    e.manager.close(&id, None).await;
}

/// A TCP proxy on `127.0.0.1` that delays every byte by `one_way` in each direction (a delay
/// line: latency without a bandwidth cap). Returns the local port.
async fn delay_proxy(upstream: (String, u16), one_way: Duration) -> u16 {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((client, _)) = listener.accept().await {
            let server = tokio::net::TcpStream::connect((upstream.0.as_str(), upstream.1))
                .await
                .unwrap();
            client.set_nodelay(true).unwrap();
            server.set_nodelay(true).unwrap();
            let (cr, cw) = client.into_split();
            let (sr, sw) = server.into_split();
            for (mut r, mut w) in [
                (
                    Box::new(cr) as Box<dyn tokio::io::AsyncRead + Unpin + Send>,
                    Box::new(sw) as Box<dyn tokio::io::AsyncWrite + Unpin + Send>,
                ),
                (Box::new(sr), Box::new(cw)),
            ] {
                let (tx, mut rx) = mpsc::unbounded_channel::<(tokio::time::Instant, Vec<u8>)>();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 64 * 1024];
                    while let Ok(n) = r.read(&mut buf).await {
                        if n == 0
                            || tx
                                .send((tokio::time::Instant::now(), buf[..n].to_vec()))
                                .is_err()
                        {
                            break;
                        }
                    }
                });
                tokio::spawn(async move {
                    while let Some((at, bytes)) = rx.recv().await {
                        tokio::time::sleep_until(at + one_way).await;
                        if w.write_all(&bytes).await.is_err() {
                            break;
                        }
                    }
                    let _ = w.shutdown().await;
                });
            }
        }
    });
    port
}

/// Benchmark (not part of the suite): upload and download a 32 MiB file through a proxy
/// that adds `SHELLFIN_BENCH_RTT_MS` (default 50) of round-trip latency, and print the
/// throughput. (The WSL2 Docker kernel has no `netem`, so the latency is added on the client.)
/// Run: `cargo test --locked --features it --test it_sftp transfer_throughput -- --ignored
/// --nocapture`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "benchmark; run by hand"]
async fn transfer_throughput() {
    const SIZE: usize = 32 * 1024 * 1024;
    let rtt_ms: u64 = std::env::var("SHELLFIN_BENCH_RTT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);
    let s = support::sshd();
    let port = delay_proxy((s.host.clone(), s.port), Duration::from_millis(rtt_ms / 2)).await;
    let e = env_at(support::USER, "127.0.0.1", port);
    let s = connect(&e).await;
    let sftp = sftp(&s).await;
    let dir = workdir(&sftp, support::USER).await;
    let local = tempfile::tempdir().unwrap();
    let transfers = e.manager.transfers().clone();
    let rec = Arc::new(Recorder::default());
    let sink: Arc<dyn TransferSink> = rec.clone();
    let src = local.path().join("bench.bin");
    let data: Vec<u8> = (0..SIZE).map(|i| (i % 251) as u8).collect();
    std::fs::write(&src, &data).unwrap();
    let mib = SIZE as f64 / (1024.0 * 1024.0);

    let t0 = std::time::Instant::now();
    let id = transfers.start(
        s.handle.clone(),
        TransferSpec::Upload {
            local: src.clone(),
            remote_dir: dir.clone(),
        },
        false,
        sink.clone(),
    );
    let ev = rec.finished(&id).await;
    let up = t0.elapsed();
    assert!(matches!(ev, TransferEvent::Done { .. }), "{ev:?}");

    let dst = local.path().join("back.bin");
    let t0 = std::time::Instant::now();
    let id = transfers.start(
        s.handle.clone(),
        TransferSpec::Download {
            remote: format!("{dir}/bench.bin"),
            local: dst.clone(),
        },
        true,
        sink.clone(),
    );
    let ev = rec.finished(&id).await;
    let down = t0.elapsed();
    assert!(matches!(ev, TransferEvent::Done { .. }), "{ev:?}");
    assert!(std::fs::read(&dst).unwrap() == data);
    println!(
        "throughput: upload {:.1} MiB/s ({up:?}), download {:.1} MiB/s ({down:?})",
        mib / up.as_secs_f64(),
        mib / down.as_secs_f64()
    );
    root_sh(&format!("rm -rf '{dir}'"));
    e.manager.close(&s.id, None).await;
}
