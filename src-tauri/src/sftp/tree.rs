//! Remote folder walk for folder downloads (§6.6 "Folders").
//!
//! Scan first, copy afterwards: the whole tree is listed into a compact [`Manifest`] before
//! anything is written, so the hard limits refuse a hostile or huge tree without leaving a
//! partial copy behind. Breadth-first with an explicit queue (no recursion), at most
//! [`SCAN_CONCURRENCY`] directories read at once (OpenSSH's sftp-server is single-threaded; the
//! tree and the editor stay responsive).
//!
//! Server names are hostile: `.`/`..` are ignored, empty names and names with `/` or NUL are
//! skipped (`invalidName`), lossy names (U+FFFD, the server name differs) are skipped
//! (`lossyName`, a directory with its subtree), symlinks and special files are never followed
//! or opened (`symlink`, `special`; kinds come from the READDIR attributes, which OpenSSH sends
//! as `lstat` attributes), entries without a type get one `lstat` (`unknownType` if still
//! unknown). Every component gets a local name through [`sanitize_component`]; a second entry
//! of the same directory with the same (case-folded on Windows/macOS) local name is skipped
//! (`nameCollision`). No full paths are stored: they are built on demand from the parents.

use std::collections::{HashSet, VecDeque};
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use russh_sftp::protocol::{FileAttributes, StatusCode};
use tokio::sync::watch;

use crate::error::{AppError, ErrorCode, TimeoutOp, TreeLimit};
use crate::ipc::dto::{FileKind, SkipCounts, SkipReason};

use super::client::{LIST_MAX_BYTES, LIST_MAX_ENTRIES, LIST_TIMEOUT, Sftp, is_status, kind_of};
use super::names::sanitize_local_name;
use super::path;

/// Most directory levels below the chosen folder.
pub const MAX_DEPTH: usize = 64;
/// Most entries of one job: files, directories and skipped entries.
pub const MAX_ENTRIES: usize = 200_000;
/// Memory charged for one job's manifest (names plus [`NODE_OVERHEAD`] per entry).
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024 * 1024;
/// Whole scan.
pub const SCAN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Directories read at once.
pub const SCAN_CONCURRENCY: usize = 4;
/// Charged per entry besides its names: the `Node`, its status byte, the `Box` headers and the
/// transient READDIR entry.
pub const NODE_OVERHEAD: usize = 128;
/// Soft threshold: above it the job asks before copying (decision D2).
pub const SOFT_MAX_FILES: u64 = 10_000;
pub const SOFT_MAX_BYTES: u64 = 10 * 1024 * 1024 * 1024;
/// Longest `relPath` sent to the WebView (bytes).
pub const MAX_REL_PATH: usize = 4096;

/// What a manifest entry is (links and special files never get one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Dir,
    File,
}

/// One directory or file of the tree. Index 0 is the root.
#[derive(Debug)]
pub struct Node {
    pub parent: u32,
    /// The server's name (one component).
    pub name: Box<str>,
    /// The local name when it differs from `name` ([`sanitize_component`]).
    pub local: Option<Box<str>>,
    pub kind: NodeKind,
    pub size: u64,
    pub mtime: Option<u32>,
    pub atime: Option<u32>,
}

impl Node {
    pub fn local_name(&self) -> &str {
        self.local.as_deref().unwrap_or(&self.name)
    }
}

/// Totals of a scan.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanStats {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub skipped: SkipCounts,
    /// Entries whose local name differs from the server name.
    pub renamed: u64,
    /// Directories that could not be listed.
    pub failed: u64,
}

/// The scanned tree: nodes in breadth-first order (a parent always has a lower index than its
/// children, so reverse index order is a valid post-order).
#[derive(Debug)]
pub struct Manifest {
    /// The remote root path (after a root symlink was resolved).
    pub root: String,
    pub nodes: Vec<Node>,
    pub stats: ScanStats,
    /// Directories whose listing failed (an item failure each; nothing below them is known).
    pub unlisted: Vec<u32>,
}

impl Manifest {
    pub fn node(&self, i: u32) -> Option<&Node> {
        self.nodes.get(usize::try_from(i).ok()?)
    }

    /// The node indices from the root's child down to `i` (empty for the root). Bounded by the
    /// depth limit; a broken parent chain (never built by [`scan`]) ends the walk.
    fn chain(&self, i: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let mut cur = i;
        while cur != 0 && out.len() <= MAX_DEPTH + 1 {
            let Some(n) = self.node(cur) else { break };
            out.push(cur);
            if n.parent >= cur {
                break;
            }
            cur = n.parent;
        }
        out.reverse();
        out
    }

    /// The remote path of node `i`.
    pub fn remote_path(&self, i: u32) -> String {
        let mut p = self.root.clone();
        for c in self.chain(i) {
            if let Some(n) = self.node(c) {
                p = path::join(&p, &n.name);
            }
        }
        p
    }

    /// Server names from the root's child to `i`, joined with `/` (server text, ≤ 4096 bytes).
    pub fn rel_path(&self, i: u32) -> String {
        let names: Vec<&str> = self
            .chain(i)
            .into_iter()
            .filter_map(|c| self.node(c).map(|n| &*n.name))
            .collect();
        cap_rel(names.join("/"))
    }

    /// Local names from the root's child to `i` (the local root folder itself is not included).
    pub fn local_components(&self, i: u32) -> Vec<String> {
        self.chain(i)
            .into_iter()
            .filter_map(|c| self.node(c).map(|n| n.local_name().to_owned()))
            .collect()
    }

    /// Number of nodes as `u32` (bounded by [`MAX_ENTRIES`]).
    pub fn len(&self) -> u32 {
        u32::try_from(self.nodes.len()).unwrap_or(u32::MAX)
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.len() <= 1
    }
}

/// `rel` cut to [`MAX_REL_PATH`] bytes at a character boundary.
fn cap_rel(mut rel: String) -> String {
    if rel.len() > MAX_REL_PATH {
        let mut cut = MAX_REL_PATH;
        while !rel.is_char_boundary(cut) {
            cut -= 1;
        }
        rel.truncate(cut);
    }
    rel
}

/// The local name of one server name component: `\` becomes `_` (it is a separator on Windows,
/// so `sanitize_local_name` would keep only the part after it), then the download rules of
/// `sanitize_local_name` (separators and `<>:"|?*` and controls → `_`, invisible characters
/// removed, reserved device names suffixed, trailing dots and spaces trimmed, ≤ 200 bytes,
/// `download` when nothing is left).
pub fn sanitize_component(name: &str) -> String {
    sanitize_local_name(&name.replace('\\', "_"))
}

/// The key two local names collide under: case-folded where the usual file systems ignore case
/// (Windows, macOS); exact elsewhere. A normalization difference the folding misses is caught
/// by the writer's identity check.
pub fn collision_key(local: &str) -> String {
    if cfg!(any(windows, target_os = "macos")) {
        local.to_lowercase()
    } else {
        local.to_owned()
    }
}

/// One READDIR entry, reduced to what the walk needs.
#[derive(Debug, Clone)]
pub struct RawEntry {
    pub name: String,
    pub attrs: FileAttributes,
}

/// Where the walk reads directories from (production: the session's SFTP channel; tests:
/// hostile in-memory trees).
pub trait RemoteDirSource: Sync {
    type Handle: Send + Sync;
    /// `opendir`; errors are mapped (`permissionDenied`, `notADirectory`, …).
    fn open_dir(&self, path: &str) -> impl Future<Output = Result<Self::Handle, AppError>> + Send;
    /// One READDIR reply; `None` at the end.
    fn read_batch(
        &self,
        handle: &Self::Handle,
        path: &str,
    ) -> impl Future<Output = Result<Option<Vec<RawEntry>>, AppError>> + Send;
    fn close_dir(&self, handle: Self::Handle) -> impl Future<Output = ()> + Send;
    fn lstat(&self, path: &str) -> impl Future<Output = Result<FileAttributes, AppError>> + Send;
}

/// The job's entry and memory budget, charged as READDIR entries arrive (shared by the
/// concurrent directory reads, so the transient entries are bounded too).
#[derive(Debug, Default)]
pub struct Budget {
    entries: AtomicUsize,
    bytes: AtomicUsize,
}

impl Budget {
    fn charge(&self, name_len: usize) -> Result<(), AppError> {
        let entries = self
            .entries
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        if entries > MAX_ENTRIES {
            return Err(too_large(TreeLimit::Entries, MAX_ENTRIES));
        }
        // A sanitized local name can be stored too (≤ 200 bytes; charged as the name again).
        let cost = name_len.saturating_mul(2).saturating_add(NODE_OVERHEAD);
        let bytes = self
            .bytes
            .fetch_add(cost, Ordering::Relaxed)
            .saturating_add(cost);
        if bytes > MAX_MANIFEST_BYTES {
            return Err(too_large(TreeLimit::Memory, MAX_MANIFEST_BYTES));
        }
        Ok(())
    }

    pub fn charged_bytes(&self) -> usize {
        self.bytes.load(Ordering::Relaxed)
    }
}

pub fn too_large(limit: TreeLimit, value: usize) -> AppError {
    AppError::new(ErrorCode::TreeTooLarge {
        limit,
        value: u64::try_from(value).unwrap_or(u64::MAX),
    })
}

/// What the scan reports while it runs.
pub trait ScanEvents: Send {
    /// Totals so far (the caller throttles).
    fn progress(&mut self, stats: &ScanStats);
    /// An entry that will not be copied.
    fn skipped(&mut self, rel_path: &str, reason: SkipReason);
    /// A directory that could not be listed (an item failure; the scan goes on).
    fn failed(&mut self, item: u32, rel_path: &str, error: &AppError);
}

/// Errors that end the scan (the rest are item failures of one directory).
fn fatal(e: &AppError) -> bool {
    matches!(
        e.kind,
        ErrorCode::NotConnected { .. }
            | ErrorCode::Disconnected { .. }
            | ErrorCode::SftpUnavailable
            | ErrorCode::Cancelled
            | ErrorCode::Timeout { .. }
            | ErrorCode::TreeTooLarge { .. }
    )
}

/// One directory's complete listing under the §6.2 caps (entries, bytes, 60 s deadline).
/// A listing that hits a cap or the deadline fails the scan (`treeTooLarge{directory}`):
/// copying part of a folder without saying so is worse than refusing (decision D9).
async fn read_dir<S: RemoteDirSource>(
    src: &S,
    dir: &str,
    budget: &Budget,
    mut stop: watch::Receiver<bool>,
) -> Result<Vec<RawEntry>, AppError> {
    let cut = || too_large(TreeLimit::Directory, LIST_MAX_ENTRIES);
    let stopped = || AppError::new(ErrorCode::Cancelled);
    let deadline = tokio::time::Instant::now() + LIST_TIMEOUT;
    let handle = tokio::select! {
        biased;
        _ = stop.wait_for(|s| *s) => return Err(stopped()),
        h = tokio::time::timeout_at(deadline, src.open_dir(dir)) => h.map_err(|_| cut())??,
    };
    let mut out = Vec::new();
    let mut cost = 0usize;
    let read = async {
        while let Some(batch) = src.read_batch(&handle, dir).await? {
            for e in batch {
                if e.name == "." || e.name == ".." {
                    continue;
                }
                cost = cost.saturating_add(e.name.len().saturating_add(NODE_OVERHEAD));
                if out.len() >= LIST_MAX_ENTRIES || cost > LIST_MAX_BYTES {
                    return Err(cut());
                }
                budget.charge(e.name.len())?;
                out.push(e);
            }
        }
        Ok::<(), AppError>(())
    };
    // On a stop (cancel, another folder's fatal error, the scan deadline) the read ends here
    // and the handle is still closed.
    let result = tokio::select! {
        biased;
        _ = stop.wait_for(|s| *s) => Err(stopped()),
        r = tokio::time::timeout_at(deadline, read) => r.unwrap_or_else(|_| Err(cut())),
    };
    // The handle is freed server-side either way; do not wait long for a stalled server.
    let _ = tokio::time::timeout(Duration::from_secs(5), src.close_dir(handle)).await;
    result.map(|()| out)
}

/// The kind of an entry from its READDIR attributes (`None`: no type sent).
fn entry_kind(attrs: &FileAttributes) -> Option<FileKind> {
    attrs.permissions.map(|_| kind_of(attrs))
}

/// Walk `root` (a directory) into a manifest. `cancel` stops the walk at once; the whole scan
/// is bounded by [`SCAN_TIMEOUT`] (`timeout{treeScan}`).
pub async fn scan<S: RemoteDirSource>(
    src: &S,
    root: &str,
    root_name: &str,
    cancel: watch::Receiver<bool>,
    events: &mut (dyn ScanEvents + '_),
) -> Result<Manifest, AppError> {
    scan_below(src, root, root_name, MAX_DEPTH, cancel, events).await
}

/// [`scan`] of a folder that is `MAX_DEPTH - max_depth` levels below a job's root (a retry
/// lists a folder that could not be listed before): directories deeper than `max_depth` below
/// `root` fail with `treeTooLarge{depth}`.
pub async fn scan_below<S: RemoteDirSource>(
    src: &S,
    root: &str,
    root_name: &str,
    max_depth: usize,
    cancel: watch::Receiver<bool>,
    events: &mut (dyn ScanEvents + '_),
) -> Result<Manifest, AppError> {
    let deadline = tokio::time::Instant::now() + SCAN_TIMEOUT;
    let limits = Limits {
        max_depth: max_depth.min(MAX_DEPTH),
        deadline,
    };
    scan_inner(src, root, root_name, cancel, limits, events).await
}

/// The bounds of one scan.
#[derive(Clone, Copy)]
struct Limits {
    max_depth: usize,
    deadline: tokio::time::Instant,
}

/// How long an aborted scan waits for its directory reads to close their handles.
const ABORT_CLOSE_WAIT: Duration = Duration::from_secs(6);

/// `fut`, or `cancelled` on cancel, or `timeout{treeScan}` at the scan deadline.
async fn bounded<T>(
    cancel: &mut watch::Receiver<bool>,
    deadline: tokio::time::Instant,
    fut: impl Future<Output = T>,
) -> Result<T, AppError> {
    tokio::select! {
        biased;
        _ = cancel.wait_for(|c| *c) => Err(AppError::new(ErrorCode::Cancelled)),
        () = tokio::time::sleep_until(deadline) => Err(AppError::new(ErrorCode::Timeout {
            op: TimeoutOp::TreeScan,
        })),
        r = fut => Ok(r),
    }
}

async fn scan_inner<S: RemoteDirSource>(
    src: &S,
    root: &str,
    root_name: &str,
    mut cancel: watch::Receiver<bool>,
    limits: Limits,
    events: &mut (dyn ScanEvents + '_),
) -> Result<Manifest, AppError> {
    let deadline = limits.deadline;
    let mut m = Manifest {
        root: root.to_owned(),
        nodes: vec![Node {
            parent: 0,
            name: root_name.into(),
            local: None,
            kind: NodeKind::Dir,
            size: 0,
            mtime: None,
            atime: None,
        }],
        stats: ScanStats::default(),
        unlisted: Vec::new(),
    };
    let budget = Budget::default();
    // Stops the directory reads in flight when the scan ends early: they close their remote
    // handles instead of being dropped mid-request.
    let (stop_tx, stop_rx) = watch::channel(false);
    let mut queue: VecDeque<(u32, usize)> = VecDeque::from([(0, 0)]);
    let mut inflight = FuturesUnordered::new();
    let walk = async {
        loop {
            while inflight.len() < SCAN_CONCURRENCY {
                let Some((idx, depth)) = queue.pop_front() else {
                    break;
                };
                let dir = m.remote_path(idx);
                if let Err(e) = path::validate(&dir, "path") {
                    m.stats.failed += 1;
                    m.unlisted.push(idx);
                    events.failed(idx, &m.rel_path(idx), &e);
                    continue;
                }
                let budget = &budget;
                let stop = stop_rx.clone();
                inflight.push(async move {
                    let r = read_dir(src, &dir, budget, stop).await;
                    (idx, depth, dir, r)
                });
            }
            if *cancel.borrow() {
                return Err(AppError::new(ErrorCode::Cancelled));
            }
            let next = bounded(&mut cancel, deadline, inflight.next()).await?;
            let Some((idx, depth, dir, result)) = next else {
                return Ok(());
            };
            match result {
                Err(e) if fatal(&e) => return Err(e),
                Err(e) => {
                    m.stats.failed += 1;
                    m.unlisted.push(idx);
                    events.failed(idx, &m.rel_path(idx), &e);
                }
                Ok(entries) => {
                    let place = Place {
                        parent: idx,
                        depth,
                        dir: &dir,
                    };
                    add_entries(
                        src,
                        &mut m,
                        place,
                        entries,
                        &mut queue,
                        &mut cancel,
                        limits,
                        events,
                    )
                    .await?;
                }
            }
            events.progress(&m.stats);
        }
    };
    let result = walk.await;
    if result.is_err() {
        let _ = stop_tx.send(true);
        let drain = async { while inflight.next().await.is_some() {} };
        let _ = tokio::time::timeout(ABORT_CLOSE_WAIT, drain).await;
    }
    result.map(|()| m)
}

/// The directory whose entries are added.
struct Place<'a> {
    parent: u32,
    depth: usize,
    dir: &'a str,
}

/// Add one directory's entries to the manifest (and its subdirectories to the queue).
#[allow(clippy::too_many_arguments)]
async fn add_entries<S: RemoteDirSource>(
    src: &S,
    m: &mut Manifest,
    place: Place<'_>,
    entries: Vec<RawEntry>,
    queue: &mut VecDeque<(u32, usize)>,
    cancel: &mut watch::Receiver<bool>,
    limits: Limits,
    events: &mut (dyn ScanEvents + '_),
) -> Result<(), AppError> {
    let Place { parent, depth, dir } = place;
    let deadline = limits.deadline;
    let parent_rel = m.rel_path(parent);
    let rel = |name: &str| {
        if parent_rel.is_empty() {
            cap_rel(name.to_owned())
        } else {
            cap_rel(format!("{parent_rel}/{name}"))
        }
    };
    let mut seen: HashSet<String> = HashSet::new();
    for e in entries {
        let mut skip = |m: &mut Manifest, reason: SkipReason, name: &str| {
            m.stats.skipped.add(reason);
            events.skipped(&rel(name), reason);
        };
        if !path::valid_entry_name(&e.name) {
            skip(m, SkipReason::InvalidName, &e.name);
            continue;
        }
        if e.name.contains(path::LOSSY) {
            skip(m, SkipReason::LossyName, &e.name);
            continue;
        }
        let mut attrs = e.attrs;
        let mut kind = entry_kind(&attrs);
        if kind.is_none() {
            // No type in READDIR: one `lstat` (never `stat`: links are not followed).
            // Cancel and the scan deadline end the wait (not only the request timeout).
            match bounded(cancel, deadline, src.lstat(&path::join(dir, &e.name))).await? {
                Ok(a) => {
                    kind = entry_kind(&a);
                    attrs = a;
                }
                Err(err) if fatal(&err) => return Err(err),
                Err(_) => {}
            }
        }
        let kind = match kind {
            Some(FileKind::Dir) => NodeKind::Dir,
            Some(FileKind::File) => NodeKind::File,
            Some(FileKind::Symlink) => {
                skip(m, SkipReason::Symlink, &e.name);
                continue;
            }
            Some(FileKind::Other) => {
                skip(m, SkipReason::Special, &e.name);
                continue;
            }
            None => {
                skip(m, SkipReason::UnknownType, &e.name);
                continue;
            }
        };
        let local = sanitize_component(&e.name);
        if !seen.insert(collision_key(&local)) {
            skip(m, SkipReason::NameCollision, &e.name);
            continue;
        }
        if kind == NodeKind::Dir && depth + 1 > limits.max_depth {
            return Err(too_large(TreeLimit::Depth, MAX_DEPTH));
        }
        let idx = m.len();
        let renamed = local != e.name;
        if renamed {
            m.stats.renamed += 1;
        }
        let size = if kind == NodeKind::File {
            attrs.size.unwrap_or(0)
        } else {
            0
        };
        m.nodes.push(Node {
            parent,
            local: renamed.then(|| local.into_boxed_str()),
            name: e.name.into_boxed_str(),
            kind,
            size,
            mtime: attrs.mtime,
            atime: attrs.atime,
        });
        match kind {
            NodeKind::Dir => {
                m.stats.dirs += 1;
                queue.push_back((idx, depth + 1));
            }
            NodeKind::File => {
                m.stats.files += 1;
                m.stats.bytes = m.stats.bytes.saturating_add(size);
            }
        }
    }
    Ok(())
}

/// The folder a "Download folder…" starts from: a directory, or a symlink to one (the user
/// chose that row; resolved once with `realpath`, nothing below it is followed). Anything else
/// is `notADirectory`.
pub async fn resolve_root(sftp: &Sftp, remote: &str) -> Result<String, AppError> {
    let not_dir = || {
        AppError::new(ErrorCode::NotADirectory {
            path: remote.to_owned(),
        })
    };
    let attrs = sftp.lstat(remote).await?;
    match kind_of(&attrs) {
        FileKind::Dir => Ok(remote.to_owned()),
        FileKind::Symlink => {
            let target = sftp.realpath(remote).await?;
            path::validate_exact(&target, "remotePath")?;
            if kind_of(&sftp.lstat(&target).await?) == FileKind::Dir {
                Ok(target)
            } else {
                Err(not_dir())
            }
        }
        _ => Err(not_dir()),
    }
}

/// The production source: the session's SFTP channel (`opendir`/`readdir`/`close`, `lstat`).
impl RemoteDirSource for Sftp {
    type Handle = String;

    async fn open_dir(&self, path: &str) -> Result<String, AppError> {
        if self.conn.is_gone() {
            return Err(self.not_connected());
        }
        match self.raw.opendir(path).await {
            Ok(h) => Ok(h.handle),
            Err(e) => Err(self.opendir_error(&e, path).await),
        }
    }

    async fn read_batch(
        &self,
        handle: &String,
        path: &str,
    ) -> Result<Option<Vec<RawEntry>>, AppError> {
        match self.raw.readdir(handle.as_str()).await {
            Ok(name) => Ok(Some(
                name.files
                    .into_iter()
                    .map(|f| RawEntry {
                        name: f.filename,
                        attrs: FileAttributes {
                            size: f.attrs.size,
                            permissions: f.attrs.permissions,
                            atime: f.attrs.atime,
                            mtime: f.attrs.mtime,
                            ..FileAttributes::empty()
                        },
                    })
                    .collect(),
            )),
            Err(e) if is_status(&e, StatusCode::Eof) => Ok(None),
            Err(e) => Err(self.map_err(&e, Some(path))),
        }
    }

    async fn close_dir(&self, handle: String) {
        let _ = self.raw.close(handle).await;
    }

    async fn lstat(&self, path: &str) -> Result<FileAttributes, AppError> {
        Sftp::lstat(self, path).await
    }
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    pub(crate) const DIR: u32 = 0o040_755;
    pub(crate) const FILE: u32 = 0o100_644;
    pub(crate) const LINK: u32 = 0o120_777;
    pub(crate) const FIFO: u32 = 0o010_644;
    pub(crate) const CHR: u32 = 0o020_666;

    pub(crate) fn attrs(perm: Option<u32>, size: u64) -> FileAttributes {
        FileAttributes {
            permissions: perm,
            size: Some(size),
            mtime: Some(1_700_000_000),
            atime: Some(1_700_000_000),
            ..FileAttributes::empty()
        }
    }

    /// An in-memory tree: directory path → entries; `lstat` answers from `stats`.
    #[derive(Default)]
    pub(crate) struct MemTree {
        pub dirs: HashMap<String, Vec<(String, FileAttributes)>>,
        pub stats: HashMap<String, FileAttributes>,
        pub denied: HashSet<String>,
        /// Entries per READDIR reply.
        pub batch: usize,
        pub opened: Mutex<Vec<String>>,
    }

    impl MemTree {
        pub(crate) fn dir(&mut self, path: &str, entries: &[(&str, Option<u32>, u64)]) {
            self.dirs.insert(
                path.to_owned(),
                entries
                    .iter()
                    .map(|(n, p, s)| ((*n).to_owned(), attrs(*p, *s)))
                    .collect(),
            );
        }
    }

    pub(crate) struct MemHandle {
        entries: Mutex<VecDeque<RawEntry>>,
    }

    impl RemoteDirSource for MemTree {
        type Handle = MemHandle;
        async fn open_dir(&self, path: &str) -> Result<MemHandle, AppError> {
            self.opened.lock().unwrap().push(path.to_owned());
            if self.denied.contains(path) {
                return Err(AppError::new(ErrorCode::PermissionDenied {
                    path: path.to_owned(),
                }));
            }
            let Some(list) = self.dirs.get(path) else {
                return Err(AppError::new(ErrorCode::RemoteNotFound {
                    path: path.to_owned(),
                }));
            };
            let mut entries: VecDeque<RawEntry> = VecDeque::new();
            entries.push_back(RawEntry {
                name: ".".into(),
                attrs: attrs(Some(DIR), 0),
            });
            entries.push_back(RawEntry {
                name: "..".into(),
                attrs: attrs(Some(DIR), 0),
            });
            entries.extend(list.iter().map(|(n, a)| RawEntry {
                name: n.clone(),
                attrs: a.clone(),
            }));
            Ok(MemHandle {
                entries: Mutex::new(entries),
            })
        }
        async fn read_batch(
            &self,
            h: &MemHandle,
            _path: &str,
        ) -> Result<Option<Vec<RawEntry>>, AppError> {
            let mut q = h.entries.lock().unwrap();
            if q.is_empty() {
                return Ok(None);
            }
            let n = self.batch.max(1).min(q.len());
            Ok(Some(q.drain(..n).collect()))
        }
        async fn close_dir(&self, _h: MemHandle) {}
        async fn lstat(&self, path: &str) -> Result<FileAttributes, AppError> {
            self.stats.get(path).cloned().ok_or_else(|| {
                AppError::new(ErrorCode::RemoteNotFound {
                    path: path.to_owned(),
                })
            })
        }
    }

    #[derive(Default)]
    pub(crate) struct Rec {
        pub skipped: Vec<(String, SkipReason)>,
        pub failed: Vec<(u32, String)>,
        pub progress: usize,
    }

    impl ScanEvents for Rec {
        fn progress(&mut self, _: &ScanStats) {
            self.progress += 1;
        }
        fn skipped(&mut self, rel: &str, reason: SkipReason) {
            self.skipped.push((rel.to_owned(), reason));
        }
        fn failed(&mut self, item: u32, rel: &str, _: &AppError) {
            self.failed.push((item, rel.to_owned()));
        }
    }

    fn no_cancel() -> watch::Receiver<bool> {
        let (tx, rx) = watch::channel(false);
        std::mem::forget(tx);
        rx
    }

    pub(crate) async fn run(
        src: &impl RemoteDirSource,
        root: &str,
    ) -> (Result<Manifest, AppError>, Rec) {
        let mut rec = Rec::default();
        let r = scan(src, root, "root", no_cancel(), &mut rec).await;
        (r, rec)
    }

    fn reason_of(rec: &Rec, rel: &str) -> Option<SkipReason> {
        rec.skipped.iter().find(|(r, _)| r == rel).map(|(_, s)| *s)
    }

    /// Hostile server names never become a path component outside the root: `.`/`..` are
    /// ignored, empty and `/`/NUL names are `invalidName`, lossy names (files and whole
    /// directories) are `lossyName`, links and special files are reported, never listed.
    #[tokio::test]
    async fn hostile_names_links_and_specials_are_skipped() {
        let mut t = MemTree {
            batch: 2,
            ..Default::default()
        };
        t.dir(
            "/r",
            &[
                ("ok.txt", Some(FILE), 5),
                ("", Some(FILE), 1),
                ("a/b", Some(FILE), 1),
                ("nul\0x", Some(FILE), 1),
                ("lossy\u{FFFD}.txt", Some(FILE), 1),
                ("lossydir\u{FFFD}", Some(DIR), 0),
                ("to-root", Some(LINK), 1),
                ("to-parent", Some(LINK), 1),
                ("pipe", Some(FIFO), 0),
                ("zero", Some(CHR), 0),
                ("sub", Some(DIR), 0),
                ("..\\..\\evil", Some(FILE), 3),
            ],
        );
        t.dir("/r/sub", &[("deep.bin", Some(FILE), 7)]);
        // Never listed: the lossy directory's subtree.
        t.dir("/r/lossydir\u{FFFD}", &[("x", Some(FILE), 1)]);
        let (m, rec) = run(&t, "/r").await;
        let m = m.unwrap();
        assert_eq!(m.stats.files, 3);
        assert_eq!(m.stats.dirs, 1);
        assert_eq!(m.stats.bytes, 15);
        let s = m.stats.skipped;
        assert_eq!(
            (s.invalid_name, s.lossy_name, s.symlink, s.special),
            (3, 2, 2, 2)
        );
        assert_eq!(reason_of(&rec, "to-root"), Some(SkipReason::Symlink));
        assert_eq!(reason_of(&rec, "pipe"), Some(SkipReason::Special));
        assert_eq!(
            reason_of(&rec, "lossydir\u{FFFD}"),
            Some(SkipReason::LossyName)
        );
        assert!(
            !t.opened
                .lock()
                .unwrap()
                .iter()
                .any(|p| p.contains('\u{FFFD}')),
            "a lossy directory is never opened"
        );
        // Every local component is a single safe name.
        for i in 1..m.len() {
            for c in m.local_components(i) {
                assert!(path::valid_entry_name(&c), "{c:?}");
                assert!(!c.contains(['\\', '/', ':', '\0']), "{c:?}");
                assert_ne!(c, "..");
            }
        }
        let evil = (1..m.len())
            .find(|&i| &*m.nodes[i as usize].name == "..\\..\\evil")
            .unwrap();
        assert_eq!(m.local_components(evil), vec![".._.._evil".to_owned()]);
        assert_eq!(m.stats.renamed, 1);
        let deep = (1..m.len())
            .find(|&i| &*m.nodes[i as usize].name == "deep.bin")
            .unwrap();
        assert_eq!(m.remote_path(deep), "/r/sub/deep.bin");
        assert_eq!(m.rel_path(deep), "sub/deep.bin");
    }

    /// Reserved device names, streams, separators, trailing dots/spaces, controls: one safe
    /// local name each.
    #[test]
    fn component_names_are_sanitized() {
        for (server, local) in [
            ("CON", "CON_"),
            ("nul.tar.gz", "nul_.tar.gz"),
            ("COM\u{B9}", "COM\u{B9}_"),
            ("file:Zone.Identifier", "file_Zone.Identifier"),
            ("a\\b", "a_b"),
            ("trail. . ", "trail"),
            ("q<>\"|?*x", "q______x"),
            ("bell\u{7}", "bell_"),
            ("rtl\u{202E}gpj.exe", "rtlgpj.exe"),
            ("zero\u{200B}width", "zerowidth"),
            ("...", "download"),
            ("new\nline", "new_line"),
        ] {
            assert_eq!(sanitize_component(server), local, "{server:?}");
        }
        assert!(sanitize_component(&"x".repeat(300)).len() <= 200);
    }

    /// Collisions after sanitizing (`a:b` / `a_b`) and, on Windows/macOS, by case: the first
    /// entry wins, the later one is skipped.
    #[tokio::test]
    async fn collisions_skip_the_later_entry() {
        let mut t = MemTree::default();
        t.dir(
            "/r",
            &[
                ("a:b", Some(FILE), 1),
                ("a_b", Some(FILE), 2),
                ("README", Some(FILE), 3),
                ("readme", Some(FILE), 4),
            ],
        );
        let (m, rec) = run(&t, "/r").await;
        let m = m.unwrap();
        assert_eq!(reason_of(&rec, "a_b"), Some(SkipReason::NameCollision));
        let case_insensitive = cfg!(any(windows, target_os = "macos"));
        assert_eq!(
            reason_of(&rec, "readme").is_some(),
            case_insensitive,
            "case collisions only where the file system folds case"
        );
        assert_eq!(m.stats.files, if case_insensitive { 2 } else { 3 });
        assert_eq!(&*m.nodes[1].name, "a:b");
        assert_eq!(m.nodes[1].local_name(), "a_b");
    }

    /// Kinds come from the READDIR attributes; an entry without a type gets one `lstat`
    /// (a link stays a link), and is `unknownType` when that does not help either.
    #[tokio::test]
    async fn missing_types_are_resolved_with_lstat_only() {
        let mut t = MemTree::default();
        t.dir(
            "/r",
            &[
                ("typeless-file", None, 1),
                ("typeless-link", None, 1),
                ("hopeless", None, 1),
            ],
        );
        t.stats
            .insert("/r/typeless-file".into(), attrs(Some(FILE), 9));
        t.stats
            .insert("/r/typeless-link".into(), attrs(Some(LINK), 1));
        t.stats.insert("/r/hopeless".into(), attrs(None, 1));
        let (m, rec) = run(&t, "/r").await;
        let m = m.unwrap();
        assert_eq!(m.stats.files, 1);
        assert_eq!(m.stats.bytes, 9);
        assert_eq!(reason_of(&rec, "typeless-link"), Some(SkipReason::Symlink));
        assert_eq!(reason_of(&rec, "hopeless"), Some(SkipReason::UnknownType));
    }

    /// A directory that cannot be opened is an item failure; the rest is scanned.
    #[tokio::test]
    async fn unreadable_directory_is_an_item_failure() {
        let mut t = MemTree::default();
        t.dir("/r", &[("locked", Some(DIR), 0), ("open", Some(DIR), 0)]);
        t.dir("/r/locked", &[("secret", Some(FILE), 1)]);
        t.dir("/r/open", &[("f", Some(FILE), 1)]);
        t.denied.insert("/r/locked".into());
        let (m, rec) = run(&t, "/r").await;
        let m = m.unwrap();
        assert_eq!(m.stats.files, 1);
        assert_eq!(m.stats.failed, 1);
        assert_eq!(rec.failed.len(), 1);
        assert_eq!(rec.failed[0].1, "locked");
        assert_eq!(m.unlisted.len(), 1);
        assert!(rec.progress > 0);
    }

    /// A generator source: every directory holds one subdirectory (`deep`), or `width`
    /// files with `name_len`-byte names (`wide`), without materialising the tree.
    struct Gen {
        depth: usize,
        width: usize,
        name_len: usize,
        reads: AtomicUsize,
    }

    struct GenHandle {
        level: usize,
        next: Mutex<usize>,
        done: Mutex<bool>,
    }

    impl RemoteDirSource for Gen {
        type Handle = GenHandle;
        async fn open_dir(&self, path: &str) -> Result<GenHandle, AppError> {
            Ok(GenHandle {
                level: path.matches('/').count(),
                next: Mutex::new(0),
                done: Mutex::new(false),
            })
        }
        async fn read_batch(
            &self,
            h: &GenHandle,
            _: &str,
        ) -> Result<Option<Vec<RawEntry>>, AppError> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            if self.width == 0 {
                let mut done = h.done.lock().unwrap();
                if *done || h.level > self.depth {
                    return Ok(None);
                }
                *done = true;
                return Ok(Some(vec![RawEntry {
                    name: "d".into(),
                    attrs: attrs(Some(DIR), 0),
                }]));
            }
            let mut next = h.next.lock().unwrap();
            if *next >= self.width {
                return Ok(None);
            }
            let batch: Vec<RawEntry> = (*next..(*next + 100).min(self.width))
                .map(|i| RawEntry {
                    name: format!("{i:0>width$}", width = self.name_len),
                    attrs: attrs(Some(FILE), 1),
                })
                .collect();
            *next += batch.len();
            Ok(Some(batch))
        }
        async fn close_dir(&self, _: GenHandle) {}
        async fn lstat(&self, _: &str) -> Result<FileAttributes, AppError> {
            Err(AppError::new(ErrorCode::Internal))
        }
    }

    fn gen_tree(depth: usize, width: usize, name_len: usize) -> Gen {
        Gen {
            depth,
            width,
            name_len,
            reads: AtomicUsize::new(0),
        }
    }

    /// 10 000 levels: refused at level 64, before anything is written, without recursion.
    #[tokio::test]
    async fn depth_limit_refuses_the_tree() {
        let g = gen_tree(10_000, 0, 1);
        let (m, _) = run(&g, "/r").await;
        let e = m.unwrap_err();
        assert_eq!(
            e.kind,
            ErrorCode::TreeTooLarge {
                limit: TreeLimit::Depth,
                value: 64
            }
        );
        assert!(g.reads.load(Ordering::Relaxed) < 200);
        // 64 levels are fine.
        let (m, _) = run(&gen_tree(MAX_DEPTH, 0, 1), "/r").await;
        assert_eq!(m.unwrap().stats.dirs, 64);
    }

    /// One directory beyond the §6.2 entry cap fails the scan (`directory`); a tree beyond
    /// 200 000 entries fails with `entries`; 4 KiB names hit the memory budget first.
    #[tokio::test]
    async fn entry_and_memory_limits_refuse_the_tree() {
        let (m, _) = run(&gen_tree(0, LIST_MAX_ENTRIES + 1, 8), "/r").await;
        assert_eq!(
            m.unwrap_err().kind,
            ErrorCode::TreeTooLarge {
                limit: TreeLimit::Directory,
                value: LIST_MAX_ENTRIES as u64
            }
        );
        // 3 directories of 99 999 files: the second fills the job budget.
        let mut t = MemTree {
            batch: 1000,
            ..Default::default()
        };
        t.dir(
            "/r",
            &[
                ("a", Some(DIR), 0),
                ("b", Some(DIR), 0),
                ("c", Some(DIR), 0),
            ],
        );
        for d in ["a", "b", "c"] {
            let names: Vec<String> = (0..99_999).map(|i| format!("{i}")).collect();
            t.dirs.insert(
                format!("/r/{d}"),
                names
                    .into_iter()
                    .map(|n| (n, attrs(Some(FILE), 1)))
                    .collect(),
            );
        }
        let (m, _) = run(&t, "/r").await;
        assert_eq!(
            m.unwrap_err().kind,
            ErrorCode::TreeTooLarge {
                limit: TreeLimit::Entries,
                value: MAX_ENTRIES as u64
            }
        );
        // 4 KiB names: 64 MiB / (2 × 4 KiB + overhead) ≈ 7 800 entries.
        let (m, _) = run(&gen_tree(0, 50_000, 4096), "/r").await;
        assert_eq!(
            m.unwrap_err().kind,
            ErrorCode::TreeTooLarge {
                limit: TreeLimit::Memory,
                value: MAX_MANIFEST_BYTES as u64
            }
        );
    }

    /// A server that answers every READDIR with an empty reply forever, or never answers:
    /// the per-directory deadline fails the scan (`directory`), the whole scan has 10 min.
    #[tokio::test(start_paused = true)]
    async fn endless_or_silent_directories_fail_the_scan() {
        struct Endless {
            silent: bool,
        }
        impl RemoteDirSource for Endless {
            type Handle = ();
            async fn open_dir(&self, _: &str) -> Result<(), AppError> {
                Ok(())
            }
            async fn read_batch(&self, _: &(), _: &str) -> Result<Option<Vec<RawEntry>>, AppError> {
                if self.silent {
                    std::future::pending::<()>().await;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
                Ok(Some(Vec::new()))
            }
            async fn close_dir(&self, _: ()) {}
            async fn lstat(&self, _: &str) -> Result<FileAttributes, AppError> {
                Err(AppError::new(ErrorCode::Internal))
            }
        }
        for silent in [false, true] {
            let start = tokio::time::Instant::now();
            let (m, _) = run(&Endless { silent }, "/r").await;
            assert_eq!(
                m.unwrap_err().kind,
                ErrorCode::TreeTooLarge {
                    limit: TreeLimit::Directory,
                    value: LIST_MAX_ENTRIES as u64
                }
            );
            assert!(start.elapsed() <= LIST_TIMEOUT + Duration::from_secs(6));
        }
    }

    /// Many slow directories: the whole scan ends at 10 minutes (`timeout{treeScan}`).
    #[tokio::test(start_paused = true)]
    async fn whole_scan_is_bounded() {
        struct Slow;
        struct H(Mutex<bool>);
        impl RemoteDirSource for Slow {
            type Handle = H;
            async fn open_dir(&self, _: &str) -> Result<H, AppError> {
                Ok(H(Mutex::new(false)))
            }
            async fn read_batch(&self, h: &H, _: &str) -> Result<Option<Vec<RawEntry>>, AppError> {
                tokio::time::sleep(Duration::from_secs(20)).await;
                let mut done = h.0.lock().unwrap();
                if *done {
                    return Ok(None);
                }
                *done = true;
                Ok(Some(
                    (0..4)
                        .map(|i| RawEntry {
                            name: format!("d{i}"),
                            attrs: attrs(Some(DIR), 0),
                        })
                        .collect(),
                ))
            }
            async fn close_dir(&self, _: H) {}
            async fn lstat(&self, _: &str) -> Result<FileAttributes, AppError> {
                Err(AppError::new(ErrorCode::Internal))
            }
        }
        let (m, _) = run(&Slow, "/r").await;
        let e = m.unwrap_err();
        // Either the depth cap or the scan deadline, never an endless scan; with 4 directories
        // of 4 per level the 10 minutes come first.
        assert_eq!(
            e.kind,
            ErrorCode::Timeout {
                op: TimeoutOp::TreeScan
            }
        );
    }

    /// A scan cancelled (or failing) while directory reads are stuck closes every remote
    /// handle it opened; a typeless entry's `lstat` that never answers ends on cancel too.
    #[tokio::test(start_paused = true)]
    async fn aborted_scans_close_their_directory_handles() {
        #[derive(Default)]
        struct Stuck {
            opens: AtomicUsize,
            closes: AtomicUsize,
        }
        impl RemoteDirSource for Stuck {
            type Handle = bool;
            async fn open_dir(&self, path: &str) -> Result<bool, AppError> {
                self.opens.fetch_add(1, Ordering::SeqCst);
                Ok(path == "/r")
            }
            async fn read_batch(
                &self,
                root: &bool,
                _: &str,
            ) -> Result<Option<Vec<RawEntry>>, AppError> {
                if *root {
                    static ONCE: AtomicUsize = AtomicUsize::new(0);
                    if ONCE.fetch_add(1, Ordering::SeqCst) == 0 {
                        return Ok(Some(
                            (0..6)
                                .map(|i| RawEntry {
                                    name: format!("d{i}"),
                                    attrs: attrs(Some(DIR), 0),
                                })
                                .collect(),
                        ));
                    }
                    return Ok(None);
                }
                std::future::pending::<()>().await;
                Ok(None)
            }
            async fn close_dir(&self, _: bool) {
                self.closes.fetch_add(1, Ordering::SeqCst);
            }
            async fn lstat(&self, _: &str) -> Result<FileAttributes, AppError> {
                std::future::pending::<()>().await;
                Err(AppError::new(ErrorCode::Internal))
            }
        }
        let src = Stuck::default();
        let (tx, rx) = watch::channel(false);
        let mut rec = Rec::default();
        let scan_fut = scan(&src, "/r", "r", rx, &mut rec);
        let cancel = async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            tx.send(true).unwrap();
        };
        let (r, ()) = tokio::join!(scan_fut, cancel);
        assert_eq!(r.unwrap_err().kind, ErrorCode::Cancelled);
        let opens = src.opens.load(Ordering::SeqCst);
        assert_eq!(opens, 1 + SCAN_CONCURRENCY, "root plus the reads in flight");
        assert_eq!(
            src.closes.load(Ordering::SeqCst),
            opens,
            "every handle closed"
        );

        // A typeless entry whose `lstat` never answers: cancel ends it.
        let mut t = MemTree::default();
        t.dir("/r", &[("typeless", None, 1)]);
        struct NeverStat(MemTree);
        impl RemoteDirSource for NeverStat {
            type Handle = MemHandle;
            async fn open_dir(&self, p: &str) -> Result<MemHandle, AppError> {
                self.0.open_dir(p).await
            }
            async fn read_batch(
                &self,
                h: &MemHandle,
                p: &str,
            ) -> Result<Option<Vec<RawEntry>>, AppError> {
                self.0.read_batch(h, p).await
            }
            async fn close_dir(&self, h: MemHandle) {
                self.0.close_dir(h).await;
            }
            async fn lstat(&self, _: &str) -> Result<FileAttributes, AppError> {
                std::future::pending::<()>().await;
                Err(AppError::new(ErrorCode::Internal))
            }
        }
        let src = NeverStat(t);
        let (tx, rx) = watch::channel(false);
        let mut rec = Rec::default();
        let scan_fut = scan(&src, "/r", "r", rx, &mut rec);
        let cancel = async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            tx.send(true).unwrap();
        };
        let (r, ()) = tokio::join!(scan_fut, cancel);
        assert_eq!(r.unwrap_err().kind, ErrorCode::Cancelled);
    }

    /// Cancel stops the scan at once.
    #[tokio::test(start_paused = true)]
    async fn cancel_stops_the_scan() {
        let g = gen_tree(0, 1_000_000, 8);
        let (tx, rx) = watch::channel(false);
        let mut rec = Rec::default();
        tx.send(true).unwrap();
        let e = scan(&g, "/r", "r", rx, &mut rec).await.unwrap_err();
        assert_eq!(e.kind, ErrorCode::Cancelled);
    }

    /// Remote paths over 4096 bytes: the directory is an item failure, the scan goes on.
    #[tokio::test]
    async fn too_long_remote_paths_are_item_failures() {
        let mut t = MemTree::default();
        let long = "x".repeat(250);
        let mut dir = "/r".to_owned();
        let mut entries_of = Vec::new();
        for _ in 0..20 {
            entries_of.push(dir.clone());
            dir = path::join(&dir, &long);
        }
        for (i, d) in entries_of.iter().enumerate() {
            let mut list = vec![(long.clone(), attrs(Some(DIR), 0))];
            if i == 0 {
                list.push(("small".into(), attrs(Some(FILE), 1)));
            }
            t.dirs.insert(d.clone(), list);
        }
        t.dirs.insert(dir, Vec::new());
        let (m, rec) = run(&t, "/r").await;
        let m = m.unwrap();
        assert_eq!(m.stats.files, 1);
        assert_eq!(m.stats.failed, 1);
        assert_eq!(rec.failed.len(), 1);
        assert!(rec.failed[0].1.len() <= MAX_REL_PATH);
    }
}
