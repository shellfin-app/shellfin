//! Folder download integration cases (§6.6 "Folders", §10.2) against the Docker sshd: a nested
//! round trip with times, hostile server names, links and special files never followed or
//! opened, the depth and per-folder caps refusing before anything is written, an unreadable
//! subfolder as an item failure, the root-exists and per-file conflict decisions, a file
//! swapped for a link between scan and copy, and cancel cleanup.
//! Headless: `SessionManager` + in-memory sinks, private temp home (as `it_sftp.rs`).
#![cfg(feature = "it")]
#![allow(
    clippy::disallowed_methods,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::result_large_err
)]

mod support;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use shellfin_lib::error::{ErrorCode, TreeLimit};
use shellfin_lib::ipc::dto::{
    ConflictPolicy, DecisionRequest, FolderSummary, OpenSessionRequest, Prompt, PromptAnswer,
    SessionEvent, SessionState, Target, TransferDecision, TransferEvent, TransferKind,
};
use shellfin_lib::known_hosts::TrustOnceApprovals;
use shellfin_lib::session::manager::SessionManager;
use shellfin_lib::session::session::{ConnectCtxTemplate, SessionDeps, SessionHandle};
use shellfin_lib::sftp::transfer::{TransferRegistry, TransferSink, TransferSpec, TreeSpec};
use shellfin_lib::ssh::prompt::EventSink;
use shellfin_lib::sshconfig::ConfigEnv;
use shellfin_lib::sshconfig::service::SshConfigService;
use shellfin_lib::storage::Stores;
use tokio::sync::mpsc;

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
}

fn q(p: &Path) -> String {
    format!("\"{}\"", p.display().to_string().replace('\\', "/"))
}

fn env() -> Env {
    let s = support::sshd();
    let (key_dir, key) = support::private_key_copy(support::KEY_RSA_OPENSSH);
    let config = format!(
        "Host bastion\n  HostName {}\n  Port {}\n  User {}\n  IdentityFile {}\n  \
         IdentitiesOnly yes\nHost *\n  IdentityAgent none\n",
        s.host,
        s.port,
        support::USER,
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
    let deps = Arc::new(SessionDeps {
        stores: Arc::new(stores),
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
    }
}

struct Session {
    id: String,
    handle: Arc<SessionHandle>,
}

async fn connect(e: &Env) -> Session {
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
    loop {
        let ev = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("timed out waiting for connected")
            .expect("event channel closed");
        match ev {
            SessionEvent::Prompt {
                prompt: Prompt::HostKeyUnknown { id: pid, .. },
            } => e
                .manager
                .answer_prompt(
                    &id,
                    &pid,
                    PromptAnswer::HostKey {
                        accept: true,
                        remember: false,
                    },
                )
                .unwrap(),
            SessionEvent::State {
                state: SessionState::Connected { .. },
            } => break,
            SessionEvent::State {
                state: SessionState::Disconnected { error, .. },
            } => panic!("disconnected: {error:?}"),
            _ => {}
        }
    }
    let handle = e.manager.get(&id).unwrap();
    Session { id, handle }
}

fn root_sh(cmd: &str) -> String {
    support::docker_exec_ok(support::BASTION, &["sh", "-c", cmd])
}

fn user_sh(cmd: &str) -> String {
    support::docker_exec_ok(
        support::BASTION,
        &["runuser", "-u", support::USER, "--", "sh", "-c", cmd],
    )
}

/// A fresh remote folder in the user's home with `script` run inside it (as the user).
fn remote_tree(script: &str) -> String {
    let dir = format!(
        "/home/{}/it-folder-{}",
        support::USER,
        uuid::Uuid::new_v4().simple()
    );
    user_sh(&format!("mkdir '{dir}' && cd '{dir}' && {script}"));
    dir
}

#[derive(Default)]
struct Recorder(Mutex<Vec<TransferEvent>>);
impl TransferSink for Recorder {
    fn send(&self, event: TransferEvent) {
        self.0.lock().unwrap().push(event);
    }
}

impl Recorder {
    async fn wait_for<T>(&self, what: &str, f: impl Fn(&TransferEvent) -> Option<T>) -> T {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
        loop {
            if let Some(t) = self.0.lock().unwrap().iter().find_map(&f) {
                return t;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "no {what}: {:?}",
                self.0.lock().unwrap()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn finished(&self) -> TransferEvent {
        self.wait_for("final event", |e| {
            matches!(
                e,
                TransferEvent::Done { .. }
                    | TransferEvent::Failed { .. }
                    | TransferEvent::Cancelled { .. }
                    | TransferEvent::Skipped { .. }
            )
            .then(|| e.clone())
        })
        .await
    }

    async fn summary(&self) -> FolderSummary {
        match self.finished().await {
            TransferEvent::Done {
                summary: Some(s), ..
            } => s,
            other => panic!("{other:?}"),
        }
    }

    async fn decision(&self) -> DecisionRequest {
        self.wait_for("decision", |e| match e {
            TransferEvent::AwaitingDecision { request, .. } => Some(request.clone()),
            _ => None,
        })
        .await
    }
}

fn start(
    t: &Arc<TransferRegistry>,
    s: &Session,
    remote: &str,
    parent: &Path,
    name: &str,
    policy: ConflictPolicy,
) -> (String, Arc<Recorder>) {
    let rec = Arc::new(Recorder::default());
    let id = t.start_tree(
        s.handle.clone(),
        TreeSpec::download(remote.to_owned(), parent.to_path_buf(), name.to_owned()),
        policy,
        rec.clone(),
    );
    (id, rec)
}

/// Every file below `dir` (relative, `/`-joined), sorted.
fn local_files(dir: &Path) -> Vec<String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            let rel = p
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if e.file_type().unwrap().is_dir() {
                out.push(format!("{rel}/"));
                walk(base, &p, out);
            } else {
                out.push(rel);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

fn mtime(p: &Path) -> u64 {
    std::fs::metadata(p)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

// ---------------------------------------------------------------------------------------------
// cases
// ---------------------------------------------------------------------------------------------

/// Nested tree with empty folders, a 0-byte and a 1 MiB file: content and mtimes arrive,
/// no `.part` is left, the events follow the contract.
#[tokio::test(flavor = "multi_thread")]
async fn folder_download_round_trip() {
    let e = env();
    let s = connect(&e).await;
    let remote = remote_tree(
        "mkdir -p a/b/c empty/deeper && printf hello > a/hello.txt && : > a/b/zero \
         && head -c 1048576 /dev/urandom > a/b/c/big.bin && touch -d @1600000000 a/hello.txt",
    );
    let local = support::private_tempdir();
    let t = e.manager.transfers().clone();
    let (id, rec) = start(&t, &s, &remote, local.path(), "proj", ConflictPolicy::Ask);
    let summary = rec.summary().await;
    assert_eq!((summary.files_done, summary.failed), (3, 0));
    assert!(!summary.retryable);
    let root = local.path().join("proj");
    assert_eq!(
        local_files(&root),
        vec![
            "a/",
            "a/b/",
            "a/b/c/",
            "a/b/c/big.bin",
            "a/b/zero",
            "a/hello.txt",
            "empty/",
            "empty/deeper/"
        ]
    );
    assert_eq!(std::fs::read(root.join("a/hello.txt")).unwrap(), b"hello");
    assert_eq!(std::fs::metadata(root.join("a/b/zero")).unwrap().len(), 0);
    let want = root_sh(&format!(
        "sha256sum '{remote}/a/b/c/big.bin' | cut -d' ' -f1"
    ));
    let got = ring::digest::digest(
        &ring::digest::SHA256,
        &std::fs::read(root.join("a/b/c/big.bin")).unwrap(),
    );
    let hex: String = got.as_ref().iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(hex, want.trim());
    assert_eq!(mtime(&root.join("a/hello.txt")), 1_600_000_000);
    let events = rec.0.lock().unwrap().clone();
    assert!(matches!(
        events.first(),
        Some(TransferEvent::Started {
            kind: TransferKind::Folder,
            total: None,
            ..
        })
    ));
    assert!(events.iter().any(|e| matches!(
        e,
        TransferEvent::Scanned {
            files: 3,
            dirs: 5,
            ..
        }
    )));
    assert!(t.take_failed(&id).is_err(), "nothing to retry");
    root_sh(&format!("rm -rf '{remote}'"));
    e.manager.close(&s.id, None).await;
}

/// Hostile server names: one safe local name each, lossy (non-UTF-8) names skipped, case
/// collisions skipped where the local file system folds case.
#[tokio::test(flavor = "multi_thread")]
async fn folder_download_hostile_names() {
    let e = env();
    let s = connect(&e).await;
    let long = "n".repeat(255);
    let remote = remote_tree(&format!(
        "printf 1 > \"$(printf 'new\\nline')\" && printf 2 > 'back\\slash' && printf 3 > CON \
         && printf 4 > 'a:b' && printf 5 > 'trail.' && printf 6 > A && printf 7 > a \
         && printf 8 > \"$(printf 'bad\\377')\" && printf 9 > '{long}' && mkdir '..x' \
         && printf 0 > '..x/in'"
    ));
    let local = support::private_tempdir();
    let t = e.manager.transfers().clone();
    let (_id, rec) = start(&t, &s, &remote, local.path(), "h", ConflictPolicy::Ask);
    let summary = rec.summary().await;
    assert_eq!(summary.skipped.lossy_name, 1, "{summary:?}");
    let files = local_files(&local.path().join("h"));
    for want in ["new_line", "back_slash", "CON_", "a_b", "trail", "..x/in"] {
        assert!(files.iter().any(|f| f == want), "{want} in {files:?}");
    }
    // `A` and `a`: both where names are case-sensitive; where the file system folds case the
    // first in the server's READDIR order wins and the other is a collision.
    let case_folds = cfg!(any(windows, target_os = "macos"));
    let cases = files.iter().filter(|f| *f == "a" || *f == "A").count();
    assert_eq!(cases, if case_folds { 1 } else { 2 }, "{files:?}");
    assert_eq!(summary.skipped.name_collision, u32::from(case_folds));
    assert!(files.iter().any(|f| f.len() == 200 && f.starts_with("nnn")));
    assert!(!files.iter().any(|f| f.ends_with(".part")));
    root_sh(&format!("rm -rf '{remote}'"));
    e.manager.close(&s.id, None).await;
}

/// Links (to `/etc`, a loop) and special files (FIFO, device node) are skipped and never
/// opened: the job finishes quickly and nothing from `/etc` arrives.
#[tokio::test(flavor = "multi_thread")]
async fn folder_download_skips_links_and_specials() {
    let e = env();
    let s = connect(&e).await;
    let remote = remote_tree(
        "ln -s /etc etc-link && ln -s loop1 loop2 && ln -s loop2 loop1 && mkfifo pipe \
         && printf ok > plain.txt && mkdir sub && ln -s .. sub/up",
    );
    root_sh(&format!("mknod '{remote}/null' c 1 3"));
    let local = support::private_tempdir();
    let t = e.manager.transfers().clone();
    let (_id, rec) = start(&t, &s, &remote, local.path(), "l", ConflictPolicy::Ask);
    let summary = tokio::time::timeout(Duration::from_secs(30), rec.summary())
        .await
        .expect("links and FIFOs must not block the job");
    assert_eq!(summary.files_done, 1);
    assert_eq!(summary.skipped.symlink, 4, "{summary:?}");
    assert_eq!(summary.skipped.special, 2, "{summary:?}");
    assert_eq!(
        local_files(&local.path().join("l")),
        vec!["plain.txt", "sub/"]
    );
    root_sh(&format!("rm -rf '{remote}'"));
    e.manager.close(&s.id, None).await;
}

/// 70 levels and a folder of 100 001 entries are refused by the scan: nothing is written.
#[tokio::test(flavor = "multi_thread")]
async fn folder_download_depth_and_entry_caps() {
    let e = env();
    let s = connect(&e).await;
    let deep = remote_tree("p=.; for i in $(seq 1 70); do p=\"$p/d\"; done; mkdir -p \"$p\"");
    let wide = remote_tree("mkdir big && cd big && seq -w 1 100001 | xargs touch");
    let local = support::private_tempdir();
    let t = e.manager.transfers().clone();
    for (remote, limit) in [(&deep, TreeLimit::Depth), (&wide, TreeLimit::Directory)] {
        let (id, rec) = start(&t, &s, remote, local.path(), "x", ConflictPolicy::Ask);
        match rec.finished().await {
            TransferEvent::Failed {
                error, retryable, ..
            } => {
                assert!(
                    matches!(error.kind, ErrorCode::TreeTooLarge { limit: l, .. } if l == limit),
                    "{error:?}"
                );
                assert!(!retryable);
            }
            other => panic!("{other:?}"),
        }
        assert!(t.take_failed(&id).is_err());
        assert_eq!(std::fs::read_dir(local.path()).unwrap().count(), 0);
    }
    root_sh(&format!("rm -rf '{deep}' '{wide}'"));
    e.manager.close(&s.id, None).await;
}

/// An unreadable subfolder is an item failure; the rest is copied.
#[tokio::test(flavor = "multi_thread")]
async fn folder_permission_denied_subdir_continues() {
    let e = env();
    let s = connect(&e).await;
    let remote = remote_tree(
        "mkdir locked open && printf s > locked/secret && printf o > open/f && chmod 000 locked",
    );
    let local = support::private_tempdir();
    let t = e.manager.transfers().clone();
    let (id, rec) = start(&t, &s, &remote, local.path(), "p", ConflictPolicy::Ask);
    let summary = rec.summary().await;
    assert_eq!((summary.files_done, summary.failed), (1, 1));
    assert!(
        summary.retryable,
        "the unreadable folder is listed again on retry"
    );
    let issue = rec
        .wait_for("item issue", |e| match e {
            TransferEvent::ItemIssue { rel_path, .. } => Some(rel_path.clone()),
            _ => None,
        })
        .await;
    assert_eq!(issue, "locked");
    assert!(local.path().join("p/open/f").exists());

    // Fixed on the server, then retried: the folder is listed now and only it is copied.
    root_sh(&format!("chmod 755 '{remote}/locked'"));
    let rec = retry(&t, &s, &id, false).await;
    let summary = rec.summary().await;
    assert_eq!((summary.files_done, summary.failed), (2, 0), "{summary:?}");
    assert_eq!(
        std::fs::read(local.path().join("p/locked/secret")).unwrap(),
        b"s"
    );
    root_sh(&format!("rm -rf '{remote}'"));
    e.manager.close(&s.id, None).await;
}

/// `transfer_retry` of a kept folder job (as the command does it).
async fn retry(t: &Arc<TransferRegistry>, s: &Session, id: &str, overwrite: bool) -> Arc<Recorder> {
    let (_, spec) = t.take_failed(id).expect("the job was kept for retry");
    let rec = Arc::new(Recorder::default());
    match spec {
        TransferSpec::Tree(state) => {
            t.restart_tree(
                s.handle.clone(),
                state,
                overwrite,
                ConflictPolicy::Ask,
                rec.clone(),
            );
        }
        other => panic!("{other:?}"),
    }
    rec
}

/// A full local disk stops the job (`localDiskFull`, retryable) without leaving a `.part`
/// behind; the retry copies the rest.
#[tokio::test(flavor = "multi_thread")]
async fn folder_disk_full_then_retry() {
    let e = env();
    let s = connect(&e).await;
    let marker = format!("diskfull-{}", uuid::Uuid::new_v4().simple());
    let remote = remote_tree(&format!(
        "mkdir {marker} && head -c 1048576 /dev/urandom > {marker}/big && printf a > a.txt"
    ));
    let local = support::private_tempdir();
    let t = e.manager.transfers().clone();
    shellfin_lib::sftp::transfer::test_hooks::fail_disk_for(&marker);
    let (id, rec) = start(&t, &s, &remote, local.path(), "full", ConflictPolicy::Ask);
    match rec.finished().await {
        TransferEvent::Failed {
            error, retryable, ..
        } => {
            assert!(
                matches!(error.kind, ErrorCode::LocalDiskFull { .. }),
                "{error:?}"
            );
            assert!(retryable);
        }
        other => panic!("{other:?}"),
    }
    let files = local_files(&local.path().join("full"));
    assert!(
        !files.iter().any(|f| f.ends_with(".part")),
        "no .part left: {files:?}"
    );
    shellfin_lib::sftp::transfer::test_hooks::clear_disk_full(&marker);
    let rec = retry(&t, &s, &id, false).await;
    let summary = rec.summary().await;
    assert_eq!(summary.failed, 0, "{summary:?}");
    assert_eq!(
        std::fs::metadata(local.path().join("full").join(&marker).join("big"))
            .unwrap()
            .len(),
        1_048_576
    );
    assert!(local.path().join("full/a.txt").exists());
    root_sh(&format!("rm -rf '{remote}'"));
    e.manager.close(&s.id, None).await;
}

/// `transfers.maxConcurrent` bounds a folder job's files and single downloads together, and
/// the session's download limit applies across the job.
#[tokio::test(flavor = "multi_thread")]
async fn folder_jobs_share_slots_and_bandwidth() {
    let e = env();
    let s = connect(&e).await;
    let remote = remote_tree(
        "for i in 1 2 3 4; do head -c 262144 /dev/urandom > f$i; done \
         && head -c 262144 /dev/urandom > single",
    );
    let mut st = e.manager.settings();
    st.transfers.max_concurrent = 2;
    st.transfers.download_limit_kibps = 256;
    e.manager.apply_settings(&st);
    let local = support::private_tempdir();
    let t = e.manager.transfers().clone();
    let started = std::time::Instant::now();
    let (_id, rec) = start(&t, &s, &remote, local.path(), "bw", ConflictPolicy::Ask);
    let single = Arc::new(Recorder::default());
    let mut singles = Vec::new();
    for i in 0..2 {
        singles.push(t.start(
            s.handle.clone(),
            TransferSpec::Download {
                remote: format!("{remote}/single"),
                local: local.path().join(format!("single{i}")),
            },
            true,
            single.clone(),
        ));
    }
    let summary = rec.summary().await;
    // f1…f4 and `single` (it is in the folder too).
    assert_eq!(summary.files_done, 5);
    for id in &singles {
        single
            .wait_for("single done", |e| match e {
                TransferEvent::Done { transfer_id, .. } if transfer_id == id => Some(()),
                _ => None,
            })
            .await;
    }
    // 6 × 256 KiB at 256 KiB/s (the first chunks are free): well above 3 s.
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_secs(3), "{elapsed:?}");
    let peak = shellfin_lib::sftp::transfer::test_hooks::peak_downloads(&s.id);
    assert!((1..=2).contains(&peak), "peak {peak}");
    root_sh(&format!("rm -rf '{remote}'"));
    e.manager.close(&s.id, None).await;
}

/// The destination exists (`ask`): Merge, then the set-aside files are answered with
/// Replace all; Keep both writes `name (2)`; `skip` ends the job `skipped`.
#[tokio::test(flavor = "multi_thread")]
async fn folder_download_conflicts() {
    let e = env();
    let s = connect(&e).await;
    let remote = remote_tree("printf new > same.txt && printf fresh > other.txt");
    let local = support::private_tempdir();
    let dest = local.path().join("c");
    std::fs::create_dir(&dest).unwrap();
    std::fs::write(dest.join("same.txt"), b"old").unwrap();
    let t = e.manager.transfers().clone();

    let (id, rec) = start(&t, &s, &remote, local.path(), "c", ConflictPolicy::Ask);
    assert_eq!(
        rec.decision().await,
        DecisionRequest::RootExists {
            name: "c".into(),
            is_dir: true
        }
    );
    t.decide(&id, TransferDecision::Merge).unwrap();
    let count = rec
        .wait_for("conflicts", |e| match e {
            TransferEvent::Conflicts { count, sample, .. } => Some((*count, sample.len())),
            _ => None,
        })
        .await;
    assert_eq!(count, (1, 1));
    t.decide(&id, TransferDecision::ReplaceAll).unwrap();
    let summary = rec.summary().await;
    assert_eq!(summary.files_done, 2);
    assert_eq!(std::fs::read(dest.join("same.txt")).unwrap(), b"new");
    assert_eq!(std::fs::read(dest.join("other.txt")).unwrap(), b"fresh");

    // Keep both.
    let (id, rec) = start(&t, &s, &remote, local.path(), "c", ConflictPolicy::Ask);
    rec.decision().await;
    t.decide(&id, TransferDecision::KeepBoth).unwrap();
    rec.summary().await;
    assert!(local.path().join("c (2)/same.txt").exists());

    // Skip.
    let (_id, rec) = start(&t, &s, &remote, local.path(), "c", ConflictPolicy::Skip);
    assert!(matches!(
        rec.finished().await,
        TransferEvent::Skipped { .. }
    ));

    // Skip all for the files.
    std::fs::write(dest.join("same.txt"), b"mine").unwrap();
    let (id, rec) = start(&t, &s, &remote, local.path(), "c", ConflictPolicy::Ask);
    rec.decision().await;
    t.decide(&id, TransferDecision::Merge).unwrap();
    rec.wait_for("conflicts", |e| {
        matches!(e, TransferEvent::Conflicts { .. }).then_some(())
    })
    .await;
    t.decide(&id, TransferDecision::SkipAll).unwrap();
    let summary = rec.summary().await;
    assert_eq!(summary.skipped.exists, 2);
    assert_eq!(std::fs::read(dest.join("same.txt")).unwrap(), b"mine");
    root_sh(&format!("rm -rf '{remote}'"));
    e.manager.close(&s.id, None).await;
}

/// The server's tree changes between the scan and the copy (the job waits for the root
/// decision): a file swapped for a link to `/etc/passwd` and one swapped for a FIFO are skipped,
/// never followed or opened.
#[tokio::test(flavor = "multi_thread")]
async fn files_swapped_after_the_scan_are_not_followed() {
    let e = env();
    let s = connect(&e).await;
    let remote = remote_tree("printf a > link-later && printf b > fifo-later && printf c > keep");
    let local = support::private_tempdir();
    std::fs::create_dir(local.path().join("w")).unwrap();
    let t = e.manager.transfers().clone();
    let (id, rec) = start(&t, &s, &remote, local.path(), "w", ConflictPolicy::Ask);
    rec.decision().await;
    user_sh(&format!(
        "cd '{remote}' && rm link-later fifo-later && ln -s /etc/passwd link-later \
         && mkfifo fifo-later"
    ));
    t.decide(&id, TransferDecision::Merge).unwrap();
    let summary = tokio::time::timeout(Duration::from_secs(30), rec.summary())
        .await
        .expect("a FIFO must not block the job");
    assert_eq!(summary.files_done, 1);
    assert_eq!(
        (summary.skipped.symlink, summary.skipped.special),
        (1, 1),
        "{summary:?}"
    );
    assert_eq!(local_files(&local.path().join("w")), vec!["keep"]);
    root_sh(&format!("rm -rf '{remote}'"));
    e.manager.close(&s.id, None).await;
}

/// A folder with both `x` and `x.part` (and a name shaped like a temporary file): under
/// overwrite, and under ask with Replace all into an existing folder, every file ends with its
/// own content and no temporary file is left.
#[tokio::test(flavor = "multi_thread")]
async fn names_like_temporary_files_are_copied_correctly() {
    let e = env();
    let s = connect(&e).await;
    let big = 4 * 1024 * 1024;
    let remote = remote_tree(&format!(
        "head -c {big} /dev/urandom > x && printf small > x.part && printf dot > .x.abcdef12.part"
    ));
    let sum = |p: &str| {
        root_sh(&format!("sha256sum '{p}' | cut -d' ' -f1"))
            .trim()
            .to_owned()
    };
    let local_sum = |p: &Path| {
        let d = ring::digest::digest(&ring::digest::SHA256, &std::fs::read(p).unwrap());
        d.as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    let local = support::private_tempdir();
    let t = e.manager.transfers().clone();
    for (policy, prepare) in [
        (ConflictPolicy::Overwrite, false),
        (ConflictPolicy::Ask, true),
    ] {
        let dest = local.path().join("tmpnames");
        if prepare {
            for n in ["x", "x.part"] {
                std::fs::write(dest.join(n), b"old").unwrap();
            }
        }
        let (id, rec) = start(&t, &s, &remote, local.path(), "tmpnames", policy);
        if prepare {
            rec.decision().await;
            t.decide(&id, TransferDecision::Merge).unwrap();
            rec.wait_for("conflicts", |e| {
                matches!(e, TransferEvent::Conflicts { .. }).then_some(())
            })
            .await;
            t.decide(&id, TransferDecision::ReplaceAll).unwrap();
        }
        let summary = rec.summary().await;
        assert_eq!(summary.files_done, 3, "{policy:?}: {summary:?}");
        assert_eq!(local_sum(&dest.join("x")), sum(&format!("{remote}/x")));
        assert_eq!(std::fs::read(dest.join("x.part")).unwrap(), b"small");
        assert_eq!(
            std::fs::read(dest.join(".x.abcdef12.part")).unwrap(),
            b"dot"
        );
        assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 3, "no temp left");
    }
    root_sh(&format!("rm -rf '{remote}'"));
    e.manager.close(&s.id, None).await;
}

/// Cancel mid-copy: finished files stay, no `.part`, empty folders the job created are gone.
#[tokio::test(flavor = "multi_thread")]
async fn folder_cancel_cleans_up() {
    let e = env();
    let s = connect(&e).await;
    let remote = remote_tree(
        "mkdir -p big empty/inner && printf s > small.txt \
         && for i in 1 2 3 4; do head -c 67108864 /dev/urandom > big/f$i; done",
    );
    let local = support::private_tempdir();
    let t = e.manager.transfers().clone();
    let (id, rec) = start(&t, &s, &remote, local.path(), "k", ConflictPolicy::Ask);
    rec.wait_for("copy progress", |e| match e {
        TransferEvent::Progress { bytes, .. } if *bytes > 0 => Some(()),
        _ => None,
    })
    .await;
    t.cancel(&id);
    assert!(matches!(
        rec.finished().await,
        TransferEvent::Cancelled { .. }
    ));
    let root: PathBuf = local.path().join("k");
    let files = if root.exists() {
        local_files(&root)
    } else {
        Vec::new()
    };
    assert!(!files.iter().any(|f| f.ends_with(".part")), "{files:?}");
    assert!(!files.iter().any(|f| f.starts_with("empty")), "{files:?}");
    for f in &files {
        if f.starts_with("big/f") {
            assert_eq!(std::fs::metadata(root.join(f)).unwrap().len(), 67_108_864);
        }
    }
    root_sh(&format!("rm -rf '{remote}'"));
    e.manager.close(&s.id, None).await;
}
