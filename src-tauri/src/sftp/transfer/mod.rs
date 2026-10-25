//! Single-file transfers (§6.6): download to a `.part` file + rename + Mark-of-the-Web, upload via
//! the §6.4 temp + rename procedure, progress (≤ 10 events/s), cancel, retry, cleanup.
//!
//! Local paths come only from the native dialogs or an OS drop (`OsChosen`, §9.4), never from the
//! WebView. Each transfer is a task with a cancel flag; failed retryable transfers are kept for
//! 10 minutes (or until their session closes) for `transfer_retry`.
//!
//! Event order per transfer: `started` → `progress`* → exactly one of `done`/`failed`/`cancelled`.
//!
//! This module holds the registry, the per-session gate and the task that reports a
//! transfer's end; `download` and `upload` hold the per-file jobs (each split into the
//! single-transfer wrapper with `started` and the slot, and a per-item function that only
//! copies), `local` the local-filesystem side (`.part`, provenance mark, upload source).

mod download;
mod job;
mod local;
mod upload;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, watch};

use crate::error::{AppError, DisconnectKind, EntityKind, ErrorCode};
use crate::ipc::dto::{ConflictPolicy, FileEntry, TransferDecision, TransferEvent};

use crate::session::session::SessionHandle;
pub use job::{TreeSpec, TreeState};

use super::client::{SESSION_EXTRA_REQUESTS, Sftp, Window};
use super::path;

use download::download;
pub use local::{
    create_part, local_times, mark_of_the_web, open_upload_source, sftp_times, upload_name,
};
use upload::upload;

/// Failed retryable transfers are kept this long (§6.6).
pub const RETRY_TTL: Duration = Duration::from_secs(10 * 60);
/// Progress events at most every 100 ms (10/s, §6.6).
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// How long `cancel_session` waits for transfers to clean up (temp/`.part` files).
pub const CANCEL_WAIT: Duration = Duration::from_secs(2);

/// Receives a transfer's events (`Channel<TransferEvent>` in the command layer).
pub trait TransferSink: Send + Sync + 'static {
    fn send(&self, event: TransferEvent);
}

/// What a transfer does; kept for `transfer_retry`.
#[derive(Debug, Clone)]
pub enum TransferSpec {
    Download {
        remote: String,
        local: PathBuf,
    },
    Upload {
        local: PathBuf,
        remote_dir: String,
    },
    /// A scanned folder download (its manifest and per-entry state, for retry).
    Tree(Arc<TreeState>),
    /// A folder download that failed before its scan finished (a retry starts over).
    TreeScan(TreeSpec),
}

struct Running {
    session_id: String,
    cancel: watch::Sender<bool>,
    task: Option<tokio::task::JoinHandle<()>>,
    /// Folder jobs: where `transfer_decide` answers go.
    decide: Option<mpsc::Sender<TransferDecision>>,
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
    /// Per-session folder job slots ([`job::MAX_JOBS_PER_SESSION`]).
    jobs: HashMap<String, Arc<Semaphore>>,
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

/// How one file (an item) ended successfully: bytes copied, or skipped by the conflict policy.
enum Item {
    Copied(u64),
    Skipped,
}

/// What a per-item copy runs with: the session (throttle), its SFTP client, the transfer's
/// gate (cancel flag, pipelining pool; the slot is already held) and the conflict policy.
#[derive(Clone, Copy)]
struct ItemCtx<'a> {
    session: &'a SessionHandle,
    sftp: &'a Sftp,
    gate: &'a Gate,
    conflict: Conflict,
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
            | ErrorCode::LocalDiskFull { .. }
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
                decide: None,
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
            inner.jobs.remove(session_id);
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

    /// Start a folder download job (§6.6 "Folders"); returns its id at once.
    pub fn start_tree(
        self: &Arc<Self>,
        session: Arc<SessionHandle>,
        spec: TreeSpec,
        policy: ConflictPolicy,
        sink: Arc<dyn TransferSink>,
    ) -> String {
        self.spawn_tree(session, policy, sink, TreeRun::New(spec))
    }

    /// `transfer_retry` of a kept folder job: only its failed and not-started files run;
    /// `overwrite` answers the set-aside conflicts with Replace.
    pub fn restart_tree(
        self: &Arc<Self>,
        session: Arc<SessionHandle>,
        state: Arc<TreeState>,
        overwrite: bool,
        policy: ConflictPolicy,
        sink: Arc<dyn TransferSink>,
    ) -> String {
        self.spawn_tree(session, policy, sink, TreeRun::Retry(state, overwrite))
    }

    fn spawn_tree(
        self: &Arc<Self>,
        session: Arc<SessionHandle>,
        policy: ConflictPolicy,
        sink: Arc<dyn TransferSink>,
        what: TreeRun,
    ) -> String {
        let max = self.max_active();
        let id = uuid::Uuid::new_v4().to_string();
        let (cancel, cancel_rx) = watch::channel(false);
        let (decide, decisions) = mpsc::channel(16);
        let mut inner = self.lock();
        Self::purge_expired(&mut inner);
        let session_id = session.id.clone();
        let slot = slot_for(&mut inner.slots, &session_id, max);
        let pool = inner
            .pools
            .entry(session_id.clone())
            .or_insert_with(|| Arc::new(Semaphore::new(SESSION_EXTRA_REQUESTS)))
            .clone();
        let job_slot = inner
            .jobs
            .entry(session_id.clone())
            .or_insert_with(|| Arc::new(Semaphore::new(job::MAX_JOBS_PER_SESSION)))
            .clone();
        let gate = Gate::new(slot, cancel_rx.clone()).with_pool(pool);
        let inputs = job::Inputs::new(decisions, cancel_rx);
        let env = job::JobEnv {
            id: id.clone(),
            session: session.clone(),
            sink: sink.clone(),
            policy,
            preserve_times: self.preserve_timestamps(),
            max_files: max,
        };
        let reg = self.clone();
        let task = tokio::spawn(async move {
            let name = match &what {
                TreeRun::New(spec) => spec.name().to_owned(),
                TreeRun::Retry(state, _) => state.name().to_owned(),
            };
            let end = match what {
                TreeRun::New(spec) => job::run_new(&env, spec, gate, inputs, job_slot).await,
                TreeRun::Retry(state, overwrite) => {
                    job::run_retry(&env, state, overwrite, gate, inputs, job_slot).await
                }
            };
            reg.finish_tree(&env, name, end);
        });
        inner.running.insert(
            id.clone(),
            Running {
                session_id,
                cancel,
                task: Some(task),
                decide: Some(decide),
            },
        );
        id
    }

    /// The bookkeeping and final event of a folder job.
    fn finish_tree(&self, env: &job::JobEnv, name: String, end: job::JobEnd) {
        let id = &env.id;
        let failed = |kept: Option<job::Kept>| {
            kept.map(|k| Failed {
                session_id: env.session.id.clone(),
                spec: match k {
                    job::Kept::State(s) => TransferSpec::Tree(s),
                    job::Kept::Spec(s) => TransferSpec::TreeScan(s),
                },
                at: Instant::now(),
            })
        };
        let (event, keep, done_name) = match end {
            job::JobEnd::Done {
                bytes,
                summary,
                keep,
            } => (
                TransferEvent::Done {
                    transfer_id: id.clone(),
                    bytes,
                    entry: None,
                    summary: Some(summary),
                },
                failed(keep.map(job::Kept::State)),
                Some(name),
            ),
            job::JobEnd::Skipped => (
                TransferEvent::Skipped {
                    transfer_id: id.clone(),
                },
                None,
                None,
            ),
            job::JobEnd::Cancelled => (
                TransferEvent::Cancelled {
                    transfer_id: id.clone(),
                },
                None,
                None,
            ),
            job::JobEnd::Failed { error, keep } => {
                let retryable = keep.is_some();
                tracing::info!(code = ?error.kind, retryable, "folder transfer failed");
                (
                    TransferEvent::Failed {
                        transfer_id: id.clone(),
                        error,
                        retryable,
                    },
                    failed(keep),
                    None,
                )
            }
        };
        // Bookkeeping first: a `failed` event must find its retry entry.
        self.finished_with(id, keep, done_name);
        env.sink.send(event);
    }

    /// `transfer_decide`: hand a decision to a running folder job (ignored when it does not
    /// wait for that kind of decision); `notFound{what:"transfer"}` when nothing runs under `id`.
    pub fn decide(&self, transfer_id: &str, decision: TransferDecision) -> Result<(), AppError> {
        let inner = self.lock();
        match inner.running.get(transfer_id) {
            Some(Running {
                decide: Some(tx), ..
            }) => match tx.try_send(decision) {
                Ok(()) => Ok(()),
                // The job is ending: as if it were gone.
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    Err(AppError::new(ErrorCode::NotFound {
                        what: EntityKind::Transfer,
                        id: transfer_id.to_owned(),
                    }))
                }
                Err(mpsc::error::TrySendError::Full(_)) => {
                    Err(AppError::invalid_argument("decision")
                        .with_detail("too many decisions queued for this transfer"))
                }
            },
            Some(_) => Ok(()),
            None => Err(AppError::new(ErrorCode::NotFound {
                what: EntityKind::Transfer,
                id: transfer_id.to_owned(),
            })),
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
                let tree_of = f.spec.is_tree().then(|| f.session_id.clone());
                inner.failed.insert(id.to_owned(), f);
                if let Some(session_id) = tree_of {
                    retain_trees(&mut inner.failed, &session_id);
                }
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

/// What a folder job task runs.
enum TreeRun {
    New(TreeSpec),
    Retry(Arc<TreeState>, bool),
}

/// At most [`job::MAX_RETAINED_JOBS`] kept folder jobs per session (their manifests can be
/// large): the oldest are forgotten (a retry then reports `notFound{what:"transfer"}`).
fn retain_trees(failed: &mut HashMap<String, Failed>, session_id: &str) {
    let mut trees: Vec<(Instant, String)> = failed
        .iter()
        .filter(|(_, f)| f.session_id == session_id && f.spec.is_tree())
        .map(|(id, f)| (f.at, id.clone()))
        .collect();
    if trees.len() <= job::MAX_RETAINED_JOBS {
        return;
    }
    trees.sort();
    let excess = trees.len() - job::MAX_RETAINED_JOBS;
    for (_, id) in trees.into_iter().take(excess) {
        failed.remove(&id);
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
        TransferSpec::Tree(state) => state.name().to_owned(),
        TransferSpec::TreeScan(spec) => spec.name().to_owned(),
    }
}

impl TransferSpec {
    /// A folder job (kept ones count against [`job::MAX_RETAINED_JOBS`]).
    fn is_tree(&self) -> bool {
        matches!(self, Self::Tree(_) | Self::TreeScan(_))
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
            files_done: None,
            files_total: None,
            current: None,
            failed: None,
            skipped: None,
        });
    }
}

/// Integration-test hooks (cargo feature `it`, never in the app): a simulated full disk for
/// downloads whose remote path contains a marker, and per-session counters of downloads that
/// hold a slot (to prove folder jobs and single transfers share `maxConcurrent`).
#[cfg(feature = "it")]
pub mod test_hooks {
    use std::collections::HashMap;
    use std::sync::{Mutex, PoisonError};

    static DISK_FULL: Mutex<Vec<String>> = Mutex::new(Vec::new());
    static ACTIVE: Mutex<Option<HashMap<String, (usize, usize)>>> = Mutex::new(None);

    /// Downloads whose remote path contains `marker` fail with a full disk after their first
    /// chunk (`clear_disk_full` ends it).
    pub fn fail_disk_for(marker: &str) {
        DISK_FULL
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(marker.to_owned());
    }

    pub fn clear_disk_full(marker: &str) {
        DISK_FULL
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|m| m != marker);
    }

    pub(super) fn disk_full(remote: &str) -> bool {
        DISK_FULL
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|m| remote.contains(m.as_str()))
    }

    /// Most downloads of `session_id` that ran at once.
    pub fn peak_downloads(session_id: &str) -> usize {
        ACTIVE
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(|m| m.get(session_id))
            .map_or(0, |(_, peak)| *peak)
    }

    /// Counts one running download of a session while alive.
    pub(super) struct Active(String);

    impl Active {
        pub(super) fn enter(session_id: &str) -> Self {
            let mut map = ACTIVE.lock().unwrap_or_else(PoisonError::into_inner);
            let e = map
                .get_or_insert_with(HashMap::new)
                .entry(session_id.to_owned())
                .or_insert((0, 0));
            e.0 += 1;
            e.1 = e.1.max(e.0);
            Self(session_id.to_owned())
        }
    }

    impl Drop for Active {
        fn drop(&mut self) {
            let mut map = ACTIVE.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(e) = map.as_mut().and_then(|m| m.get_mut(&self.0)) {
                e.0 = e.0.saturating_sub(1);
            }
        }
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

    /// Another gate on the same session slots, pool and cancel flag (one per file of a folder
    /// job, so its files wait for slots independently).
    pub fn fork(&self) -> Self {
        Self {
            slot: self.slot.clone(),
            cancel: self.cancel.clone(),
            pool: self.pool.clone(),
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
        // Folder jobs run through `start_tree` / `restart_tree`.
        TransferSpec::Tree(_) | TransferSpec::TreeScan(_) => {
            Err(AppError::internal("a folder job as a single transfer"))
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
                summary: None,
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

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, AppError> + Send + 'static,
) -> Result<T, AppError> {
    // A panic is caught on the blocking thread: the `JoinError` text would carry its payload.
    tokio::task::spawn_blocking(move || crate::error::catch_panic("transfer", f))
        .await
        .map_err(|e| crate::error::join_failed(e.is_panic(), "transfer"))??
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::super::throttle::Limiter;
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

    /// The files of a folder job (forked gates) and single transfers (their own gates on the
    /// same session semaphore) never run more than the session's slots together.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn folder_files_and_single_transfers_share_the_slots() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let slot = Arc::new(Semaphore::new(2));
        let (_tx, rx) = watch::channel(false);
        let job_gate = Gate::new(slot.clone(), rx.clone());
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for i in 0..12 {
            // 8 files of one job, 4 single transfers.
            let mut gate = if i < 8 {
                job_gate.fork()
            } else {
                Gate::new(slot.clone(), rx.clone())
            };
            let (active, peak) = (active.clone(), peak.clone());
            tasks.push(tokio::spawn(async move {
                let _permit = gate.acquire().await.unwrap();
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(20)).await;
                active.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), 2);
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
                    decide: None,
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

    /// Kept folder jobs (large manifests) are bounded per session: the oldest are forgotten;
    /// single-file failures and other sessions do not count.
    #[test]
    fn at_most_four_folder_jobs_are_kept_per_session() {
        let reg = TransferRegistry::default();
        let tree = |session: &str, age: u64| Failed {
            session_id: session.into(),
            spec: TransferSpec::TreeScan(TreeSpec::download(
                "/r".into(),
                PathBuf::from("p"),
                "r".into(),
            )),
            at: Instant::now() - Duration::from_secs(age),
        };
        reg.finished("file", Some(failed("s")));
        reg.finished("other", Some(tree("o", 100)));
        for i in 0..6u64 {
            reg.finished(&format!("j{i}"), Some(tree("s", 60 - i)));
        }
        assert!(reg.take_failed("j0").is_err());
        assert!(reg.take_failed("j1").is_err());
        for i in 2..6 {
            assert!(reg.take_failed(&format!("j{i}")).is_ok(), "j{i}");
        }
        assert!(reg.take_failed("file").is_ok());
        assert!(reg.take_failed("other").is_ok());
        assert_eq!(
            spec_name(&TransferSpec::TreeScan(TreeSpec::download(
                "/r".into(),
                PathBuf::from("p"),
                "proj".into()
            ))),
            "proj"
        );
    }

    /// `transfer_decide` reaches a running folder job; unknown ids are `notFound`; a single
    /// transfer ignores decisions.
    #[test]
    fn decisions_reach_running_jobs_only() {
        let reg = TransferRegistry::default();
        let (cancel, _) = watch::channel(false);
        let (tx, mut rx) = mpsc::channel(4);
        reg.lock().running.insert(
            "job".into(),
            Running {
                session_id: "s".into(),
                cancel,
                task: None,
                decide: Some(tx),
            },
        );
        reg.decide("job", TransferDecision::Merge).unwrap();
        assert_eq!(rx.try_recv().unwrap(), TransferDecision::Merge);
        // A full queue is an error, not a silent drop.
        for _ in 0..4 {
            reg.decide("job", TransferDecision::Proceed).unwrap();
        }
        let e = reg.decide("job", TransferDecision::Proceed).unwrap_err();
        assert!(matches!(e.kind, ErrorCode::InvalidArgument { ref field } if field == "decision"));
        // A job whose receiver is gone is not found.
        drop(rx);
        let e = reg.decide("job", TransferDecision::Proceed).unwrap_err();
        assert!(matches!(e.kind, ErrorCode::NotFound { .. }));
        let (cancel, _) = watch::channel(false);
        reg.lock().running.insert(
            "file".into(),
            Running {
                session_id: "s".into(),
                cancel,
                task: None,
                decide: None,
            },
        );
        assert!(reg.decide("file", TransferDecision::Proceed).is_ok());
        let e = reg.decide("gone", TransferDecision::Proceed).unwrap_err();
        assert!(matches!(
            e.kind,
            ErrorCode::NotFound {
                what: EntityKind::Transfer,
                ..
            }
        ));
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
                decide: None,
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
                decide: None,
            },
        );
        let r2 = running(&reg, "r2", "other");
        reg.signal_cancel_session("s");
        assert!(*rx.borrow());
        assert!(!*r2.borrow());
    }
}
