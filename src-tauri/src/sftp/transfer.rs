//! Single-file transfers (§6.6): download to a `.part` file + rename + Mark-of-the-Web, upload via
//! the §6.4 temp + rename procedure, progress (≤ 10 events/s), cancel, retry, cleanup.
//!
//! Local paths come only from the native dialogs or an OS drop (`OsChosen`, §9.4), never from the
//! WebView. Each transfer is a task with a cancel flag; failed retryable transfers are kept for
//! 10 minutes (or until their session closes) for `transfer_retry`.
//!
//! Event order per transfer: `started` → `progress`* → exactly one of `done`/`failed`/`cancelled`.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};

use crate::error::{AppError, DisconnectKind, EntityKind, ErrorCode, IoOp};
use crate::ipc::dto::{FileEntry, FileKind, TransferDirection, TransferEvent};
use crate::platform::file_trust::FileKind as LocalKind;
use crate::platform::local_path::{LocalPathError, open_local};
use crate::session::session::SessionHandle;

use super::client::{
    PipelinedReader, SESSION_EXTRA_REQUESTS, Sftp, Window, ensure_regular, kind_of,
};
use super::path;
use super::save::{self, DataSource, Hooks, NEW_FILE_MODE};
use super::throttle::Limiter;

/// Failed retryable transfers are kept this long (§6.6).
pub const RETRY_TTL: Duration = Duration::from_secs(10 * 60);
/// Progress events at most every 100 ms (10/s, §6.6).
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// How long `cancel_session` waits for transfers to clean up (temp/`.part` files).
pub const CANCEL_WAIT: Duration = Duration::from_secs(2);
/// How long a cancelled download waits for its remote CLOSE before removing the `.part`.
const CANCEL_CLOSE_WAIT: Duration = Duration::from_millis(500);
/// Local read size for uploads.
const LOCAL_READ: usize = 256 * 1024;
/// Local write buffer for downloads.
const LOCAL_WRITE: usize = 256 * 1024;
/// `.part` name attempts (`<chosen>.part`, `<chosen>.1.part`, …).
const PART_TRIES: u32 = 100;

/// Receives a transfer's events (`Channel<TransferEvent>` in the command layer).
pub trait TransferSink: Send + Sync + 'static {
    fn send(&self, event: TransferEvent);
}

/// What a transfer does; kept for `transfer_retry`.
#[derive(Debug, Clone)]
pub enum TransferSpec {
    Download { remote: String, local: PathBuf },
    Upload { local: PathBuf, remote_dir: String },
}

struct Running {
    session_id: String,
    cancel: watch::Sender<bool>,
    task: Option<tokio::task::JoinHandle<()>>,
}

struct Failed {
    session_id: String,
    spec: TransferSpec,
    at: Instant,
}

#[derive(Default)]
struct Inner {
    running: HashMap<String, Running>,
    failed: HashMap<String, Failed>,
    /// Per-session transfer slots with the size they were created for.
    slots: HashMap<String, (usize, Arc<Semaphore>)>,
    /// Per-session pool of pipelined requests above `PIPELINE_DEPTH`
    /// ([`SESSION_EXTRA_REQUESTS`]), shared by the session's transfers.
    pools: HashMap<String, Arc<Semaphore>>,
    /// Successful transfers since the last "transfers done" notification, and the
    /// name of the last one.
    batch_done: u32,
    batch_name: Option<String>,
}

/// Default for transfers that run at once per session (`transfers.maxConcurrent`
/// changes it, 1–64); the rest wait after `started`. Each holds ≤ 1 remote handle, a 256 KiB
/// local buffer and a pipelining window of 8 requests × 32 KiB plus up to 56 more from
/// the session pool, so a burst of many files never exhausts the server's `max-open-handles`
/// and leaves the channel to tree/editor requests. Per session at most
/// `active × (8 × 32 KiB + 256 KiB) + 168 × 32 KiB` is buffered: 6.75 MiB with the default 3,
/// 37.25 MiB with 64.
pub const MAX_ACTIVE_PER_SESSION: usize = 3;
/// Upper bound of `transfers.maxConcurrent`.
pub const MAX_ACTIVE_LIMIT: usize = 64;

/// What happens when the destination already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conflict {
    /// Upload: `remoteExists` (retryable). Download: the target was confirmed in the Save dialog.
    Fail,
    Overwrite,
    /// Finish with `skipped`, writing nothing.
    Skip,
}

impl Conflict {
    /// Resolve the explicit `overwrite` argument of a command against `transfers.onConflict`:
    /// an explicit `true` always overwrites; `ask` never overwrites by itself.
    pub fn resolve(explicit_overwrite: bool, policy: crate::ipc::dto::ConflictPolicy) -> Self {
        use crate::ipc::dto::ConflictPolicy;
        match (explicit_overwrite, policy) {
            (true, _) | (false, ConflictPolicy::Overwrite) => Self::Overwrite,
            (false, ConflictPolicy::Skip) => Self::Skip,
            (false, ConflictPolicy::Ask) => Self::Fail,
        }
    }
}

/// How a transfer ended successfully.
enum Finish {
    Done(u64, Option<FileEntry>),
    Skipped,
}

/// All transfers of the process (`AppState.transfers`, §3).
#[derive(Default)]
pub struct TransferRegistry {
    inner: Mutex<Inner>,
    /// `transfers.maxConcurrent`; 0 = not set yet (the default applies).
    max_active: std::sync::atomic::AtomicUsize,
    /// "transfers done" OS notification when the queue drains.
    notifier: std::sync::OnceLock<Arc<dyn crate::notify::Notifier>>,
    /// `!transfers.preserveTimestamps` (so the derived default preserves).
    skip_timestamps: std::sync::atomic::AtomicBool,
}

impl std::fmt::Debug for TransferRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.lock();
        f.debug_struct("TransferRegistry")
            .field("running", &inner.running.len())
            .field("failed", &inner.failed.len())
            .finish()
    }
}

/// `true` if the user may retry a transfer that failed with `e` (§6.6, §5.8).
pub fn retryable(e: &AppError) -> bool {
    matches!(
        e.kind,
        ErrorCode::RemoteExists { .. }
            | ErrorCode::Disconnected { .. }
            | ErrorCode::NotConnected { .. }
            | ErrorCode::SftpUnavailable
            | ErrorCode::Timeout { .. }
    )
}

impl TransferRegistry {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn purge_expired(inner: &mut Inner) {
        inner.failed.retain(|_, f| f.at.elapsed() < RETRY_TTL);
    }

    /// `transfers.maxConcurrent` (1–64): applies to transfers started afterwards; running and
    /// already queued ones keep the gate they were started with.
    pub fn set_max_active(&self, n: usize) {
        self.max_active.store(
            n.clamp(1, MAX_ACTIVE_LIMIT),
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    /// Where "transfers done" goes (set once by the session manager).
    pub fn set_notifier(&self, notifier: Arc<dyn crate::notify::Notifier>) {
        let _ = self.notifier.set(notifier);
    }

    pub fn max_active(&self) -> usize {
        match self.max_active.load(std::sync::atomic::Ordering::Relaxed) {
            0 => MAX_ACTIVE_PER_SESSION,
            n => n,
        }
    }

    /// Start a transfer for `session`; `overwrite` = `Overwrite`, else `Fail`.
    pub fn start(
        self: &Arc<Self>,
        session: Arc<SessionHandle>,
        spec: TransferSpec,
        overwrite: bool,
        sink: Arc<dyn TransferSink>,
    ) -> String {
        let conflict = if overwrite {
            Conflict::Overwrite
        } else {
            Conflict::Fail
        };
        self.start_with(session, spec, conflict, sink)
    }

    /// Start a transfer for `session`; returns its id at once (events go to `sink`).
    pub fn start_with(
        self: &Arc<Self>,
        session: Arc<SessionHandle>,
        spec: TransferSpec,
        conflict: Conflict,
        sink: Arc<dyn TransferSink>,
    ) -> String {
        let max = self.max_active();
        let id = uuid::Uuid::new_v4().to_string();
        let (cancel, cancel_rx) = watch::channel(false);
        let mut inner = self.lock();
        Self::purge_expired(&mut inner);
        let reg = self.clone();
        let task_id = id.clone();
        let session_id = session.id.clone();
        let slot = slot_for(&mut inner.slots, &session_id, max);
        let pool = inner
            .pools
            .entry(session_id.clone())
            .or_insert_with(|| Arc::new(Semaphore::new(SESSION_EXTRA_REQUESTS)))
            .clone();
        let gate = Gate::new(slot, cancel_rx).with_pool(pool);
        let task = tokio::spawn(async move {
            run(reg, task_id, session, spec, conflict, sink, gate).await;
        });
        inner.running.insert(
            id.clone(),
            Running {
                session_id,
                cancel,
                task: Some(task),
            },
        );
        id
    }

    /// `transfer_retry`: the failed transfer's spec and session (consumed), or
    /// `notFound{what:"transfer"}`.
    pub fn take_failed(&self, transfer_id: &str) -> Result<(String, TransferSpec), AppError> {
        let mut inner = self.lock();
        Self::purge_expired(&mut inner);
        inner
            .failed
            .remove(transfer_id)
            .map(|f| (f.session_id, f.spec))
            .ok_or_else(|| {
                AppError::new(ErrorCode::NotFound {
                    what: EntityKind::Transfer,
                    id: transfer_id.to_owned(),
                })
            })
    }

    /// `transfer_cancel` (idempotent): stop a running transfer (it reports `cancelled` and
    /// removes its temp files) or forget a failed one.
    pub fn cancel(&self, transfer_id: &str) {
        let mut inner = self.lock();
        if let Some(r) = inner.running.get(transfer_id) {
            let _ = r.cancel.send(true);
        }
        inner.failed.remove(transfer_id);
    }

    /// Session close (§5.8): cancel its running transfers and wait (bounded) until they cleaned
    /// up; forget its failed transfers.
    pub async fn cancel_session(&self, session_id: &str) {
        self.cancel_running(session_id, true).await;
    }

    /// `session_disconnect` (§6.6): cancel the session's active and queued transfers and
    /// wait (bounded) for their cleanup, but keep its failed retryable transfers: a disconnect
    /// is not a close, and they can be retried after reconnecting.
    pub async fn cancel_session_active(&self, session_id: &str) {
        self.cancel_running(session_id, false).await;
    }

    /// Quit: signal cancellation to the session's active and queued transfers and return
    /// at once (no waiting); the tasks notice it even inside a stalled request.
    pub fn signal_cancel_session(&self, session_id: &str) {
        let inner = self.lock();
        for r in inner
            .running
            .values()
            .filter(|r| r.session_id == session_id)
        {
            let _ = r.cancel.send(true);
        }
    }

    async fn cancel_running(&self, session_id: &str, forget_failed: bool) {
        let tasks: Vec<tokio::task::JoinHandle<()>> = {
            let mut inner = self.lock();
            if forget_failed {
                inner.failed.retain(|_, f| f.session_id != session_id);
            }
            inner.slots.remove(session_id);
            inner.pools.remove(session_id);
            inner
                .running
                .values_mut()
                .filter(|r| r.session_id == session_id)
                .filter_map(|r| {
                    let _ = r.cancel.send(true);
                    r.task.take()
                })
                .collect()
        };
        if tasks.is_empty() {
            return;
        }
        let all = futures_util::future::join_all(tasks);
        if tokio::time::timeout(CANCEL_WAIT, all).await.is_err() {
            tracing::warn!("transfers did not finish their cleanup in time");
        }
    }

    /// Number of running transfers (tests, diagnostics).
    pub fn running_count(&self) -> usize {
        self.lock().running.len()
    }

    #[cfg(test)]
    fn finished(&self, id: &str, failed: Option<Failed>) {
        self.finished_with(id, failed, None);
    }

    /// `transfers.preserveTimestamps` (read when a transfer starts its copy).
    pub fn set_preserve_timestamps(&self, preserve: bool) {
        self.skip_timestamps
            .store(!preserve, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn preserve_timestamps(&self) -> bool {
        !self
            .skip_timestamps
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// A transfer ended; `done_name` = it completed successfully (its file name). When the last
    /// running transfer of the app ends and something completed since the last time, the
    /// notifier hears about the batch (`transfers.notifyOnComplete`).
    fn finished_with(&self, id: &str, failed: Option<Failed>, done_name: Option<String>) {
        let batch = {
            let mut inner = self.lock();
            inner.running.remove(id);
            if let Some(f) = failed {
                inner.failed.insert(id.to_owned(), f);
            }
            if let Some(name) = done_name {
                inner.batch_done = inner.batch_done.saturating_add(1);
                inner.batch_name = Some(name);
            }
            take_batch(&mut inner)
        };
        if let (Some((count, name)), Some(n)) = (batch, self.notifier.get()) {
            n.transfers_done(count, name.as_deref());
        }
    }
}

/// The finished batch once nothing is running any more (and resets it).
fn take_batch(inner: &mut Inner) -> Option<(u32, Option<String>)> {
    if !inner.running.is_empty() || inner.batch_done == 0 {
        return None;
    }
    let count = std::mem::take(&mut inner.batch_done);
    let name = inner.batch_name.take();
    Some((count, name))
}

/// The file name a transfer is known by (for notifications).
fn spec_name(spec: &TransferSpec) -> String {
    match spec {
        TransferSpec::Download { remote, .. } => path::basename(remote).to_owned(),
        TransferSpec::Upload { local, .. } => local
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    }
}

/// The session's gate of size `max`. If the configured size changed, a new gate replaces the
/// old one for transfers started from now on (the old gate stays alive in its holders).
fn slot_for(
    slots: &mut HashMap<String, (usize, Arc<Semaphore>)>,
    session_id: &str,
    max: usize,
) -> Arc<Semaphore> {
    match slots.get(session_id) {
        Some((size, sem)) if *size == max => sem.clone(),
        _ => {
            let sem = Arc::new(Semaphore::new(max));
            slots.insert(session_id.to_owned(), (max, sem.clone()));
            sem
        }
    }
}

/// Throttled progress reporting.
struct Progress {
    id: String,
    total: Option<u64>,
    sink: Arc<dyn TransferSink>,
    last: Option<Instant>,
}

impl Progress {
    fn report(&mut self, bytes: u64) {
        if self.last.is_some_and(|t| t.elapsed() < PROGRESS_INTERVAL) {
            return;
        }
        self.last = Some(Instant::now());
        self.sink.send(TransferEvent::Progress {
            transfer_id: self.id.clone(),
            bytes,
            total: self.total,
        });
    }
}

/// A transfer's concurrency slot and cancel flag.
pub struct Gate {
    slot: Arc<Semaphore>,
    cancel: watch::Receiver<bool>,
    /// The session's pool of extra pipelined requests.
    pool: Arc<Semaphore>,
}

impl Gate {
    /// A gate with a private pool (tests; [`Gate::with_pool`] shares the session's).
    pub fn new(slot: Arc<Semaphore>, cancel: watch::Receiver<bool>) -> Self {
        Self {
            slot,
            cancel,
            pool: Arc::new(Semaphore::new(SESSION_EXTRA_REQUESTS)),
        }
    }

    pub fn with_pool(mut self, pool: Arc<Semaphore>) -> Self {
        self.pool = pool;
        self
    }

    /// A pipelining window drawing on the session's pool.
    pub fn window(&self) -> Window {
        Window::transfer(self.pool.clone())
    }

    /// The session's pool (for the writes of an upload).
    pub fn pool(&self) -> Arc<Semaphore> {
        self.pool.clone()
    }

    pub fn cancelled_fn(&self) -> impl Fn() -> bool + Send + Sync + use<> {
        let cancel = self.cancel.clone();
        move || *cancel.borrow()
    }

    /// The cancel flag as a signal ([`save::or_cancelled`]).
    pub fn cancel_signal(&self) -> watch::Receiver<bool> {
        self.cancel.clone()
    }

    /// Wait for a slot of the session; `cancelled` if the transfer is cancelled while it waits.
    pub async fn acquire(&mut self) -> Result<OwnedSemaphorePermit, AppError> {
        let cancelled = || AppError::new(ErrorCode::Cancelled);
        if *self.cancel.borrow() {
            return Err(cancelled());
        }
        let slot = self.slot.clone();
        tokio::select! {
            biased;
            _ = self.cancel.wait_for(|c| *c) => Err(cancelled()),
            permit = slot.acquire_owned() => permit.map_err(|_| cancelled()),
        }
    }
}

/// One transfer's fixed parameters.
struct Job<'a> {
    id: &'a str,
    session: &'a SessionHandle,
    conflict: Conflict,
    /// `transfers.preserveTimestamps`.
    preserve_times: bool,
    sink: &'a Arc<dyn TransferSink>,
}

/// SFTP v3 `(atime, mtime)` (u32 seconds) of local metadata; `None` when the mtime is outside
/// the representable range (before 1970, after 2106). A missing atime follows the mtime.
pub fn sftp_times(meta: &std::fs::Metadata) -> Option<(u32, u32)> {
    let secs = |t: std::time::SystemTime| {
        t.duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|d| u32::try_from(d.as_secs()).ok())
    };
    let mtime = secs(meta.modified().ok()?)?;
    let atime = meta.accessed().ok().and_then(secs).unwrap_or(mtime);
    Some((atime, mtime))
}

/// Local file times from remote SFTP attributes (`None` without an mtime).
pub fn local_times(atime: Option<u32>, mtime: Option<u32>) -> Option<std::fs::FileTimes> {
    let at = |s: u32| std::time::UNIX_EPOCH + Duration::from_secs(u64::from(s));
    let m = mtime?;
    Some(
        std::fs::FileTimes::new()
            .set_modified(at(m))
            .set_accessed(at(atime.unwrap_or(m))),
    )
}

async fn run(
    reg: Arc<TransferRegistry>,
    id: String,
    session: Arc<SessionHandle>,
    spec: TransferSpec,
    conflict: Conflict,
    sink: Arc<dyn TransferSink>,
    mut gate: Gate,
) {
    let cancelled = gate.cancelled_fn();
    let job = Job {
        id: &id,
        session: &session,
        conflict,
        preserve_times: reg.preserve_timestamps(),
        sink: &sink,
    };
    let result = match &spec {
        TransferSpec::Download { remote, local } => download(&job, remote, local, &mut gate).await,
        TransferSpec::Upload { local, remote_dir } => {
            upload(&job, local, remote_dir, &mut gate).await
        }
    };
    let mut keep = None;
    let mut done_name = None;
    let event = match result {
        Ok(Finish::Done(bytes, entry)) => {
            done_name = Some(spec_name(&spec));
            TransferEvent::Done {
                transfer_id: id.clone(),
                bytes,
                entry,
            }
        }
        Ok(Finish::Skipped) => TransferEvent::Skipped {
            transfer_id: id.clone(),
        },
        Err(_) if cancelled() => TransferEvent::Cancelled {
            transfer_id: id.clone(),
        },
        Err(error) => {
            let retryable = retryable(&error);
            tracing::info!(code = ?error.kind, retryable, "transfer failed");
            if retryable {
                keep = Some(Failed {
                    session_id: session.id.clone(),
                    spec: spec.clone(),
                    at: Instant::now(),
                });
            }
            TransferEvent::Failed {
                transfer_id: id.clone(),
                error,
                retryable,
            }
        }
    };
    // Bookkeeping first: a `failed` event must find its retry entry.
    reg.finished_with(&id, keep, done_name);
    sink.send(event);
}

/// A failure of a connection that is gone becomes `disconnected` (retryable, §5.8). That
/// includes a stop by the [`stop_signal`] when the connection went away (the I/O reports
/// `cancelled`; a user cancel is reported as `cancelled` by the caller anyway).
fn lost_as_disconnected(sftp: Option<&Sftp>, e: AppError) -> AppError {
    let lost = |s: &Sftp| {
        s.conn.loss().unwrap_or_else(|| {
            AppError::new(ErrorCode::Disconnected {
                reason: DisconnectKind::ConnectionReset,
                message: None,
            })
        })
    };
    match (&e.kind, sftp) {
        (ErrorCode::NotConnected { .. }, Some(s)) => lost(s),
        (ErrorCode::Cancelled, Some(s)) if s.conn.is_gone() => lost(s),
        _ => e,
    }
}

/// How often a running transfer checks whether its connection is gone.
const LOSS_POLL: Duration = Duration::from_millis(250);

/// Ends the [`stop_signal`] watcher with the transfer.
struct StopGuard(tokio::task::JoinHandle<()>);

impl Drop for StopGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The signal a transfer's I/O races (`save::or_cancelled`): fires on cancel **or** as
/// soon as the session gives the connection up. With a deep window the requests of a
/// dead link can queue behind a full SSH channel window, so waiting for the 30 s SFTP request
/// timeout (per request, plus close and remove) would report the loss a minute late, after the
/// automatic reconnect has already run out of attempts.
fn stop_signal(
    mut cancel: watch::Receiver<bool>,
    gone: impl Fn() -> bool + Send + 'static,
) -> (watch::Receiver<bool>, StopGuard) {
    let (tx, rx) = watch::channel(*cancel.borrow());
    let task = tokio::spawn(async move {
        let mut cancel_open = true;
        loop {
            if *cancel.borrow() || gone() {
                let _ = tx.send(true);
                return;
            }
            tokio::select! {
                changed = cancel.changed(), if cancel_open => cancel_open = changed.is_ok(),
                () = tokio::time::sleep(LOSS_POLL) => {}
            }
        }
    });
    (rx, StopGuard(task))
}

/// The abort check of a download's throttle wait: the user's cancel **or** the
/// [`stop_signal`] (fires on a lost connection too), so a long wait for bandwidth does not delay
/// reporting the loss. The resulting `cancelled` becomes `disconnected` when the connection is
/// gone ([`lost_as_disconnected`]).
fn cancel_or_stop<'a>(
    cancelled: &'a (dyn Fn() -> bool + Send + Sync),
    stop: watch::Receiver<bool>,
) -> impl Fn() -> bool + Send + Sync + 'a {
    move || cancelled() || *stop.borrow()
}

fn local_error(e: &io::Error, path: &Path, op: IoOp) -> AppError {
    let shown = path.display().to_string();
    let kind = match e.kind() {
        io::ErrorKind::NotFound => ErrorCode::LocalNotFound { path: shown },
        io::ErrorKind::PermissionDenied => ErrorCode::LocalPermissionDenied { path: shown },
        io::ErrorKind::IsADirectory => ErrorCode::LocalIsADirectory { path: shown },
        _ => ErrorCode::Io {
            op,
            path: Some(shown),
        },
    };
    AppError::new(kind).with_detail(e.to_string())
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, AppError> + Send + 'static,
) -> Result<T, AppError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| AppError::internal(format!("background task failed: {e}")))?
}

// ---------------------------------------------------------------------------------------------
// download
// ---------------------------------------------------------------------------------------------

/// `<chosen>.part`, `<chosen>.1.part`, … created with `create_new` (Unix: `O_NOFOLLOW`, default
/// mode `0o666 & !umask`).
pub fn create_part(chosen: &Path) -> Result<(PathBuf, std::fs::File), AppError> {
    let Some(name) = chosen.file_name() else {
        return Err(AppError::new(ErrorCode::LocalNotFound {
            path: chosen.display().to_string(),
        }));
    };
    if chosen.is_dir() {
        return Err(AppError::new(ErrorCode::LocalIsADirectory {
            path: chosen.display().to_string(),
        }));
    }
    for n in 0..PART_TRIES {
        let mut part_name: OsString = name.to_owned();
        if n > 0 {
            part_name.push(format!(".{n}"));
        }
        part_name.push(".part");
        let part = chosen.with_file_name(part_name);
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        match opts.open(&part) {
            Ok(f) => return Ok((part, f)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(local_error(&e, &part, IoOp::WriteLocalFile)),
        }
    }
    Err(AppError::new(ErrorCode::Io {
        op: IoOp::WriteLocalFile,
        path: Some(chosen.display().to_string()),
    })
    .with_detail("no free .part name"))
}

/// Provenance mark for a downloaded file (§6.6), so the OS warns before it is run or
/// opened: Windows Mark-of-the-Web (`Zone.Identifier` ADS, ZoneId 3 = Internet → SmartScreen,
/// Protected View); macOS `com.apple.quarantine` (Gatekeeper); nothing standard on Linux.
pub fn mark_of_the_web(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        let mut ads = path.as_os_str().to_owned();
        ads.push(":Zone.Identifier");
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(PathBuf::from(ads))?;
        std::io::Write::write_all(&mut f, b"[ZoneTransfer]\r\nZoneId=3\r\n")?;
        f.sync_all()
    }
    #[cfg(target_os = "macos")]
    {
        quarantine(path)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = path;
        Ok(())
    }
}

/// `com.apple.quarantine` = `0081;<hex unix time>;Shellfin;` (flag 0x0081: downloaded, not yet
/// approved), set without following a symlink.
#[cfg(target_os = "macos")]
fn quarantine(path: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let value = format!("0081;{now:x};Shellfin;");
    let p = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let name = c"com.apple.quarantine";
    // SAFETY: `p` and `name` are NUL-terminated; `value` is valid for `value.len()` bytes.
    let rc = unsafe {
        libc::setxattr(
            p.as_ptr(),
            name.as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            0,
            libc::XATTR_NOFOLLOW,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Mark the finished `.part` (the mark moves with the file on rename, so the final name never
/// exists unmarked), then move it over the chosen path (the user confirmed any
/// overwrite in the native dialog). If the `.part` could not be marked, the final file is tried.
#[cfg(test)]
fn finish_part(part: &Path, chosen: &Path) -> Result<(), AppError> {
    finish_part_with(part, chosen, None)
}

/// [`finish_part`], plus the remote times on the open `.part` handle after the
/// mark (writing the mark can touch the times) and before the rename. Failing to set the times
/// is logged, never an error.
fn finish_part_with(
    part: &Path,
    chosen: &Path,
    times: Option<(std::fs::File, std::fs::FileTimes)>,
) -> Result<(), AppError> {
    let marked = mark_of_the_web(part);
    if let Some((file, t)) = times {
        if let Err(e) = file.set_times(t) {
            tracing::info!(error = %e, "could not set the download's timestamps");
        }
        drop(file);
    }
    std::fs::rename(part, chosen).map_err(|e| local_error(&e, chosen, IoOp::WriteLocalFile))?;
    if let Err(first) = marked
        && let Err(e) = mark_of_the_web(chosen)
    {
        tracing::warn!(error = %e, part_error = %first, "could not mark the download");
    }
    Ok(())
}

async fn download(
    job: &Job<'_>,
    remote: &str,
    local: &Path,
    gate: &mut Gate,
) -> Result<Finish, AppError> {
    let Job {
        id,
        session,
        conflict,
        preserve_times,
        sink,
    } = *job;
    let cancelled = gate.cancelled_fn();
    let cancelled = &cancelled;
    let name = path::basename(remote).to_owned();
    let sftp = Sftp::for_session(session).await;
    let attrs = match &sftp {
        Ok(s) => s.stat(remote).await.map(Some),
        Err(e) => Err(e.clone()),
    };
    let total = attrs
        .as_ref()
        .ok()
        .and_then(|a| a.as_ref())
        .and_then(|a| a.size);
    sink.send(TransferEvent::Started {
        transfer_id: id.to_owned(),
        name,
        direction: TransferDirection::Download,
        total,
    });
    let sftp = sftp?;
    let attrs = attrs.map_err(|e| lost_as_disconnected(Some(&sftp), e))?;
    // Only a regular file (after links) is downloaded; a FIFO would block the server,
    // `/dev/zero` would fill the disk.
    if let Some(a) = &attrs {
        ensure_regular(a, remote)?;
    }
    let times = attrs
        .as_ref()
        .filter(|_| preserve_times)
        .and_then(|a| local_times(a.atime, a.mtime));
    // Wait for a slot after `started`, before any handle is opened.
    let _slot = gate.acquire().await?;
    if conflict == Conflict::Skip {
        let chosen = local.to_path_buf();
        if blocking(move || Ok(local_target_exists(&chosen))).await? {
            return Ok(Finish::Skipped);
        }
    }
    let chosen = local.to_path_buf();
    let (part, file) = blocking(move || create_part(&chosen)).await?;
    // One local write per 256 KiB instead of per 32 KiB chunk.
    let mut file =
        tokio::io::BufWriter::with_capacity(LOCAL_WRITE, tokio::fs::File::from_std(file));
    let mut progress = Progress {
        id: id.to_owned(),
        total,
        sink: sink.clone(),
        last: None,
    };

    let conn = sftp.conn.clone();
    let (stop, _stop_guard) = stop_signal(gate.cancel_signal(), move || conn.is_gone());
    // A throttle wait (up to a chunk's cost at the lowest limit, more behind the session's
    // other transfers) also ends when the connection is lost, not only on a user cancel.
    let throttle_stop = cancel_or_stop(cancelled, stop.clone());
    let mut cancel = Some(stop);
    let copy = async {
        let handle = save::or_cancelled(&mut cancel, sftp.open_read(remote)).await?;
        let mut reader = PipelinedReader::with_window(
            sftp.raw.clone(),
            handle.clone(),
            sftp.read_chunk(),
            None,
            gate.window(),
        );
        let result = async {
            loop {
                if cancelled() {
                    return Err(AppError::new(ErrorCode::Cancelled));
                }
                // A read stuck on a stalled server ends as soon as the transfer is
                // cancelled (not after the 30 s request timeout).
                let next = tokio::select! {
                    biased;
                    () = save::until_cancelled(cancel.as_mut()) => {
                        return Err(AppError::new(ErrorCode::Cancelled));
                    }
                    next = reader.next() => next,
                };
                match next {
                    Ok(Some(chunk)) => {
                        session
                            .throttle
                            .download
                            .acquire(chunk.len(), &throttle_stop)
                            .await?;
                        file.write_all(&chunk)
                            .await
                            .map_err(|e| local_error(&e, &part, IoOp::WriteLocalFile))?;
                        progress.report(reader.position());
                    }
                    Ok(None) => return Ok(reader.position()),
                    Err(e) => return Err(sftp.map_err(&e, Some(remote))),
                }
            }
        }
        .await;
        drop(reader);
        if cancelled() || sftp.conn.is_gone() {
            // Do not let a stalled server or a dead link keep the `.part` file around.
            let _ = tokio::time::timeout(CANCEL_CLOSE_WAIT, sftp.close_quietly(handle)).await;
        } else {
            sftp.close_quietly(handle).await;
        }
        let n = result?;
        // The buffered tail reaches the file before it is synced, marked and renamed.
        file.flush()
            .await
            .map_err(|e| local_error(&e, &part, IoOp::WriteLocalFile))?;
        file.get_mut()
            .sync_all()
            .await
            .map_err(|e| local_error(&e, &part, IoOp::WriteLocalFile))?;
        Ok::<u64, AppError>(n)
    }
    .await;
    // On failure or cancel the unflushed buffer is simply dropped with the `.part` file.
    let file = file.into_inner().into_std().await;
    let bytes = match copy {
        Ok(n) if !cancelled() => n,
        other => {
            drop(file);
            let part = part.clone();
            let _ = blocking(move || {
                let _ = std::fs::remove_file(&part);
                Ok(())
            })
            .await;
            let e = other
                .err()
                .unwrap_or_else(|| AppError::new(ErrorCode::Cancelled));
            return Err(lost_as_disconnected(Some(&sftp), e));
        }
    };
    let chosen = local.to_path_buf();
    let part_done = part.clone();
    let with_times = times.map(|t| (file, t));
    if let Err(e) = blocking(move || finish_part_with(&part_done, &chosen, with_times)).await {
        let _ = blocking(move || {
            let _ = std::fs::remove_file(&part);
            Ok(())
        })
        .await;
        return Err(e);
    }
    Ok(Finish::Done(bytes, None))
}

/// `true` if something (file, directory, link, even a dangling one) is at `path`.
fn local_target_exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

// ---------------------------------------------------------------------------------------------
// upload
// ---------------------------------------------------------------------------------------------

/// A throttle plus the transfer's cancel check (waits abort on cancel).
type Throttle<'a> = (&'a Limiter, &'a (dyn Fn() -> bool + Send + Sync));

/// A local file opened through `local_path` (source `OsChosen`: regular files only, §9.4).
struct LocalSource<'a> {
    file: tokio::fs::File,
    path: PathBuf,
    /// Up to [`LOCAL_READ`] bytes read ahead (reused) and the cursor into them (one
    /// copy per chunk; `split_off` copied the whole rest of the buffer for every chunk).
    buf: Vec<u8>,
    filled: usize,
    pos: usize,
    throttle: Option<Throttle<'a>>,
}

impl<'a> LocalSource<'a> {
    fn new(file: tokio::fs::File, path: PathBuf, throttle: Option<Throttle<'a>>) -> Self {
        Self {
            file,
            path,
            buf: Vec::new(),
            filled: 0,
            pos: 0,
            throttle,
        }
    }
}

impl DataSource for LocalSource<'_> {
    async fn next_chunk(&mut self, max: usize) -> Result<Option<Vec<u8>>, AppError> {
        // Read up to LOCAL_READ at once, hand out ≤ max per chunk.
        if self.pos >= self.filled {
            self.buf.resize(LOCAL_READ, 0);
            let n = self
                .file
                .read(&mut self.buf)
                .await
                .map_err(|e| local_error(&e, &self.path, IoOp::ReadLocalFile))?;
            (self.filled, self.pos) = (n, 0);
            if n == 0 {
                return Ok(None);
            }
        }
        let n = (self.filled - self.pos).min(max.max(1));
        if let Some((limiter, cancelled)) = self.throttle {
            limiter.acquire(n, cancelled).await?;
        }
        let chunk = self
            .buf
            .get(self.pos..self.pos + n)
            .ok_or_else(|| AppError::internal("upload buffer cursor out of range"))?
            .to_vec();
        self.pos += n;
        Ok(Some(chunk))
    }
}

/// Open an upload source: `localIsADirectory` for directories, `localNotFound`,
/// `localPermissionDenied`; returns the file, its name and size.
pub fn open_upload_source(local: &Path) -> Result<(std::fs::File, String, u64), AppError> {
    let shown = || local.display().to_string();
    let meta = std::fs::metadata(local).map_err(|e| local_error(&e, local, IoOp::ReadLocalFile))?;
    if meta.is_dir() {
        return Err(AppError::new(ErrorCode::LocalIsADirectory {
            path: shown(),
        }));
    }
    let file =
        open_local(local, LocalKind::UploadSource).map_err(|e| upload_open_error(e, local))?;
    let size = file.metadata().map(|m| m.len()).unwrap_or(meta.len());
    let name = upload_name(local)?;
    Ok((file, name, size))
}

/// The error for an upload source that `local_path` refused.
fn upload_open_error(e: LocalPathError, local: &Path) -> AppError {
    let shown = || local.display().to_string();
    match e {
        LocalPathError::NotFound => AppError::new(ErrorCode::LocalNotFound { path: shown() }),
        LocalPathError::PermissionDenied => {
            AppError::new(ErrorCode::LocalPermissionDenied { path: shown() })
        }
        // A FIFO, socket or device is not a directory.
        LocalPathError::NotAFile => AppError::new(ErrorCode::NotARegularFile { path: shown() }),
        other => AppError::new(ErrorCode::Io {
            op: IoOp::ReadLocalFile,
            path: Some(shown()),
        })
        .with_detail(other.to_string()),
    }
}

/// The remote name of an upload: the local file name, which must be valid Unicode (a lossy
/// U+FFFD name would create a different, unusable remote name) and a valid entry name.
pub fn upload_name(local: &Path) -> Result<String, AppError> {
    let name = local.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
        AppError::invalid_argument("name").with_detail("local file name is not valid Unicode")
    })?;
    if !path::valid_entry_name(name) {
        return Err(AppError::invalid_argument("name"));
    }
    Ok(name.to_owned())
}

/// Name and size for `started`, from metadata only (no handle is held while queued).
fn upload_info(local: &Path) -> (String, Option<u64>) {
    let name = local
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let size = std::fs::metadata(local)
        .ok()
        .filter(|m| m.is_file())
        .map(|m| m.len());
    (name, size)
}

async fn upload(
    job: &Job<'_>,
    local: &Path,
    remote_dir: &str,
    gate: &mut Gate,
) -> Result<Finish, AppError> {
    let Job {
        id,
        session,
        conflict,
        preserve_times,
        sink,
    } = *job;
    let overwrite = conflict == Conflict::Overwrite;
    let cancelled = gate.cancelled_fn();
    let cancelled = &cancelled;
    let cancel_signal = gate.cancel_signal();
    let window_pool = gate.pool();
    let local_owned = local.to_path_buf();
    let (shown_name, total) = blocking(move || Ok(upload_info(&local_owned))).await?;
    sink.send(TransferEvent::Started {
        transfer_id: id.to_owned(),
        name: shown_name,
        direction: TransferDirection::Upload,
        total,
    });
    // Wait for a slot after `started`, before the local file or a remote handle is
    // opened (a burst of 1 000 files holds no handles while queued).
    let _slot = gate.acquire().await?;
    let local_owned = local.to_path_buf();
    let (file, name, size, times) = blocking(move || {
        let (file, name, size) = open_upload_source(&local_owned)?;
        // Times of the opened file (`preserveTimestamps`).
        let times = file.metadata().ok().as_ref().and_then(sftp_times);
        Ok((file, name, size, times))
    })
    .await?;
    let times = times.filter(|_| preserve_times);
    let sftp = Sftp::for_session(session).await?;
    let conn = sftp.conn.clone();
    let (stop, _stop_guard) = stop_signal(cancel_signal, move || conn.is_gone());
    let target = path::join(remote_dir, &name);
    let result = async {
        // Uploads never write through a symlink. An existing link is replaced as an
        // entry (posix-rename over it); only a regular file is written into (keeping its mode,
        // owner and group).
        let existing = sftp.lstat_opt(&target).await?;
        let (cur, exists) = match existing.as_ref().map(kind_of) {
            None => (None, false),
            Some(FileKind::Dir) => {
                return Err(AppError::new(ErrorCode::IsADirectory {
                    path: target.clone(),
                }));
            }
            Some(FileKind::Symlink) => (None, true),
            Some(FileKind::File) => (existing.as_ref(), true),
            Some(FileKind::Other) => return Err(save::not_regular(&target)),
        };
        if exists && conflict == Conflict::Skip {
            return Ok(Finish::Skipped);
        }
        if exists && !overwrite {
            return Err(AppError::new(ErrorCode::RemoteExists {
                path: target.clone(),
            }));
        }
        let resolved = target.clone();
        let mut source = LocalSource::new(
            tokio::fs::File::from_std(file),
            local.to_path_buf(),
            Some((&session.throttle.upload, cancelled)),
        );
        let mut progress = Progress {
            id: id.to_owned(),
            total: Some(size),
            sink: sink.clone(),
            last: None,
        };
        let mut report = |n: u64| progress.report(n);
        let mut hooks = Hooks {
            progress: &mut report,
            cancelled,
            cancel_signal: Some(stop.clone()),
            window_pool: Some(window_pool.clone()),
        };
        let mut warnings = Vec::new();
        let written = save::write_file(
            &sftp,
            &save::WriteSpec {
                target: &resolved,
                cur,
                exists,
                new_mode: NEW_FILE_MODE,
                replace: overwrite,
                times,
                recheck: false,
            },
            &mut source,
            &mut hooks,
            &mut warnings,
        )
        .await?
        .bytes;
        for w in warnings {
            tracing::info!(code = ?w.kind, "upload warning");
        }
        let entry = sftp.entry(&target).await?;
        Ok(Finish::Done(written, Some(entry)))
    }
    .await;
    result.map_err(|e| lost_as_disconnected(Some(&sftp), e))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// The transfer's stop signal fires on cancel and on a lost connection (polled), and its
    /// watcher ends with the guard.
    #[tokio::test(start_paused = true)]
    async fn stop_signal_fires_on_cancel_or_loss() {
        use std::sync::atomic::{AtomicBool, Ordering};
        // Cancel: at once.
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let (mut stop, _guard) = stop_signal(cancel_rx, || false);
        assert!(!*stop.borrow());
        cancel_tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_millis(10), stop.wait_for(|s| *s))
            .await
            .unwrap()
            .unwrap();

        // Loss: within one poll interval, also when the cancel sender is gone.
        let gone = Arc::new(AtomicBool::new(false));
        let (cancel_tx, cancel_rx) = watch::channel(false);
        drop(cancel_tx);
        let g = gone.clone();
        let (mut stop, guard) = stop_signal(cancel_rx, move || g.load(Ordering::SeqCst));
        tokio::time::sleep(LOSS_POLL * 4).await;
        assert!(!*stop.borrow(), "nothing happened yet");
        gone.store(true, Ordering::SeqCst);
        tokio::time::timeout(LOSS_POLL * 2, stop.wait_for(|s| *s))
            .await
            .unwrap()
            .unwrap();
        drop(guard);

        // Dropping the guard ends the watcher: the signal never fires afterwards.
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let (mut stop, guard) = stop_signal(cancel_rx, || false);
        drop(guard);
        tokio::task::yield_now().await;
        assert!(
            stop.wait_for(|s| *s).await.is_err(),
            "sender gone, never fired"
        );
    }

    /// A download waiting for throttle budget stops when the connection is lost
    /// (not only on a user cancel), and its booking is given back.
    #[tokio::test(start_paused = true)]
    async fn throttle_wait_ends_on_connection_loss() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let limiter = Limiter::new(1); // 1 KiB/s
        // The session's other transfers booked an hour ahead.
        limiter.reserve(1024 * 3600, Instant::now());
        let gone = Arc::new(AtomicBool::new(false));
        let g = gone.clone();
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let (stop, _guard) = stop_signal(cancel_rx, move || g.load(Ordering::SeqCst));
        let user_cancel = || false;
        let check = cancel_or_stop(&user_cancel, stop);
        assert!(!check());
        // Without the stop this chunk would wait an hour.
        let wait = limiter.acquire(32 * 1024, &check);
        tokio::pin!(wait);
        tokio::select! {
            _ = &mut wait => panic!("should still be waiting"),
            () = tokio::time::sleep(Duration::from_secs(1)) => {}
        }
        gone.store(true, Ordering::SeqCst);
        let err = tokio::time::timeout(LOSS_POLL * 2 + Duration::from_millis(200), wait)
            .await
            .expect("the wait ends soon after the loss")
            .unwrap_err();
        assert_eq!(err.kind, ErrorCode::Cancelled);
        assert!(check());
        // The abandoned booking (32 s) was given back: only the hour is left.
        let left = limiter.reserve(1, Instant::now());
        assert!(left <= Duration::from_secs(3600), "{left:?}");
    }

    #[test]
    fn part_files_are_created_exclusively() {
        let dir = tempfile::tempdir().unwrap();
        let chosen = dir.path().join("report.pdf");
        let (p1, f1) = create_part(&chosen).unwrap();
        assert_eq!(p1, dir.path().join("report.pdf.part"));
        let (p2, _f2) = create_part(&chosen).unwrap();
        assert_eq!(p2, dir.path().join("report.pdf.1.part"));
        drop(f1);
        std::fs::write(&p1, b"data").unwrap();
        finish_part(&p1, &chosen).unwrap();
        assert_eq!(std::fs::read(&chosen).unwrap(), b"data");
        assert!(!p1.exists());
        // A directory as the chosen path.
        let d = dir.path().join("sub");
        std::fs::create_dir(&d).unwrap();
        let e = create_part(&d).unwrap_err();
        assert!(matches!(e.kind, ErrorCode::LocalIsADirectory { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn part_file_never_follows_a_planted_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim");
        std::fs::write(&victim, b"keep").unwrap();
        let chosen = dir.path().join("x.bin");
        std::os::unix::fs::symlink(&victim, dir.path().join("x.bin.part")).unwrap();
        let (p, _f) = create_part(&chosen).unwrap();
        assert_eq!(p, dir.path().join("x.bin.1.part"));
        assert_eq!(std::fs::read(&victim).unwrap(), b"keep");
    }

    #[cfg(windows)]
    #[test]
    fn mark_of_the_web_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("setup.exe");
        std::fs::write(&f, b"MZ").unwrap();
        mark_of_the_web(&f).unwrap();
        let mut ads = f.as_os_str().to_owned();
        ads.push(":Zone.Identifier");
        let zone = std::fs::read_to_string(PathBuf::from(ads)).unwrap();
        assert_eq!(zone, "[ZoneTransfer]\r\nZoneId=3\r\n");
        assert_eq!(std::fs::read(&f).unwrap(), b"MZ");
    }

    #[test]
    fn upload_sources() {
        let dir = tempfile::tempdir().unwrap();
        let e = open_upload_source(dir.path()).unwrap_err();
        assert!(matches!(e.kind, ErrorCode::LocalIsADirectory { .. }));
        let e = open_upload_source(&dir.path().join("missing")).unwrap_err();
        assert!(matches!(e.kind, ErrorCode::LocalNotFound { .. }));
        let f = dir.path().join("a.txt");
        std::fs::write(&f, b"hello").unwrap();
        let (_, name, size) = open_upload_source(&f).unwrap();
        assert_eq!((name.as_str(), size), ("a.txt", 5));
    }

    #[tokio::test]
    async fn local_source_hands_out_bounded_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("big");
        // Not a multiple of the chunk or read size; the content must come back in order.
        let data: Vec<u8> = (0..700_001u32).map(|i| (i % 253) as u8).collect();
        std::fs::write(&f, &data).unwrap();
        let mut s = LocalSource::new(tokio::fs::File::open(&f).await.unwrap(), f.clone(), None);
        let mut got = Vec::new();
        while let Some(c) = s.next_chunk(32_768).await.unwrap() {
            assert!(!c.is_empty() && c.len() <= 32_768);
            got.extend_from_slice(&c);
        }
        assert!(got == data, "content differs");
        // An empty file ends at once; a zero max still makes progress.
        let e = dir.path().join("empty");
        std::fs::write(&e, b"").unwrap();
        let mut s = LocalSource::new(tokio::fs::File::open(&e).await.unwrap(), e, None);
        assert!(s.next_chunk(32_768).await.unwrap().is_none());
        let mut s = LocalSource::new(tokio::fs::File::open(&f).await.unwrap(), f, None);
        assert_eq!(s.next_chunk(0).await.unwrap().unwrap().len(), 1);
    }

    /// A FIFO, socket or device as an upload source is `notARegularFile`, not
    /// `localIsADirectory`.
    #[test]
    fn non_regular_upload_sources_are_named_correctly() {
        let p = Path::new("dir/pipe");
        assert_eq!(
            upload_open_error(LocalPathError::NotAFile, p).kind,
            ErrorCode::NotARegularFile {
                path: p.display().to_string()
            }
        );
        assert!(matches!(
            upload_open_error(LocalPathError::NotFound, p).kind,
            ErrorCode::LocalNotFound { .. }
        ));
        assert!(matches!(
            upload_open_error(LocalPathError::PermissionDenied, p).kind,
            ErrorCode::LocalPermissionDenied { .. }
        ));
    }

    /// On Unix: a real FIFO is refused at open (and nothing blocks on it).
    #[cfg(unix)]
    #[test]
    fn fifo_upload_source_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("pipe");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: `c` is a valid NUL-terminated path (test only).
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let e = open_upload_source(&fifo).unwrap_err();
        assert!(matches!(e.kind, ErrorCode::NotARegularFile { .. }), "{e:?}");
    }

    /// Never more than the slot count run at once; a queued transfer can be cancelled
    /// without ever running.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn gate_limits_concurrency_and_cancels_queued() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let slot = Arc::new(Semaphore::new(MAX_ACTIVE_PER_SESSION));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        let mut cancels = Vec::new();
        for _ in 0..12 {
            let (tx, rx) = watch::channel(false);
            cancels.push(tx);
            let mut gate = Gate::new(slot.clone(), rx);
            let (active, peak) = (active.clone(), peak.clone());
            tasks.push(tokio::spawn(async move {
                let _permit = gate.acquire().await?;
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(30)).await;
                active.fetch_sub(1, Ordering::SeqCst);
                Ok::<(), AppError>(())
            }));
        }
        // Cancel the last one while it is (almost certainly) still queued.
        let _ = cancels.last().unwrap().send(true);
        let mut cancelled = 0;
        for t in tasks {
            if let Err(e) = t.await.unwrap() {
                assert!(matches!(e.kind, ErrorCode::Cancelled));
                cancelled += 1;
            }
        }
        assert_eq!(peak.load(Ordering::SeqCst), MAX_ACTIVE_PER_SESSION);
        assert_eq!(cancelled, 1);
        assert_eq!(slot.available_permits(), MAX_ACTIVE_PER_SESSION);
        // Cancelled before waiting at all.
        let (tx, rx) = watch::channel(true);
        let mut gate = Gate::new(Arc::new(Semaphore::new(0)), rx);
        assert!(gate.acquire().await.is_err());
        drop(tx);
    }

    #[test]
    fn conflict_resolution_never_overwrites_silently_when_asking() {
        use crate::ipc::dto::ConflictPolicy::{Ask, Overwrite, Skip};
        // An explicit overwrite always wins.
        for p in [Ask, Overwrite, Skip] {
            assert_eq!(Conflict::resolve(true, p), Conflict::Overwrite);
        }
        assert_eq!(Conflict::resolve(false, Ask), Conflict::Fail);
        assert_eq!(Conflict::resolve(false, Overwrite), Conflict::Overwrite);
        assert_eq!(Conflict::resolve(false, Skip), Conflict::Skip);
    }

    /// One "transfers done" notification when the queue drains, counting only
    /// successful transfers since the last one.
    #[test]
    fn transfers_done_is_notified_once_per_drained_batch() {
        #[derive(Default)]
        struct Rec(Mutex<Vec<(u32, Option<String>)>>);
        impl crate::notify::Notifier for Rec {
            fn connection_lost(&self, _: &str) {}
            fn transfers_done(&self, count: u32, name: Option<&str>) {
                self.0
                    .lock()
                    .unwrap()
                    .push((count, name.map(str::to_owned)));
            }
        }
        let rec = Arc::new(Rec::default());
        let reg = TransferRegistry::default();
        reg.set_notifier(rec.clone());
        let running = |reg: &TransferRegistry, id: &str| {
            let (cancel, _) = watch::channel(false);
            reg.lock().running.insert(
                id.to_owned(),
                Running {
                    session_id: "s".into(),
                    cancel,
                    task: None,
                },
            );
        };
        // Two at once: nothing until the last one ends.
        running(&reg, "a");
        running(&reg, "b");
        reg.finished_with("a", None, Some("a.txt".into()));
        assert!(rec.0.lock().unwrap().is_empty());
        reg.finished_with("b", None, Some("b.txt".into()));
        assert_eq!(*rec.0.lock().unwrap(), vec![(2, Some("b.txt".into()))]);
        // One alone: its name.
        running(&reg, "c");
        reg.finished_with("c", None, Some("c.txt".into()));
        // Failed / cancelled / skipped only: no notification.
        running(&reg, "d");
        reg.finished_with("d", None, None);
        assert_eq!(
            *rec.0.lock().unwrap(),
            vec![(2, Some("b.txt".into())), (1, Some("c.txt".into()))]
        );
        assert!(reg.preserve_timestamps(), "default on");
        reg.set_preserve_timestamps(false);
        assert!(!reg.preserve_timestamps());
        assert_eq!(
            spec_name(&TransferSpec::Download {
                remote: "/srv/a b.tar".into(),
                local: PathBuf::from("x")
            }),
            "a b.tar"
        );
    }

    /// Timestamps conversion and the download path (times set on the `.part`
    /// before the rename, after the mark).
    #[test]
    fn download_times_are_applied_before_the_rename() {
        assert!(local_times(None, None).is_none());
        assert!(local_times(Some(5), None).is_none());
        let dir = tempfile::tempdir().unwrap();
        let chosen = dir.path().join("data.bin");
        let (part, file) = create_part(&chosen).unwrap();
        std::fs::write(&part, b"payload").unwrap();
        let mtime = 1_600_000_000u32;
        let times = local_times(Some(1_500_000_000), Some(mtime)).unwrap();
        finish_part_with(&part, &chosen, Some((file, times))).unwrap();
        let meta = std::fs::metadata(&chosen).unwrap();
        let got = meta
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(got, u64::from(mtime));
        assert_eq!(sftp_times(&meta).map(|(_, m)| m), Some(mtime));
        assert_eq!(std::fs::read(&chosen).unwrap(), b"payload");
        assert!(!part.exists());
    }

    #[test]
    fn max_active_is_clamped_and_new_gates_follow_it() {
        let reg = TransferRegistry::default();
        assert_eq!(reg.max_active(), MAX_ACTIVE_PER_SESSION);
        reg.set_max_active(0);
        assert_eq!(reg.max_active(), 1);
        reg.set_max_active(999);
        assert_eq!(reg.max_active(), MAX_ACTIVE_LIMIT);
        reg.set_max_active(64);
        assert_eq!(reg.max_active(), 64);
        reg.set_max_active(2);
        assert_eq!(reg.max_active(), 2);

        let mut slots = HashMap::new();
        let a = slot_for(&mut slots, "s", 3);
        let a2 = slot_for(&mut slots, "s", 3);
        assert!(Arc::ptr_eq(&a, &a2), "same size reuses the gate");
        let _held = a.clone().try_acquire_owned().unwrap();
        let b = slot_for(&mut slots, "s", 1);
        assert!(!Arc::ptr_eq(&a, &b), "a new size makes a new gate");
        assert_eq!(b.available_permits(), 1);
        // The old gate keeps counting its holders.
        assert_eq!(a.available_permits(), 2);
    }

    #[test]
    fn existing_download_target_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x");
        assert!(!local_target_exists(&f));
        std::fs::write(&f, b"1").unwrap();
        assert!(local_target_exists(&f));
        assert!(local_target_exists(dir.path()));
    }

    #[test]
    fn upload_names_must_be_unicode() {
        assert_eq!(upload_name(Path::new("dir/a b.txt")).unwrap(), "a b.txt");
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let p = Path::new(std::ffi::OsStr::from_bytes(b"/tmp/bad-\xff"));
            let e = upload_name(p).unwrap_err();
            assert!(matches!(e.kind, ErrorCode::InvalidArgument { ref field } if field == "name"));
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            let p = PathBuf::from(std::ffi::OsString::from_wide(&[0x61, 0xD800, 0x62]));
            let e = upload_name(&p).unwrap_err();
            assert!(matches!(e.kind, ErrorCode::InvalidArgument { ref field } if field == "name"));
        }
    }

    #[test]
    fn retryable_errors() {
        assert!(retryable(&AppError::new(ErrorCode::RemoteExists {
            path: "/x".into()
        })));
        assert!(retryable(&AppError::new(ErrorCode::Disconnected {
            reason: DisconnectKind::KeepaliveTimeout,
            message: None
        })));
        assert!(!retryable(&AppError::new(ErrorCode::PermissionDenied {
            path: "/x".into()
        })));
        assert!(!retryable(&AppError::new(ErrorCode::LocalIsADirectory {
            path: "x".into()
        })));
    }

    #[test]
    fn registry_retry_and_cancel_bookkeeping() {
        let reg = TransferRegistry::default();
        reg.finished(
            "t1",
            Some(Failed {
                session_id: "s".into(),
                spec: TransferSpec::Upload {
                    local: PathBuf::from("a"),
                    remote_dir: "/d".into(),
                },
                at: Instant::now(),
            }),
        );
        let (sid, spec) = reg.take_failed("t1").unwrap();
        assert_eq!(sid, "s");
        assert!(matches!(spec, TransferSpec::Upload { .. }));
        // Consumed.
        let e = reg.take_failed("t1").unwrap_err();
        assert!(matches!(
            e.kind,
            ErrorCode::NotFound {
                what: EntityKind::Transfer,
                ..
            }
        ));
        // Expired entries are gone.
        reg.finished(
            "t2",
            Some(Failed {
                session_id: "s".into(),
                spec: TransferSpec::Download {
                    remote: "/r".into(),
                    local: PathBuf::from("l"),
                },
                at: Instant::now() - RETRY_TTL - Duration::from_secs(1),
            }),
        );
        assert!(reg.take_failed("t2").is_err());
        // Cancel forgets a failed transfer.
        reg.finished(
            "t3",
            Some(Failed {
                session_id: "s".into(),
                spec: TransferSpec::Download {
                    remote: "/r".into(),
                    local: PathBuf::from("l"),
                },
                at: Instant::now(),
            }),
        );
        reg.cancel("t3");
        assert!(reg.take_failed("t3").is_err());
    }

    fn failed(session_id: &str) -> Failed {
        Failed {
            session_id: session_id.into(),
            spec: TransferSpec::Download {
                remote: "/r".into(),
                local: PathBuf::from("l"),
            },
            at: Instant::now(),
        }
    }

    /// A running entry whose task waits for its cancel flag (like a transfer stuck in a request).
    fn running(reg: &TransferRegistry, id: &str, session_id: &str) -> watch::Receiver<bool> {
        let (cancel, rx) = watch::channel(false);
        let mut task_rx = rx.clone();
        let task = tokio::spawn(async move {
            let _ = task_rx.wait_for(|c| *c).await;
        });
        reg.lock().running.insert(
            id.into(),
            Running {
                session_id: session_id.into(),
                cancel,
                task: Some(task),
            },
        );
        rx
    }

    /// A disconnect cancels the session's running transfers but keeps its failed ones
    /// for `transfer_retry`; a close forgets them. Other sessions are untouched.
    #[tokio::test]
    async fn disconnect_keeps_failed_transfers_for_retry() {
        let reg = TransferRegistry::default();
        reg.finished("f1", Some(failed("s")));
        reg.finished("f2", Some(failed("other")));
        let r1 = running(&reg, "r1", "s");
        let r2 = running(&reg, "r2", "other");
        reg.cancel_session_active("s").await;
        assert!(*r1.borrow(), "running transfer cancelled");
        assert!(!*r2.borrow());
        assert!(reg.take_failed("f1").is_ok(), "failed transfer kept");
        reg.finished("f1", Some(failed("s")));
        reg.cancel_session("s").await;
        assert!(reg.take_failed("f1").is_err(), "close forgets it");
        assert!(reg.take_failed("f2").is_ok());
    }

    /// The quit path only signals (returns at once, even if the tasks never finish).
    #[tokio::test]
    async fn signal_cancel_session_does_not_wait() {
        let reg = TransferRegistry::default();
        let (cancel, rx) = watch::channel(false);
        reg.lock().running.insert(
            "stuck".into(),
            Running {
                session_id: "s".into(),
                cancel,
                task: Some(tokio::spawn(std::future::pending::<()>())),
            },
        );
        let r2 = running(&reg, "r2", "other");
        reg.signal_cancel_session("s");
        assert!(*rx.borrow());
        assert!(!*r2.borrow());
    }
}
