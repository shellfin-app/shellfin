//! Folder downloads (§6.6 "Folders"): one transfer = one job. Phases: **scan** (the remote walk
//! into a bounded manifest, nothing is written), **prepare** (soft threshold, destination root
//! and its conflict, free space), **copy** (directories in walk order, files through the
//! session gate shared with single-file transfers), **finish** (final directory modes, the
//! final event).
//!
//! The job never holds a session slot itself: each file takes one from the session's gate, so
//! `transfers.maxConcurrent` bounds the files of all jobs and single transfers together. At
//! most [`MAX_JOBS_PER_SESSION`] jobs scan or copy at once per session.
//!
//! Events are bounded: `scanProgress` and `progress` at most every 100 ms, the first
//! [`MAX_ISSUES`] skipped or failed entries one by one (counters afterwards), at most
//! [`MAX_CONFLICT_SAMPLE`] conflict samples.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use russh_sftp::protocol::FileAttributes;
use tokio::sync::{mpsc, watch};

use crate::error::{AppError, ErrorCode, TreeLimit};
use crate::ipc::dto::{
    ConflictPolicy, ConflictSample, ConflictSide, DecisionRequest, FileKind, FolderSummary,
    ItemIssue, SkipCounts, SkipReason, TransferDecision, TransferDirection, TransferEvent,
    TransferKind,
};
use crate::platform::local_tree::{
    CreatedSet, LocalTree, Probe, RootState, TreeError, keep_both_names,
};
use crate::session::session::SessionHandle;

use super::super::client::{Sftp, ensure_regular, kind_of};
use super::super::path;
use super::super::tree::{
    MAX_DEPTH, MAX_ENTRIES, Manifest, Node, NodeKind, RemoteDirSource, SOFT_MAX_BYTES,
    SOFT_MAX_FILES, ScanEvents, ScanStats, scan, scan_below, too_large,
};
use super::download::download_item;
use super::local::{Dest, local_times, tree_error};
use super::{
    Conflict, Gate, Item, ItemCtx, PROGRESS_INTERVAL, TransferSink, blocking, lost_as_disconnected,
};

/// Folder jobs that scan or copy at once per session; more wait (cancellable) after `started`.
pub const MAX_JOBS_PER_SESSION: usize = 4;
/// Skipped or failed entries reported one by one per job.
pub const MAX_ISSUES: usize = 1_000;
/// Conflict samples per job.
pub const MAX_CONFLICT_SAMPLE: usize = 50;
/// Item failures in a row that stop the job (e.g. a remote quota SFTP v3 only reports as
/// `failure`).
pub const MAX_CONSECUTIVE_FAILURES: u32 = 50;
/// Failed folder jobs kept per session for `transfer_retry`.
pub const MAX_RETAINED_JOBS: usize = 4;

/// What the user asked for: the remote folder, the local parent folder from the dialog and the
/// local name of the destination folder below it.
#[derive(Debug, Clone)]
pub struct TreeSpec {
    remote: String,
    parent: PathBuf,
    name: String,
}

impl TreeSpec {
    pub fn download(remote: String, parent: PathBuf, name: String) -> Self {
        Self {
            remote,
            parent,
            name,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

/// One entry's state in a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum St {
    Pending,
    Done,
    Failed,
    Skipped,
    /// Set aside: the target exists and `onConflict = ask`.
    Conflict,
    /// Not tried: a directory above it failed.
    Blocked,
}

/// A scanned job, kept for `transfer_retry` (manifest ≤ 64 MiB, at most
/// [`MAX_RETAINED_JOBS`] per session).
pub struct TreeState {
    spec: TreeSpec,
    manifest: Manifest,
    status: Mutex<Vec<St>>,
    /// The local destination folder's name below the parent (after "keep both").
    root_name: Mutex<String>,
    /// The destination folder's identity: a retry refuses a folder that is no longer the same
    /// (an ancestor swapped for a link or junction meanwhile).
    root_id: Mutex<Option<(u64, u64)>>,
    /// The files this job wrote (all passes): a retry never replaces them.
    created: CreatedSet,
}

impl std::fmt::Debug for TreeState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TreeState")
            .field("nodes", &self.manifest.nodes.len())
            .finish_non_exhaustive()
    }
}

impl TreeState {
    pub fn name(&self) -> &str {
        self.spec.name()
    }

    fn statuses(&self) -> std::sync::MutexGuard<'_, Vec<St>> {
        self.status.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// How conflicting files are decided for the rest of the job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Set aside and ask once (`conflicts`).
    Ask,
    Replace,
    Skip,
    /// Replace when the source is newer (SFTP mtimes have 1 s resolution; equal is not older).
    ReplaceOlder,
}

impl Mode {
    fn of_policy(p: ConflictPolicy) -> Self {
        match p {
            ConflictPolicy::Ask => Self::Ask,
            ConflictPolicy::Overwrite => Self::Replace,
            ConflictPolicy::Skip => Self::Skip,
        }
    }

    fn of_decision(d: TransferDecision) -> Option<Self> {
        match d {
            TransferDecision::ReplaceAll => Some(Self::Replace),
            TransferDecision::SkipAll => Some(Self::Skip),
            TransferDecision::ReplaceOlder => Some(Self::ReplaceOlder),
            _ => None,
        }
    }
}

/// What a job waits on: user decisions (`transfer_decide`) and the cancel flag.
pub struct Inputs {
    decisions: mpsc::Receiver<TransferDecision>,
    cancel: watch::Receiver<bool>,
    /// A `cancel` decision seen while dropping stale decisions ([`Inputs::drain`]).
    pending_cancel: bool,
}

impl Inputs {
    pub fn new(decisions: mpsc::Receiver<TransferDecision>, cancel: watch::Receiver<bool>) -> Self {
        Self {
            decisions,
            cancel,
            pending_cancel: false,
        }
    }

    /// Drop decisions that arrived before the question about to be asked (a double click on
    /// an earlier prompt must not answer the next one). A queued `cancel` is kept as a cancel.
    fn drain(&mut self) {
        while let Ok(d) = self.decisions.try_recv() {
            self.pending_cancel |= d == TransferDecision::Cancel;
        }
    }

    /// The next decision `accept` takes; `cancelled` on cancel or a `cancel` decision.
    async fn wait(
        &mut self,
        accept: impl Fn(TransferDecision) -> bool,
    ) -> Result<TransferDecision, AppError> {
        let cancelled = || AppError::new(ErrorCode::Cancelled);
        loop {
            if *self.cancel.borrow() || self.pending_cancel {
                return Err(cancelled());
            }
            tokio::select! {
                biased;
                _ = self.cancel.wait_for(|c| *c) => return Err(cancelled()),
                d = self.decisions.recv() => match d {
                    None => return Err(cancelled()),
                    Some(TransferDecision::Cancel) => return Err(cancelled()),
                    Some(d) if accept(d) => return Ok(d),
                    Some(_) => {}
                },
            }
        }
    }
}

/// The bounded event reporting of one job (throttled progress, capped issues and samples).
struct Report {
    id: String,
    sink: Arc<dyn TransferSink>,
    issues: usize,
    last_progress: Option<Instant>,
    last_scan: Option<Instant>,
}

impl Report {
    fn new(id: &str, sink: Arc<dyn TransferSink>) -> Self {
        Self {
            id: id.to_owned(),
            sink,
            issues: 0,
            last_progress: None,
            last_scan: None,
        }
    }

    fn send(&self, event: TransferEvent) {
        self.sink.send(event);
    }

    fn issue(&mut self, item: Option<u32>, rel_path: &str, issue: ItemIssue) {
        if self.issues >= MAX_ISSUES {
            return;
        }
        self.issues += 1;
        self.send(TransferEvent::ItemIssue {
            transfer_id: self.id.clone(),
            item_id: item,
            rel_path: rel_path.to_owned(),
            issue,
        });
    }

    fn due(last: &mut Option<Instant>, force: bool) -> bool {
        if !force && last.is_some_and(|t| t.elapsed() < PROGRESS_INTERVAL) {
            return false;
        }
        *last = Some(Instant::now());
        true
    }
}

impl ScanEvents for Report {
    fn progress(&mut self, s: &ScanStats) {
        if Self::due(&mut self.last_scan, false) {
            self.send(TransferEvent::ScanProgress {
                transfer_id: self.id.clone(),
                files: s.files,
                dirs: s.dirs,
                bytes: s.bytes,
            });
        }
    }

    fn skipped(&mut self, rel_path: &str, reason: SkipReason) {
        self.issue(None, rel_path, ItemIssue::Skipped { reason });
    }

    fn failed(&mut self, item: u32, rel_path: &str, error: &AppError) {
        self.issue(
            Some(item),
            rel_path,
            ItemIssue::Failed {
                error: error.clone(),
                retryable: false,
            },
        );
    }
}

/// How a job ended.
pub enum JobEnd {
    /// Copied (possibly with item failures; `keep` = retryable).
    Done {
        bytes: u64,
        summary: FolderSummary,
        keep: Option<Arc<TreeState>>,
    },
    Skipped,
    Cancelled,
    Failed {
        error: AppError,
        keep: Option<Kept>,
    },
}

/// What a failed job keeps for `transfer_retry`.
pub enum Kept {
    /// Scanned: the retry copies the failed and not-started files.
    State(Arc<TreeState>),
    /// Failed before the scan finished: the retry starts over.
    Spec(TreeSpec),
}

/// The fixed parameters of a job run.
pub struct JobEnv {
    pub id: String,
    pub session: Arc<SessionHandle>,
    pub sink: Arc<dyn TransferSink>,
    pub policy: ConflictPolicy,
    pub preserve_times: bool,
    /// Files in flight at once (the session's `maxConcurrent`; each still needs a slot).
    pub max_files: usize,
}

/// A fresh job: scan, prepare, copy.
pub async fn run_new(
    env: &JobEnv,
    spec: TreeSpec,
    gate: Gate,
    mut inputs: Inputs,
    job_slot: Arc<tokio::sync::Semaphore>,
) -> JobEnd {
    let mut report = Report::new(&env.id, env.sink.clone());
    report.send(TransferEvent::Started {
        transfer_id: env.id.clone(),
        name: spec.name.clone(),
        direction: TransferDirection::Download,
        total: None,
        kind: TransferKind::Folder,
    });
    let rescan = || Some(Kept::Spec(spec.clone()));
    let _job = match acquire_job(&job_slot, &mut inputs).await {
        Ok(p) => p,
        Err(e) => return end_err(e, rescan()),
    };
    let sftp = match Sftp::for_session(&env.session).await {
        Ok(s) => s,
        Err(e) => return end_err(e, rescan()),
    };
    let manifest = match scan(
        &sftp,
        &spec.remote,
        &spec.name,
        inputs.cancel.clone(),
        &mut report,
    )
    .await
    {
        Ok(m) => m,
        Err(e) => return end_err(lost_as_disconnected(Some(&sftp), e), rescan()),
    };
    let stats = manifest.stats;
    report.send(TransferEvent::Scanned {
        transfer_id: env.id.clone(),
        files: stats.files,
        dirs: stats.dirs,
        bytes: stats.bytes,
        skipped: stats.skipped,
        renamed: stats.renamed,
    });
    let mut status = vec![St::Pending; manifest.nodes.len()];
    if let Some(root) = status.first_mut() {
        *root = St::Done;
    }
    for &i in &manifest.unlisted {
        if let Some(s) = usize::try_from(i).ok().and_then(|i| status.get_mut(i)) {
            *s = St::Failed;
        }
    }
    let state = Arc::new(TreeState {
        root_name: Mutex::new(spec.name.clone()),
        root_id: Mutex::new(None),
        created: CreatedSet::default(),
        spec,
        manifest,
        status: Mutex::new(status),
    });
    // Soft threshold (decision D2): ask before a very large copy.
    if stats.files > SOFT_MAX_FILES || stats.bytes > SOFT_MAX_BYTES {
        inputs.drain();
        report.send(TransferEvent::AwaitingDecision {
            transfer_id: env.id.clone(),
            request: DecisionRequest::LargeTree {
                files: stats.files,
                bytes: stats.bytes,
            },
        });
        if let Err(e) = inputs.wait(|d| d == TransferDecision::Proceed).await {
            return end_err(e, None);
        }
    }
    let tree = match prepare_root(env, &state, &mut inputs, &mut report).await {
        Ok(Some(t)) => t,
        Ok(None) => return JobEnd::Skipped,
        // Nothing was copied yet: a retry starts over.
        Err(e) => return end_err(e, Some(Kept::Spec(state.spec.clone()))),
    };
    copy(
        env,
        &sftp,
        &state,
        tree,
        gate,
        inputs,
        report,
        Mode::of_policy(env.policy),
    )
    .await
}

/// A retry of a kept job: only failed and not-started files (and the directories they need);
/// `overwrite` answers the conflicts with Replace.
pub async fn run_retry(
    env: &JobEnv,
    state: Arc<TreeState>,
    overwrite: bool,
    gate: Gate,
    mut inputs: Inputs,
    job_slot: Arc<tokio::sync::Semaphore>,
) -> JobEnd {
    let report = Report::new(&env.id, env.sink.clone());
    let remaining = {
        let mut status = state.statuses();
        let m = &state.manifest;
        let mut bytes = 0u64;
        for (i, s) in status.iter_mut().enumerate() {
            let is_listed_failure = u32::try_from(i).is_ok_and(|i| m.unlisted.contains(&i));
            if matches!(s, St::Failed | St::Blocked | St::Conflict) && !is_listed_failure {
                *s = St::Pending;
            }
            if *s == St::Pending
                && let Some(n) = m.nodes.get(i)
                && n.kind == NodeKind::File
            {
                bytes = bytes.saturating_add(n.size);
            }
        }
        bytes
    };
    report.send(TransferEvent::Started {
        transfer_id: env.id.clone(),
        name: state.name().to_owned(),
        direction: TransferDirection::Download,
        total: Some(remaining),
        kind: TransferKind::Folder,
    });
    let _job = match acquire_job(&job_slot, &mut inputs).await {
        Ok(p) => p,
        Err(e) => return end_err(e, Some(Kept::State(state))),
    };
    let sftp = match Sftp::for_session(&env.session).await {
        Ok(s) => s,
        Err(e) => return end_err(e, Some(Kept::State(state))),
    };
    let root_name = state
        .root_name
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let parent = state.spec.parent.clone();
    let created = state.created.clone();
    let opened = blocking(move || {
        LocalTree::open(&parent, &root_name, false, created)
            .map_err(|e| tree_error(e, &parent.join(&root_name)))
    })
    .await;
    let expected_id = *state.root_id.lock().unwrap_or_else(PoisonError::into_inner);
    let tree = match opened {
        Ok(t) if expected_id.is_some() && t.root_id() != expected_id => {
            let shown = state.spec.parent.join(
                &*state
                    .root_name
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner),
            );
            return end_err(
                AppError::new(ErrorCode::LocalUntrustedPath {
                    path: shown.display().to_string(),
                }),
                None,
            );
        }
        Ok(t) => Root {
            tree: Arc::new(t),
            created: false,
        },
        Err(e) => return end_err(e, Some(Kept::State(state))),
    };
    let mode = if overwrite {
        Mode::Replace
    } else {
        Mode::of_policy(env.policy)
    };
    let mut report = report;
    // Folders that could not be listed before are listed now (their subtrees join the job).
    let state = match rescan_unlisted(state, &sftp, inputs.cancel.clone(), &mut report).await {
        Ok(s) => s,
        Err((e, s)) => {
            return end_err(lost_as_disconnected(Some(&sftp), e), Some(Kept::State(s)));
        }
    };
    copy(env, &sftp, &state, tree, gate, inputs, report, mode).await
}

/// Scan events of a retry's listing of folder `base` (index in the job), reported relative to
/// the job's root.
struct Below<'a> {
    report: &'a mut Report,
    prefix: String,
    base: u32,
    offset: u32,
}

impl Below<'_> {
    fn rel(&self, rel: &str) -> String {
        if rel.is_empty() {
            self.prefix.clone()
        } else {
            format!("{}/{rel}", self.prefix)
        }
    }

    fn index(&self, sub: u32) -> u32 {
        if sub == 0 {
            self.base
        } else {
            self.offset.saturating_add(sub - 1)
        }
    }
}

impl ScanEvents for Below<'_> {
    fn progress(&mut self, _: &ScanStats) {}
    fn skipped(&mut self, rel_path: &str, reason: SkipReason) {
        let rel = self.rel(rel_path);
        self.report.skipped(&rel, reason);
    }
    fn failed(&mut self, item: u32, rel_path: &str, error: &AppError) {
        let (rel, item) = (self.rel(rel_path), self.index(item));
        self.report.failed(item, &rel, error);
    }
}

/// A retry lists the folders the first scan could not list (e.g. permissions fixed since) and
/// appends their subtrees to the manifest, under the same limits (depth counted from the job's
/// root, 200 000 entries in all). A folder that still cannot be listed stays a failure.
async fn rescan_unlisted<S: RemoteDirSource>(
    state: Arc<TreeState>,
    sftp: &S,
    cancel: watch::Receiver<bool>,
    report: &mut Report,
) -> Result<Arc<TreeState>, (AppError, Arc<TreeState>)> {
    if state.manifest.unlisted.is_empty() {
        return Ok(state);
    }
    // The retry owns the state (`take_failed` removed its record); if not, it copies only.
    let owned = match Arc::try_unwrap(state) {
        Ok(s) => s,
        Err(shared) => return Ok(shared),
    };
    let TreeState {
        spec,
        mut manifest,
        status,
        root_name,
        root_id,
        created,
    } = owned;
    let mut status = status.into_inner().unwrap_or_else(PoisonError::into_inner);
    let todo = std::mem::take(&mut manifest.unlisted);
    let mut result = Ok(());
    for (n, &base) in todo.iter().enumerate() {
        let depth = manifest.local_components(base).len();
        let path = manifest.remote_path(base);
        let name = manifest
            .node(base)
            .map(|n| n.name.to_string())
            .unwrap_or_default();
        let offset = manifest.len();
        let mut below = Below {
            prefix: manifest.rel_path(base),
            report: &mut *report,
            base,
            offset,
        };
        let max_depth = MAX_DEPTH.saturating_sub(depth);
        let sub = match scan_below(sftp, &path, &name, max_depth, cancel.clone(), &mut below).await
        {
            Ok(sub) => sub,
            Err(e) => {
                // This and the remaining folders stay unlisted.
                manifest.unlisted.extend(todo.iter().skip(n).copied());
                result = Err(e);
                break;
            }
        };
        if sub.unlisted.contains(&0) {
            manifest.unlisted.push(base);
            continue;
        }
        if manifest.nodes.len() + sub.nodes.len() > MAX_ENTRIES {
            manifest.unlisted.extend(todo.iter().skip(n).copied());
            result = Err(too_large(TreeLimit::Entries, MAX_ENTRIES));
            break;
        }
        for (j, node) in sub.nodes.into_iter().enumerate().skip(1) {
            let parent = below.index(node.parent);
            manifest.nodes.push(Node { parent, ..node });
            let j = u32::try_from(j).unwrap_or(u32::MAX);
            if sub.unlisted.contains(&j) {
                manifest.unlisted.push(below.index(j));
                status.push(St::Failed);
            } else {
                status.push(St::Pending);
            }
        }
        if let Some(s) = usize::try_from(base).ok().and_then(|b| status.get_mut(b)) {
            *s = St::Pending;
        }
        let st = &mut manifest.stats;
        st.files = st.files.saturating_add(sub.stats.files);
        st.dirs = st.dirs.saturating_add(sub.stats.dirs);
        st.bytes = st.bytes.saturating_add(sub.stats.bytes);
        st.renamed = st.renamed.saturating_add(sub.stats.renamed);
        st.skipped.merge(&sub.stats.skipped);
    }
    manifest.stats.failed = u64::try_from(manifest.unlisted.len()).unwrap_or(u64::MAX);
    let state = Arc::new(TreeState {
        spec,
        manifest,
        status: Mutex::new(status),
        root_name,
        root_id,
        created,
    });
    match result {
        Ok(()) => Ok(state),
        Err(e) => Err((e, state)),
    }
}

async fn acquire_job(
    slot: &Arc<tokio::sync::Semaphore>,
    inputs: &mut Inputs,
) -> Result<tokio::sync::OwnedSemaphorePermit, AppError> {
    let cancelled = || AppError::new(ErrorCode::Cancelled);
    if *inputs.cancel.borrow() {
        return Err(cancelled());
    }
    tokio::select! {
        biased;
        _ = inputs.cancel.wait_for(|c| *c) => Err(cancelled()),
        p = slot.clone().acquire_owned() => p.map_err(|_| cancelled()),
    }
}

fn end_err(e: AppError, keep: Option<Kept>) -> JobEnd {
    if e.kind == ErrorCode::Cancelled {
        return JobEnd::Cancelled;
    }
    let keep = keep.filter(|_| retryable_job_error(&e));
    JobEnd::Failed { error: e, keep }
}

/// Fatal job errors after which a retry makes sense.
fn retryable_job_error(e: &AppError) -> bool {
    super::retryable(e) || matches!(e.kind, ErrorCode::LocalDiskFull { .. })
}

/// The opened destination root.
struct Root {
    tree: Arc<LocalTree>,
    /// Created by this job (removed on cancel when it stayed empty).
    created: bool,
}

/// The destination root `<parent>/<name>`: created, merged, or "keep both" per
/// `onConflict` and the user's decision; then the free-space check. `None` = skipped.
async fn prepare_root(
    env: &JobEnv,
    state: &Arc<TreeState>,
    inputs: &mut Inputs,
    report: &mut Report,
) -> Result<Option<Root>, AppError> {
    let parent = state.spec.parent.clone();
    let name = state.spec.name.clone();
    let shown = parent.join(&name);
    let (p, n) = (parent.clone(), name.clone());
    let existing =
        blocking(move || LocalTree::inspect(&p, &n).map_err(|e| tree_error(e, &p.join(&n))))
            .await?;
    // `Some(true)` merge, `Some(false)` keep both, `None` create.
    let choice = match (existing, env.policy) {
        (RootState::Missing, _) => None,
        (_, ConflictPolicy::Skip) => return Ok(None),
        (RootState::Dir, ConflictPolicy::Overwrite) => Some(true),
        (RootState::Other, ConflictPolicy::Overwrite) => Some(false),
        (state_now, ConflictPolicy::Ask) => {
            let is_dir = state_now == RootState::Dir;
            inputs.drain();
            report.send(TransferEvent::AwaitingDecision {
                transfer_id: env.id.clone(),
                request: DecisionRequest::RootExists {
                    name: name.clone(),
                    is_dir,
                },
            });
            let d = inputs
                .wait(|d| {
                    d == TransferDecision::KeepBoth || (is_dir && d == TransferDecision::Merge)
                })
                .await?;
            Some(d == TransferDecision::Merge)
        }
    };
    let (p, n) = (parent.clone(), name.clone());
    let set = state.created.clone();
    let opened = blocking(move || {
        let open_new = |candidate: &str| LocalTree::open(&p, candidate, true, set.clone());
        let result = match choice {
            Some(true) => {
                LocalTree::open(&p, &n, false, set.clone()).map(|t| (t, n.clone(), false))
            }
            None => open_new(&n).map(|t| (t, n.clone(), true)),
            Some(false) => {
                let mut found = Err(TreeError::Exists);
                for candidate in keep_both_names(&n) {
                    match open_new(&candidate) {
                        Ok(t) => {
                            found = Ok((t, candidate, true));
                            break;
                        }
                        Err(TreeError::Exists) => {}
                        Err(e) => {
                            found = Err(e);
                            break;
                        }
                    }
                }
                found
            }
        };
        result.map_err(|e| tree_error(e, &shown))
    })
    .await?;
    let (tree, root_name, created) = opened;
    *state
        .root_name
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = root_name;
    *state.root_id.lock().unwrap_or_else(PoisonError::into_inner) = tree.root_id();
    let tree = Arc::new(tree);
    // Free space against the files still to copy (decision D2).
    let needed = state
        .manifest
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::File)
        .fold(0u64, |a, n| a.saturating_add(n.size));
    let t = tree.clone();
    if let Ok(Ok(available)) = blocking(move || Ok(t.free_space())).await
        && available < needed
    {
        inputs.drain();
        report.send(TransferEvent::AwaitingDecision {
            transfer_id: env.id.clone(),
            request: DecisionRequest::LowSpace { needed, available },
        });
        if let Err(e) = inputs.wait(|d| d == TransferDecision::Proceed).await {
            cleanup(Root { tree, created }, &state.manifest, &[], true).await;
            return Err(e);
        }
    }
    Ok(Some(Root { tree, created }))
}

/// One file's result.
enum Outcome {
    /// Bytes written.
    Copied(u64),
    Skip(SkipReason),
    Conflict(ConflictSample),
    Fail(AppError),
    /// Ends the job (cancel, connection loss, disk full).
    Fatal(AppError),
}

/// Whether an item error ends the whole job.
fn is_fatal(e: &AppError) -> bool {
    matches!(
        e.kind,
        ErrorCode::Cancelled
            | ErrorCode::Disconnected { .. }
            | ErrorCode::NotConnected { .. }
            | ErrorCode::SftpUnavailable
            | ErrorCode::LocalDiskFull { .. }
    )
}

/// What a file may be downloaded as, from fresh `lstat` attributes (the scan's attributes can be
/// stale or lies). `download_item` relies on this check: only a regular file is opened; a
/// link (never followed), a FIFO/socket/device (never opened), a directory or an entry
/// without a type is skipped or failed.
fn check_remote(attrs: &FileAttributes, remote: &str) -> Result<(), Outcome> {
    if attrs.permissions.is_none() {
        return Err(Outcome::Skip(SkipReason::UnknownType));
    }
    match kind_of(attrs) {
        FileKind::File => {}
        FileKind::Symlink => return Err(Outcome::Skip(SkipReason::Symlink)),
        FileKind::Other => return Err(Outcome::Skip(SkipReason::Special)),
        FileKind::Dir => {
            return Err(Outcome::Fail(AppError::new(ErrorCode::IsADirectory {
                path: remote.to_owned(),
            })));
        }
    }
    ensure_regular(attrs, remote).map_err(Outcome::Fail)
}

/// What to do with the local target: `Ok(replace)` = copy (replacing an existing entry or
/// not), `Err` = skip, set aside or fail.
fn target_action(
    probe: Probe,
    mode: Mode,
    source: ConflictSide,
    item: u32,
    rel_path: &dyn Fn() -> String,
    shown: &dyn Fn() -> String,
) -> Result<bool, Outcome> {
    let (target, newer_than_target) = match probe {
        Probe::Missing => return Ok(false),
        // A folder, or a Windows junction / directory symlink (a folder entry): in the way.
        Probe::Dir | Probe::Link { dir: true } => {
            return Err(Outcome::Fail(AppError::new(ErrorCode::LocalIsADirectory {
                path: shown(),
            })));
        }
        // Never replace a file this job wrote (a collision the name folding missed).
        Probe::File { ours: true, .. } => return Err(Outcome::Skip(SkipReason::NameCollision)),
        Probe::File { size, mtime, .. } => (
            ConflictSide {
                size: Some(size),
                mtime,
            },
            matches!((source.mtime, mtime), (Some(s), Some(t)) if s > t),
        ),
        // A link or special file is replaced as an entry, never written through.
        Probe::Link { dir: false } | Probe::Other => (
            ConflictSide {
                size: None,
                mtime: None,
            },
            false,
        ),
    };
    match mode {
        Mode::Replace => Ok(true),
        Mode::Skip => Err(Outcome::Skip(SkipReason::Exists)),
        Mode::ReplaceOlder if newer_than_target => Ok(true),
        Mode::ReplaceOlder => Err(Outcome::Skip(SkipReason::Exists)),
        Mode::Ask => Err(Outcome::Conflict(ConflictSample {
            item_id: item,
            rel_path: rel_path(),
            source,
            target,
        })),
    }
}

/// Shared live counters of a copy pass.
struct Live {
    /// Bytes written so far (finished files plus the running ones' partial bytes).
    moved: AtomicU64,
    /// Bytes of the files of this pass (adjusted when a size changed since the scan).
    total: AtomicU64,
    /// Stop dispatching (a fatal error was seen).
    stop: AtomicBool,
    current: Mutex<Option<String>>,
}

struct FileJob<'a> {
    env: &'a JobEnv,
    sftp: &'a Sftp,
    gate: Gate,
    tree: Arc<LocalTree>,
    live: &'a Live,
    idx: u32,
    remote: String,
    rel_path: String,
    dir: Vec<String>,
    name: String,
    display: String,
    mode: Mode,
}

async fn copy_file(mut f: FileJob<'_>) -> (u32, Outcome) {
    let idx = f.idx;
    let outcome = copy_file_inner(&mut f).await;
    (idx, outcome)
}

async fn copy_file_inner(f: &mut FileJob<'_>) -> Outcome {
    let classify = |e: AppError| {
        let e = lost_as_disconnected(Some(f.sftp), e);
        if is_fatal(&e) {
            Outcome::Fatal(e)
        } else {
            Outcome::Fail(e)
        }
    };
    let _slot = match f.gate.acquire().await {
        Ok(p) => p,
        Err(e) => return Outcome::Fatal(e),
    };
    if f.live.stop.load(Ordering::Relaxed) {
        return Outcome::Fatal(AppError::new(ErrorCode::Cancelled));
    }
    if let Err(e) = path::validate(&f.remote, "path") {
        return Outcome::Fail(e);
    }
    let mut cancel = Some(f.gate.cancel_signal());
    let attrs = match super::super::save::or_cancelled(&mut cancel, f.sftp.lstat(&f.remote)).await {
        Ok(a) => a,
        Err(e) => return classify(e),
    };
    if let Err(o) = check_remote(&attrs, &f.remote) {
        return o;
    }
    let (tree, dir, name) = (f.tree.clone(), f.dir.clone(), f.name.clone());
    let probe = match blocking(move || {
        tree.probe(&dir, &name)
            .map_err(|e| tree_error(e, &tree.shown(&dir)))
    })
    .await
    {
        Ok(p) => p,
        Err(e) => return classify(e),
    };
    let source = ConflictSide {
        size: attrs.size,
        mtime: attrs.mtime.map(i64::from),
    };
    let shown = || {
        let mut p = f.tree.shown(&f.dir);
        p.push(&f.name);
        p.display().to_string()
    };
    let rel = || f.rel_path.clone();
    let replace = match target_action(probe, f.mode, source, f.idx, &rel, &shown) {
        Ok(r) => r,
        Err(o) => return o,
    };
    *f.live
        .current
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(f.display.clone());
    let times = if f.env.preserve_times {
        local_times(attrs.atime, attrs.mtime)
    } else {
        None
    };
    let dest = Arc::new(Dest::Tree {
        tree: f.tree.clone(),
        dir: f.dir.clone(),
        name: f.name.clone(),
        replace,
    });
    let ctx = ItemCtx {
        session: &f.env.session,
        sftp: f.sftp,
        gate: &f.gate,
        conflict: Conflict::Overwrite,
    };
    let live = f.live;
    let mut last = 0u64;
    let result = download_item(&ctx, &f.remote, dest, times, &mut |n| {
        live.moved
            .fetch_add(n.saturating_sub(last), Ordering::Relaxed);
        last = n;
    })
    .await;
    match result {
        Ok(Item::Copied(n)) => Outcome::Copied(n),
        Ok(Item::Skipped) => Outcome::Skip(SkipReason::Exists),
        Err(e) => {
            live.moved.fetch_sub(last, Ordering::Relaxed);
            // The target appeared since the probe: never replaced, decided like a conflict.
            if matches!(e.kind, ErrorCode::LocalExists { .. }) && !replace {
                return match f.mode {
                    Mode::Ask => Outcome::Conflict(ConflictSample {
                        item_id: f.idx,
                        rel_path: f.rel_path.clone(),
                        source,
                        target: ConflictSide {
                            size: None,
                            mtime: None,
                        },
                    }),
                    _ => Outcome::Skip(SkipReason::Exists),
                };
            }
            classify(e)
        }
    }
}

/// Counters of a copy pass.
#[derive(Default)]
struct Tally {
    files_done: u64,
    failed: u64,
    skipped: SkipCounts,
    conflicts: u64,
    sample: Vec<ConflictSample>,
    conflicts_dirty: bool,
    /// The `round` of the `conflicts` events: a decision ends the round ([`requeue_conflicts`]),
    /// so the webview can drop an event of an answered round that was still on its way.
    round: u32,
    consecutive: u32,
}

#[allow(clippy::too_many_arguments)]
async fn copy(
    env: &JobEnv,
    sftp: &Sftp,
    state: &Arc<TreeState>,
    root: Root,
    gate: Gate,
    mut inputs: Inputs,
    mut report: Report,
    mut mode: Mode,
) -> JobEnd {
    let m = &state.manifest;
    let tree = root.tree.clone();
    let mut tally = Tally {
        skipped: m.stats.skipped,
        failed: u64::try_from(m.unlisted.len()).unwrap_or(0),
        ..Tally::default()
    };
    let mut created_dirs: Vec<u32> = Vec::new();
    let mut work: VecDeque<u32> = VecDeque::new();
    let (files_total, total) = {
        let status = state.statuses();
        let mut files = 0u64;
        let mut bytes = 0u64;
        for (i, s) in status.iter().enumerate().skip(1) {
            match s {
                St::Pending => {
                    if let Ok(i) = u32::try_from(i) {
                        work.push_back(i);
                    }
                    if let Some(n) = m.nodes.get(i)
                        && n.kind == NodeKind::File
                    {
                        files += 1;
                        bytes = bytes.saturating_add(n.size);
                    }
                }
                St::Done if m.nodes.get(i).is_some_and(|n| n.kind == NodeKind::File) => {
                    tally.files_done += 1;
                }
                _ => {}
            }
        }
        (files + tally.files_done, bytes)
    };
    let live = Live {
        moved: AtomicU64::new(0),
        total: AtomicU64::new(total),
        stop: AtomicBool::new(false),
        current: Mutex::new(None),
    };
    let mut fatal: Option<AppError> = None;
    let mut decisions_open = true;
    let mut inflight = FuturesUnordered::new();
    let mut tick = tokio::time::interval(PROGRESS_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let cancelled = gate.cancelled_fn();
    let max_files = env.max_files.max(1);
    let progress = |report: &mut Report, tally: &Tally, force: bool| {
        if Report::due(&mut report.last_progress, force) {
            report.send(TransferEvent::Progress {
                transfer_id: env.id.clone(),
                bytes: live.moved.load(Ordering::Relaxed),
                total: Some(live.total.load(Ordering::Relaxed)),
                files_done: Some(tally.files_done),
                files_total: Some(files_total),
                current: live
                    .current
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone(),
                failed: Some(tally.failed),
                skipped: Some(tally.skipped),
            });
        }
    };
    let send_conflicts = |report: &mut Report, tally: &mut Tally| {
        tally.conflicts_dirty = false;
        report.send(TransferEvent::Conflicts {
            transfer_id: env.id.clone(),
            count: tally.conflicts,
            sample: tally.sample.clone(),
            round: tally.round,
        });
    };
    // The copy starts (also right after `started` of a retry, which has no scan).
    progress(&mut report, &tally, true);
    // Directories first, in walk order (parents before children), in one blocking pass, so no
    // file future waits unpolled behind a directory creation.
    let dirs: Vec<u32> = work
        .iter()
        .copied()
        .filter(|&i| m.node(i).is_some_and(|n| n.kind == NodeKind::Dir))
        .collect();
    work.retain(|&i| m.node(i).is_some_and(|n| n.kind == NodeKind::File));
    if !dirs.is_empty() {
        // Recorded as they are created: a pass that ends in a panic still has its directories
        // cleaned up (removed on cancel, Unix: default mode).
        let made = Arc::new(Mutex::new(Vec::new()));
        let (st, t, stop, mk) = (
            state.clone(),
            tree.clone(),
            gate.cancelled_fn(),
            made.clone(),
        );
        let results = blocking(move || Ok(create_dirs(&st, &t, &dirs, &stop, &mk))).await;
        created_dirs.append(&mut made.lock().unwrap_or_else(PoisonError::into_inner));
        match results {
            Ok(results) => {
                dir_results(
                    &mut report,
                    &mut tally,
                    results,
                    |i| m.rel_path(i),
                    &mut fatal,
                );
                tally.consecutive = 0;
            }
            Err(e) => fatal = Some(e),
        }
        progress(&mut report, &tally, true);
    }
    loop {
        // Dispatch: files up to `max_files` in flight (each waits for a session slot).
        while fatal.is_none() && !cancelled() && inflight.len() < max_files {
            let Some(i) = work.pop_front() else { break };
            let Some(node) = m.node(i) else { continue };
            if !parent_done(state, node.parent) {
                set(state, i, St::Blocked);
                continue;
            }
            let comps = m.local_components(i);
            match node.kind {
                // Created in the pass above.
                NodeKind::Dir => {}
                NodeKind::File => {
                    let (dir, name) = match comps.split_last() {
                        Some((last, dir)) => (dir.to_vec(), last.clone()),
                        None => continue,
                    };
                    inflight.push(copy_file(FileJob {
                        env,
                        sftp,
                        gate: gate.fork(),
                        tree: tree.clone(),
                        live: &live,
                        idx: i,
                        remote: m.remote_path(i),
                        rel_path: m.rel_path(i),
                        dir,
                        name,
                        display: node.name.to_string(),
                        mode,
                    }));
                }
            }
        }
        if inflight.is_empty() {
            if fatal.is_some() || cancelled() {
                break;
            }
            if !work.is_empty() {
                continue;
            }
            let waiting = state.statuses().contains(&St::Conflict);
            if !waiting || mode != Mode::Ask {
                break;
            }
            // Everything else is done: wait for the conflict decision.
            progress(&mut report, &tally, true);
            send_conflicts(&mut report, &mut tally);
            match inputs.wait(|d| Mode::of_decision(d).is_some()).await {
                Ok(d) => {
                    mode = Mode::of_decision(d).unwrap_or(Mode::Skip);
                    requeue_conflicts(state, &mut work, &mut tally);
                    continue;
                }
                Err(e) => {
                    fatal = Some(e);
                    break;
                }
            }
        }
        tokio::select! {
            biased;
            Some((i, outcome)) = inflight.next() => {
                let scanned = m.node(i).map_or(0, |n| n.size);
                // `total` = the bytes this pass copies: a file's real size replaces the scanned
                // one, skipped and failed files leave it (only this loop changes `total`).
                let adjust = |actual: u64| {
                    let t = live.total.load(Ordering::Relaxed);
                    let t = t.saturating_sub(scanned).saturating_add(actual);
                    live.total.store(t, Ordering::Relaxed);
                };
                match outcome {
                    Outcome::Copied(n) => {
                        adjust(n);
                        set(state, i, St::Done);
                        tally.files_done += 1;
                        tally.consecutive = 0;
                    }
                    Outcome::Skip(reason) => {
                        adjust(0);
                        set(state, i, St::Skipped);
                        tally.skipped.add(reason);
                        report.issue(Some(i), &m.rel_path(i), ItemIssue::Skipped { reason });
                    }
                    // Set aside under `ask`, but the decision came while it was in flight:
                    // decided like the others now.
                    Outcome::Conflict(_) if mode != Mode::Ask => {
                        set(state, i, St::Pending);
                        work.push_back(i);
                    }
                    Outcome::Conflict(sample) => {
                        set(state, i, St::Conflict);
                        tally.conflicts += 1;
                        let first = tally.conflicts == 1;
                        if tally.sample.len() < MAX_CONFLICT_SAMPLE {
                            tally.sample.push(sample);
                        }
                        if first {
                            send_conflicts(&mut report, &mut tally);
                        } else {
                            tally.conflicts_dirty = true;
                        }
                    }
                    Outcome::Fail(e) => {
                        adjust(0);
                        set(state, i, St::Failed);
                        fail(&mut report, &mut tally, i, &m.rel_path(i), e, &mut fatal);
                    }
                    Outcome::Fatal(e) => {
                        // Not started or cut off: retried later.
                        set(state, i, St::Pending);
                        if fatal.is_none() && e.kind != ErrorCode::Cancelled {
                            fatal = Some(e);
                        }
                    }
                }
                if fatal.is_some() {
                    live.stop.store(true, Ordering::Relaxed);
                }
                progress(&mut report, &tally, false);
            }
            d = inputs.decisions.recv(), if mode == Mode::Ask && decisions_open => {
                // The conflicts dialog answered while other files still copy.
                match d {
                    None => decisions_open = false,
                    Some(TransferDecision::Cancel) => {
                        live.stop.store(true, Ordering::Relaxed);
                        fatal = Some(AppError::new(ErrorCode::Cancelled));
                    }
                    Some(d) => {
                        if let Some(m2) = Mode::of_decision(d) {
                            mode = m2;
                            requeue_conflicts(state, &mut work, &mut tally);
                        }
                    }
                }
            }
            _ = tick.tick() => {
                progress(&mut report, &tally, false);
                if tally.conflicts_dirty && mode == Mode::Ask {
                    send_conflicts(&mut report, &mut tally);
                }
            }
        }
    }
    let was_cancelled = cancelled()
        || fatal
            .as_ref()
            .is_some_and(|e| e.kind == ErrorCode::Cancelled);
    drop(inflight);
    drop(tree);
    cleanup(root, m, &created_dirs, was_cancelled).await;
    if was_cancelled {
        return JobEnd::Cancelled;
    }
    if let Some(e) = fatal {
        let e = lost_as_disconnected(Some(sftp), e);
        return end_err(e, Some(Kept::State(state.clone())));
    }
    progress(&mut report, &tally, true);
    // Failed files and folders (a folder that could not be listed is listed again on retry).
    let retryable = state
        .statuses()
        .iter()
        .any(|s| matches!(s, St::Failed | St::Blocked));
    JobEnd::Done {
        bytes: live.moved.load(Ordering::Relaxed),
        summary: FolderSummary {
            local_name: state
                .root_name
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
            files_done: tally.files_done,
            skipped: tally.skipped,
            failed: tally.failed,
            renamed: m.stats.renamed,
            retryable,
        },
        keep: retryable.then(|| state.clone()),
    }
}

fn parent_done(state: &TreeState, parent: u32) -> bool {
    let status = state.statuses();
    usize::try_from(parent)
        .ok()
        .and_then(|p| status.get(p))
        .is_some_and(|s| *s == St::Done)
}

/// Create (or find) the job's directories in walk order; a directory whose parent failed is
/// `Blocked`. Stops on cancel and after [`MAX_CONSECUTIVE_FAILURES`] failures in a row (the
/// caller then stops the job). Returns each directory's result (`true` = created now); the
/// directories created now are also appended to `made` at once (it survives a panic).
fn create_dirs(
    state: &TreeState,
    tree: &LocalTree,
    dirs: &[u32],
    cancelled: &(dyn Fn() -> bool + Send + Sync),
    made: &Mutex<Vec<u32>>,
) -> Vec<(u32, Result<bool, AppError>)> {
    let m = &state.manifest;
    let mut out = Vec::new();
    let mut consecutive = 0u32;
    for &i in dirs {
        if cancelled() || consecutive >= MAX_CONSECUTIVE_FAILURES {
            break;
        }
        let Some(node) = m.node(i) else { continue };
        if !parent_done(state, node.parent) {
            set(state, i, St::Blocked);
            continue;
        }
        let comps = m.local_components(i);
        let r = tree
            .ensure_dir(&comps)
            .map_err(|e| tree_error(e, &tree.shown(&comps)));
        match &r {
            Ok(created) => {
                if *created {
                    made.lock().unwrap_or_else(PoisonError::into_inner).push(i);
                }
                set(state, i, St::Done);
                consecutive = 0;
            }
            Err(_) => {
                set(state, i, St::Failed);
                consecutive += 1;
            }
        }
        out.push((i, r));
    }
    out
}

/// Counts the results of the directory pass like file outcomes: a failure is an issue (a fatal
/// one stops the job), a success ends a run of consecutive failures.
fn dir_results(
    report: &mut Report,
    tally: &mut Tally,
    results: Vec<(u32, Result<bool, AppError>)>,
    rel_path: impl Fn(u32) -> String,
    fatal: &mut Option<AppError>,
) {
    for (i, r) in results {
        match r {
            Ok(_) => tally.consecutive = 0,
            Err(e) => {
                if is_fatal(&e) && fatal.is_none() {
                    *fatal = Some(e.clone());
                }
                fail(report, tally, i, &rel_path(i), e, fatal);
            }
        }
    }
}

fn set(state: &TreeState, i: u32, s: St) {
    let mut status = state.statuses();
    if let Some(slot) = usize::try_from(i).ok().and_then(|i| status.get_mut(i)) {
        *slot = s;
    }
}

fn fail(
    report: &mut Report,
    tally: &mut Tally,
    i: u32,
    rel_path: &str,
    e: AppError,
    fatal: &mut Option<AppError>,
) {
    tally.failed += 1;
    tally.consecutive += 1;
    report.issue(
        Some(i),
        rel_path,
        ItemIssue::Failed {
            error: e.clone(),
            retryable: true,
        },
    );
    if tally.consecutive >= MAX_CONSECUTIVE_FAILURES && fatal.is_none() {
        *fatal = Some(e);
    }
}

/// After a conflict decision: the set-aside files run again under the new mode.
fn requeue_conflicts(state: &TreeState, work: &mut VecDeque<u32>, tally: &mut Tally) {
    let mut status = state.statuses();
    for (i, s) in status.iter_mut().enumerate() {
        if *s == St::Conflict {
            *s = St::Pending;
            if let Ok(i) = u32::try_from(i) {
                work.push_back(i);
            }
        }
    }
    tally.conflicts = 0;
    tally.sample.clear();
    tally.conflicts_dirty = false;
    tally.round = tally.round.saturating_add(1);
}

/// The end of a job: on cancel, directories it created and left empty are removed (never
/// recursive, decision D10), the destination root too when the job created it; then (Unix) the
/// remaining created directories get their default mode. Children before parents (reverse walk
/// order). Takes the last reference to the tree (Windows: the root handle must be closed
/// before the root can be removed).
async fn cleanup(root: Root, m: &Manifest, created: &[u32], cancelled: bool) {
    let dirs: Vec<Vec<String>> = created
        .iter()
        .rev()
        .map(|&i| m.local_components(i))
        .collect();
    let Root {
        tree,
        created: root_created,
    } = root;
    let _ = blocking(move || {
        for d in &dirs {
            if cancelled {
                tree.remove_if_empty(d);
            }
            tree.finish_dir(d);
        }
        if root_created {
            tree.finish_dir(&[]);
            if cancelled && let Ok(t) = Arc::try_unwrap(tree) {
                t.remove_root_if_empty();
            }
        }
        Ok(())
    })
    .await;
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    #[derive(Default)]
    struct Sink(Mutex<Vec<TransferEvent>>);
    impl TransferSink for Sink {
        fn send(&self, event: TransferEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    fn attrs(perm: Option<u32>) -> FileAttributes {
        FileAttributes {
            permissions: perm,
            size: Some(1),
            ..FileAttributes::empty()
        }
    }

    /// The type check `download_item` relies on: only a regular file is downloaded; the
    /// server's scan-time attributes are not trusted (a file that became a link, FIFO,
    /// device or directory since the scan, or lost its type).
    #[test]
    fn only_regular_files_reach_download_item() {
        assert!(check_remote(&attrs(Some(0o100_644)), "/f").is_ok());
        assert!(matches!(
            check_remote(&attrs(Some(0o120_777)), "/f"),
            Err(Outcome::Skip(SkipReason::Symlink))
        ));
        for special in [0o010_644, 0o020_666, 0o060_660, 0o140_777] {
            assert!(matches!(
                check_remote(&attrs(Some(special)), "/f"),
                Err(Outcome::Skip(SkipReason::Special))
            ));
        }
        assert!(matches!(
            check_remote(&attrs(Some(0o040_755)), "/f"),
            Err(Outcome::Fail(AppError {
                kind: ErrorCode::IsADirectory { .. },
                ..
            }))
        ));
        assert!(matches!(
            check_remote(&attrs(None), "/f"),
            Err(Outcome::Skip(SkipReason::UnknownType))
        ));
    }

    #[test]
    fn target_actions_follow_the_mode() {
        let src = ConflictSide {
            size: Some(5),
            mtime: Some(200),
        };
        let rel = || "a/b".to_owned();
        let shown = || "/l/a/b".to_owned();
        let file = |mtime| Probe::File {
            size: 3,
            mtime: Some(mtime),
            ours: false,
        };
        let act = |p, m| target_action(p, m, src, 7, &rel, &shown);
        assert!(matches!(act(Probe::Missing, Mode::Ask), Ok(false)));
        assert!(matches!(act(file(100), Mode::Replace), Ok(true)));
        assert!(matches!(
            act(file(100), Mode::Skip),
            Err(Outcome::Skip(SkipReason::Exists))
        ));
        assert!(matches!(act(file(100), Mode::ReplaceOlder), Ok(true)));
        // Equal mtimes (1 s resolution): not older.
        assert!(matches!(
            act(file(200), Mode::ReplaceOlder),
            Err(Outcome::Skip(SkipReason::Exists))
        ));
        match act(file(100), Mode::Ask) {
            Err(Outcome::Conflict(s)) => {
                assert_eq!((s.item_id, s.rel_path.as_str()), (7, "a/b"));
                assert_eq!(s.target.size, Some(3));
            }
            _ => panic!("expected a conflict"),
        }
        // A file this job wrote is never replaced, whatever the mode.
        let ours = Probe::File {
            size: 1,
            mtime: None,
            ours: true,
        };
        assert!(matches!(
            act(ours, Mode::Replace),
            Err(Outcome::Skip(SkipReason::NameCollision))
        ));
        assert!(matches!(
            act(Probe::Dir, Mode::Replace),
            Err(Outcome::Fail(_))
        ));
        let link = Probe::Link { dir: false };
        assert!(matches!(act(link, Mode::Replace), Ok(true)));
        assert!(matches!(
            act(link, Mode::ReplaceOlder),
            Err(Outcome::Skip(SkipReason::Exists))
        ));
        // A junction at a file's target: a folder in the way, also under Replace.
        assert!(matches!(
            act(Probe::Link { dir: true }, Mode::Replace),
            Err(Outcome::Fail(AppError {
                kind: ErrorCode::LocalIsADirectory { .. },
                ..
            }))
        ));
    }

    /// A flood of issues and progress reports from a 100 000-file job: at most 1 000 issue
    /// events and progress at most every 100 ms.
    #[test]
    fn events_towards_the_webview_are_bounded() {
        let sink = Arc::new(Sink::default());
        let mut r = Report::new("t", sink.clone());
        let stats = ScanStats::default();
        let start = Instant::now();
        for i in 0..100_000u32 {
            r.skipped(&format!("f{i}"), SkipReason::Symlink);
            ScanEvents::progress(&mut r, &stats);
        }
        let elapsed = start.elapsed().as_millis() as usize;
        let events = sink.0.lock().unwrap();
        let issues = events
            .iter()
            .filter(|e| matches!(e, TransferEvent::ItemIssue { .. }))
            .count();
        let progress = events
            .iter()
            .filter(|e| matches!(e, TransferEvent::ScanProgress { .. }))
            .count();
        assert_eq!(issues, MAX_ISSUES);
        assert!(progress <= elapsed / 100 + 1, "{progress} in {elapsed} ms");
    }

    #[tokio::test]
    async fn decisions_wait_for_a_fitting_answer_or_cancel() {
        let (tx, rx) = mpsc::channel(8);
        let (ctx, crx) = watch::channel(false);
        let mut inputs = Inputs::new(rx, crx);
        tx.send(TransferDecision::ReplaceAll).await.unwrap(); // does not fit: ignored
        tx.send(TransferDecision::Merge).await.unwrap();
        let d = inputs.wait(|d| d == TransferDecision::Merge).await.unwrap();
        assert_eq!(d, TransferDecision::Merge);
        // A stale answer (a double click on the previous prompt) never answers the next one.
        tx.send(TransferDecision::Proceed).await.unwrap();
        inputs.drain();
        tx.send(TransferDecision::Merge).await.unwrap();
        assert_eq!(
            inputs
                .wait(|d| d == TransferDecision::Proceed || d == TransferDecision::Merge)
                .await
                .unwrap(),
            TransferDecision::Merge
        );
        tx.send(TransferDecision::Cancel).await.unwrap();
        assert_eq!(
            inputs.wait(|_| true).await.unwrap_err().kind,
            ErrorCode::Cancelled
        );
        ctx.send(true).unwrap();
        assert_eq!(
            inputs.wait(|_| true).await.unwrap_err().kind,
            ErrorCode::Cancelled
        );
    }

    /// A folder the first scan could not list is listed on retry: its subtree joins the job
    /// (pending, under the right parent, reported relative to the root); one that still cannot
    /// be listed stays a failure.
    #[tokio::test]
    async fn retry_lists_folders_that_failed_before() {
        use crate::sftp::tree::tests::{DIR, FILE, MemTree};
        let mut t = MemTree::default();
        t.dir(
            "/r",
            &[
                ("locked", Some(DIR), 0),
                ("still", Some(DIR), 0),
                ("ok", Some(FILE), 1),
            ],
        );
        t.dir(
            "/r/locked",
            &[("secret", Some(FILE), 5), ("sub", Some(DIR), 0)],
        );
        t.dir("/r/locked/sub", &[("deep", Some(FILE), 7)]);
        t.dir("/r/still", &[("x", Some(FILE), 1)]);
        t.denied.insert("/r/locked".into());
        t.denied.insert("/r/still".into());
        let (_tx, rx) = watch::channel(false);
        let sink = Arc::new(Sink::default());
        let mut report = Report::new("t", sink.clone());
        let manifest = scan(&t, "/r", "r", rx.clone(), &mut report).await.unwrap();
        assert_eq!(manifest.unlisted.len(), 2);
        let status = (0..manifest.nodes.len())
            .map(|i| {
                if i == 0 {
                    St::Done
                } else if manifest.unlisted.contains(&(i as u32)) {
                    St::Failed
                } else {
                    St::Done
                }
            })
            .collect();
        let state = Arc::new(TreeState {
            spec: TreeSpec::download("/r".into(), PathBuf::from("p"), "r".into()),
            manifest,
            status: Mutex::new(status),
            root_name: Mutex::new("r".into()),
            root_id: Mutex::new(None),
            created: CreatedSet::default(),
        });
        t.denied.remove("/r/locked");
        let state = rescan_unlisted(state, &t, rx, &mut report).await.unwrap();
        let m = &state.manifest;
        assert_eq!(m.unlisted.len(), 1, "/r/still is still unreadable");
        assert_eq!(m.stats.files, 3);
        let find = |name: &str| {
            (1..m.len())
                .find(|&i| &*m.nodes[i as usize].name == name)
                .unwrap()
        };
        assert_eq!(m.remote_path(find("deep")), "/r/locked/sub/deep");
        assert_eq!(m.rel_path(find("secret")), "locked/secret");
        let status = state.statuses();
        for name in ["locked", "secret", "sub", "deep"] {
            assert_eq!(status[find(name) as usize], St::Pending, "{name}");
        }
        assert_eq!(status[find("still") as usize], St::Failed);
        let events = sink.0.lock().unwrap();
        assert!(events.iter().any(|e| matches!(
            e,
            TransferEvent::ItemIssue { rel_path, .. } if rel_path == "still"
        )));
    }

    /// A job state over `/r` with the directories `a`, `b`, `c` and the files `a/x`, `b/y`
    /// (all pending, the root done).
    async fn small_state() -> Arc<TreeState> {
        use crate::sftp::tree::tests::{DIR, FILE, MemTree};
        let mut t = MemTree::default();
        t.dir(
            "/r",
            &[
                ("a", Some(DIR), 0),
                ("b", Some(DIR), 0),
                ("c", Some(DIR), 0),
            ],
        );
        t.dir("/r/a", &[("x", Some(FILE), 1)]);
        t.dir("/r/b", &[("y", Some(FILE), 1)]);
        t.dir("/r/c", &[]);
        let (_tx, rx) = watch::channel(false);
        let mut report = Report::new("t", Arc::new(Sink::default()));
        let manifest = scan(&t, "/r", "r", rx, &mut report).await.unwrap();
        let status = (0..manifest.nodes.len())
            .map(|i| if i == 0 { St::Done } else { St::Pending })
            .collect();
        Arc::new(TreeState {
            spec: TreeSpec::download("/r".into(), PathBuf::from("p"), "r".into()),
            manifest,
            status: Mutex::new(status),
            root_name: Mutex::new("r".into()),
            root_id: Mutex::new(None),
            created: CreatedSet::default(),
        })
    }

    fn find(m: &Manifest, name: &str) -> u32 {
        (1..m.len())
            .find(|&i| &*m.nodes[i as usize].name == name)
            .unwrap()
    }

    /// A decision ends the conflict round: the set-aside files run again and later `conflicts`
    /// events carry the next round, so the webview can tell a stale event of the answered
    /// round (sent before the job saw the answer) from a new question.
    #[tokio::test]
    async fn a_conflict_decision_starts_a_new_round() {
        let state = small_state().await;
        let (x, y) = (find(&state.manifest, "x"), find(&state.manifest, "y"));
        set(&state, x, St::Conflict);
        set(&state, y, St::Done);
        let mut tally = Tally {
            conflicts: 1,
            conflicts_dirty: true,
            ..Tally::default()
        };
        assert_eq!(tally.round, 0, "the first round");
        let mut work = VecDeque::new();
        requeue_conflicts(&state, &mut work, &mut tally);
        assert_eq!(work, VecDeque::from([x]));
        assert_eq!(state.statuses()[x as usize], St::Pending);
        assert_eq!((tally.conflicts, tally.conflicts_dirty), (0, false));
        assert_eq!(tally.round, 1);
        requeue_conflicts(&state, &mut work, &mut tally);
        assert_eq!(tally.round, 2, "every decision ends a round");
        tally.round = u32::MAX;
        requeue_conflicts(&state, &mut work, &mut tally);
        assert_eq!(
            tally.round,
            u32::MAX,
            "never wraps back to an answered round"
        );
    }

    /// The directories created before the pass panicked are still known (cleanup removes them
    /// on cancel and, on Unix, gives them their default mode).
    #[tokio::test]
    async fn directories_created_before_a_panic_are_recorded() {
        let state = small_state().await;
        let dir = crate::platform::test_support::private_tempdir();
        let tree = Arc::new(LocalTree::open(dir.path(), "t", true, CreatedSet::default()).unwrap());
        let dirs: Vec<u32> = ["a", "b", "c"]
            .iter()
            .map(|n| find(&state.manifest, n))
            .collect();
        let made = Arc::new(Mutex::new(Vec::new()));
        let calls = Arc::new(AtomicU64::new(0));
        let (st, t, mk, d, c) = (
            state.clone(),
            tree.clone(),
            made.clone(),
            dirs.clone(),
            calls.clone(),
        );
        let r = blocking(move || {
            // The third directory's turn panics (a stand-in for any panic in the pass).
            let stop = move || {
                assert!(c.fetch_add(1, Ordering::Relaxed) < 2, "boom");
                false
            };
            Ok(create_dirs(&st, &t, &d, &stop, &mk))
        })
        .await;
        assert!(r.is_err());
        assert_eq!(*made.lock().unwrap(), dirs[..2].to_vec());
        assert!(dir.path().join("t").join("a").is_dir());
        assert!(!dir.path().join("t").join("c").exists());
    }

    /// A successful directory ends a run of failures: failures spread among successes never
    /// stop the job as "too many consecutive failures", 50 in a row do.
    #[test]
    fn directory_successes_reset_the_failure_run() {
        let denied = || AppError::new(ErrorCode::PermissionDenied { path: "d".into() });
        let mut report = Report::new("t", Arc::new(Sink::default()));
        let mut tally = Tally::default();
        let mut fatal = None;
        let spread: Vec<_> = (0..2 * MAX_CONSECUTIVE_FAILURES)
            .map(|i| (i, if i % 2 == 0 { Err(denied()) } else { Ok(true) }))
            .collect();
        dir_results(
            &mut report,
            &mut tally,
            spread,
            |i| i.to_string(),
            &mut fatal,
        );
        assert!(fatal.is_none(), "{fatal:?}");
        assert_eq!(tally.failed, u64::from(MAX_CONSECUTIVE_FAILURES));
        assert_eq!(tally.consecutive, 0);
        let run: Vec<_> = (0..MAX_CONSECUTIVE_FAILURES)
            .map(|i| (i, Err(denied())))
            .collect();
        dir_results(&mut report, &mut tally, run, |i| i.to_string(), &mut fatal);
        assert!(fatal.is_some());
    }

    #[test]
    fn modes_from_policy_and_decisions() {
        assert_eq!(Mode::of_policy(ConflictPolicy::Ask), Mode::Ask);
        assert_eq!(Mode::of_policy(ConflictPolicy::Overwrite), Mode::Replace);
        assert_eq!(Mode::of_policy(ConflictPolicy::Skip), Mode::Skip);
        assert_eq!(
            Mode::of_decision(TransferDecision::ReplaceOlder),
            Some(Mode::ReplaceOlder)
        );
        assert_eq!(Mode::of_decision(TransferDecision::Merge), None);
        assert!(retryable_job_error(&AppError::new(
            ErrorCode::LocalDiskFull {
                needed: None,
                available: None
            }
        )));
        assert!(matches!(
            end_err(AppError::new(ErrorCode::Cancelled), None),
            JobEnd::Cancelled
        ));
    }
}
