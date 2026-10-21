//! SFTP client (§6.1–§6.3) over the session's `RawSftpSession`: error mapping (§4.3), bounded
//! listing, stat, file operations, pipelined reads/writes and text reads.
//!
//! Every operation works on a snapshot of the session's connection. If that connection is lost
//! (or given up by the supervisor) while a request runs, the error becomes `notConnected` (§5.8:
//! in-flight requests of a lost connection fail with `notConnected`).

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures_util::stream::{FuturesOrdered, FuturesUnordered};
use futures_util::{FutureExt, StreamExt};
use russh_sftp::client::RawSftpSession;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::{Data, FileAttributes, OpenFlags, Status, StatusCode};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::error::{AppError, ErrorCode, TimeoutOp};
use crate::ipc::dto::{DirListing, Etag, FileEntry, FileKind, TextDocument};
use crate::session::session::SessionHandle;
use crate::ssh::connect::Connection;

use super::path;
use super::text;

/// Listing caps (§6.2).
pub const LIST_MAX_ENTRIES: usize = 100_000;
/// One memory budget per listing: what the client holds and sends — the Rust strings
/// (`name` + full `path`), their JSON form for the WebView (escapes included) and a fixed
/// per-entry overhead. Replaces §6.2's "64 MiB of names", which ignored `path` and escaping.
pub const LIST_MAX_BYTES: usize = 48 * 1024 * 1024;
/// Per-entry overhead charged against [`LIST_MAX_BYTES`]: the `FileEntry` struct (+ `Vec` slack)
/// and the JSON keys/numbers of one entry.
pub const LIST_ENTRY_OVERHEAD: usize = 256;
pub const LIST_TIMEOUT: Duration = Duration::from_secs(60);
pub const LIST_MAX_SYMLINK_STATS: usize = 1_000;
pub const LIST_STAT_CONCURRENCY: usize = 8;
/// Pipelining (§6.3): chunk size and requests in flight (editor reads and saves; also the floor
/// of a transfer's window).
pub const CHUNK: u32 = 32 * 1024;
pub const PIPELINE_DEPTH: usize = 8;
/// Most requests one transfer keeps in flight (OpenSSH `sftp` uses 64 × 32 KiB), so one
/// transfer is no longer capped at 256 KiB per round trip (≈ 2.5 MiB/s at 100 ms).
pub const TRANSFER_DEPTH: usize = 64;
/// Requests above [`PIPELINE_DEPTH`] that the transfers of one session may have in flight
/// together (permits of the session's pool): 3 × 56 × 32 KiB = 5.25 MiB, enough for the default
/// 3 concurrent transfers at full depth.
pub const SESSION_EXTRA_REQUESTS: usize = 3 * (TRANSFER_DEPTH - PIPELINE_DEPTH);
/// A completed request that took longer than this shrinks the window (halves it, never
/// below [`PIPELINE_DEPTH`]). Requests queue behind each other on a slow link; this keeps the
/// newest request of a full window far from the 30 s SFTP request timeout.
pub const SLOW_REQUEST: Duration = Duration::from_secs(5);

const S_IFMT: u32 = 0o170_000;
const S_IFDIR: u32 = 0o040_000;
const S_IFREG: u32 = 0o100_000;
const S_IFLNK: u32 = 0o120_000;

/// `FileKind` from the SFTP `permissions` field (`None` ⇒ `other`).
pub fn kind_of(attrs: &FileAttributes) -> FileKind {
    match attrs.permissions.map(|p| p & S_IFMT) {
        Some(S_IFDIR) => FileKind::Dir,
        Some(S_IFREG) => FileKind::File,
        Some(S_IFLNK) => FileKind::Symlink,
        _ => FileKind::Other,
    }
}

/// What may be read (editor, download, save conflict read): a regular file. A directory
/// ⇒ `isADirectory`; a FIFO, socket or device ⇒ `notARegularFile` (OpenSSH's single-threaded
/// sftp-server would block on a FIFO; `/dev/zero` never ends). Attributes without a file type
/// (a server that sends no `permissions`) are allowed: nothing is known to be wrong with them.
pub fn ensure_regular(attrs: &FileAttributes, path: &str) -> Result<(), AppError> {
    match attrs.permissions.map(|p| p & S_IFMT) {
        Some(S_IFDIR) => Err(AppError::new(ErrorCode::IsADirectory {
            path: path.to_owned(),
        })),
        None | Some(0 | S_IFREG) => Ok(()),
        Some(_) => Err(AppError::new(ErrorCode::NotARegularFile {
            path: path.to_owned(),
        })),
    }
}

/// Most READDIR replies [`Sftp::delete`] reads to find out whether a directory is empty
/// (a hostile server can answer every READDIR with an empty NAME packet forever).
pub const DIR_PROBE_MAX_PACKETS: usize = 1_000;

/// Whether a directory has entries other than `.`/`..`, from successive READDIR replies
/// (`next` yields one reply's names, `None` at the end or on error). Bounded by
/// [`DIR_PROBE_MAX_PACKETS`] replies and [`LIST_TIMEOUT`]; when the bound is hit the directory
/// counts as not empty (the delete then reports `directoryNotEmpty`, the safe answer).
pub async fn probe_dir_entries<F, Fut>(mut next: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<Vec<String>>>,
{
    let probe = async {
        for _ in 0..DIR_PROBE_MAX_PACKETS {
            let Some(names) = next().await else {
                return false;
            };
            if names.iter().any(|n| n != "." && n != "..") {
                return true;
            }
        }
        true
    };
    tokio::time::timeout(LIST_TIMEOUT, probe)
        .await
        .unwrap_or(true)
}

/// Permission bits only (`& 0o7777`).
pub fn mode_bits(attrs: &FileAttributes) -> Option<u32> {
    attrs.permissions.map(|p| p & 0o7777)
}

pub fn entry_from(
    path: String,
    name: String,
    attrs: &FileAttributes,
    link_target_kind: Option<FileKind>,
) -> FileEntry {
    FileEntry {
        lossy_name: name.contains(path::LOSSY),
        name,
        path,
        kind: kind_of(attrs),
        link_target_kind,
        size: attrs.size,
        mtime: attrs.mtime.map(i64::from),
        permissions: mode_bits(attrs),
        uid: attrs.uid,
        gid: attrs.gid,
        owner: None,
        group: None,
    }
}

/// Longest owner/group name taken from a long name (Linux user names are ≤ 32 chars).
pub const MAX_OWNER_CHARS: usize = 32;

/// Owner and group names from a `readdir` long name
/// (`-rw-r--r--  1 alice  staff  1234 Jan  1 12:00 name`). The long name is server text in no
/// fixed format (SFTP v3 §7), so it is used only when it is consistent with the attributes:
/// a 10-char mode token, a link count, then owner, group and the size column equal to the
/// reported size. Names are ≤ 32 chars, without control characters; a purely numeric token is
/// "no name" (the UI shows `uid`/`gid` then).
pub fn owner_group_from_longname(
    longname: &str,
    attrs: &FileAttributes,
) -> (Option<String>, Option<String>) {
    let mut tokens = longname.split_ascii_whitespace();
    let (Some(mode), Some(links), Some(owner), Some(group), Some(size)) = (
        tokens.next(),
        tokens.next(),
        tokens.next(),
        tokens.next(),
        tokens.next(),
    ) else {
        return (None, None);
    };
    let mode_ok = mode.len() == 10
        && mode
            .bytes()
            .all(|b| b.is_ascii_alphabetic() || matches!(b, b'-' | b'+' | b'@' | b'.'));
    let links_ok = !links.is_empty() && links.bytes().all(|b| b.is_ascii_digit());
    let size_ok = attrs
        .size
        .is_some_and(|s| size.parse::<u64>().is_ok_and(|v| v == s));
    if !(mode_ok && links_ok && size_ok) {
        return (None, None);
    }
    let name = |t: &str| {
        let ok = (1..=MAX_OWNER_CHARS).contains(&t.chars().count())
            && !t.chars().any(|c| c.is_control() || c.is_whitespace())
            && !t.chars().any(crate::ssh::shellquote::is_deceptive_char)
            && !t.bytes().all(|b| b.is_ascii_digit());
        ok.then(|| t.to_owned())
    };
    (name(owner), name(group))
}

/// Length of `s` as a JSON string body (serde_json escapes `"`, `\` and C0 controls).
pub fn json_str_len(s: &str) -> usize {
    s.chars()
        .map(|c| match c {
            '"' | '\\' | '\u{08}' | '\u{0C}' | '\n' | '\r' | '\t' => 2,
            c if u32::from(c) < 0x20 => 6,
            c => c.len_utf8(),
        })
        .sum()
}

/// Builds a `DirListing` from `readdir` entries under the §6.2 caps: at most `max_entries`
/// entries and `max_bytes` of cost (see [`LIST_MAX_BYTES`]). `truncated` is set only when an
/// entry had to be left out, so `entries.len() == max_entries` ⇔ a full page, or when
/// the listing ran into its deadline ([`collect_listing`]).
pub struct ListingBuilder {
    dir: String,
    dir_json: usize,
    entries: Vec<FileEntry>,
    cost: usize,
    truncated: bool,
    max_entries: usize,
    max_bytes: usize,
}

impl ListingBuilder {
    pub fn new(dir: &str) -> Self {
        Self::with_limits(dir, LIST_MAX_ENTRIES, LIST_MAX_BYTES)
    }

    pub fn with_limits(dir: &str, max_entries: usize, max_bytes: usize) -> Self {
        Self {
            dir: dir.to_owned(),
            dir_json: json_str_len(dir),
            entries: Vec::new(),
            cost: 0,
            truncated: false,
            max_entries,
            max_bytes,
        }
    }

    /// Add one `readdir` entry without a long name (no owner/group names).
    pub fn push(&mut self, name: String, attrs: &FileAttributes) -> bool {
        self.push_long(name, "", attrs)
    }

    /// Add one `readdir` entry (invalid names are skipped); owner/group names come from
    /// `longname` when it is consistent ([`owner_group_from_longname`]). `false` once the
    /// listing is full.
    pub fn push_long(&mut self, name: String, longname: &str, attrs: &FileAttributes) -> bool {
        if self.truncated {
            return false;
        }
        if !path::valid_entry_name(&name) {
            if name != "." && name != ".." {
                tracing::debug!("skipped a directory entry with an invalid name");
            }
            return true;
        }
        let sep = usize::from(!self.dir.ends_with('/'));
        let name_json = json_str_len(&name);
        let full_len = self.dir.len() + sep + name.len();
        let (owner, group) = owner_group_from_longname(longname, attrs);
        // Owner/group: Rust strings + their JSON (no escapes: no controls, quotes are possible).
        let names_cost = [&owner, &group]
            .iter()
            .filter_map(|n| n.as_deref())
            .map(|n| n.len() + json_str_len(n))
            .sum::<usize>();
        let cost = name.len() + full_len // Rust strings
            + name_json + self.dir_json + sep + name_json // JSON for the WebView
            + names_cost
            + LIST_ENTRY_OVERHEAD;
        if self.entries.len() >= self.max_entries || self.cost + cost > self.max_bytes {
            self.truncated = true;
            return false;
        }
        self.cost += cost;
        let full = path::join(&self.dir, &name);
        let mut entry = entry_from(full, name, attrs, None);
        entry.owner = owner;
        entry.group = group;
        self.entries.push(entry);
        true
    }

    pub fn cost(&self) -> usize {
        self.cost
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn finish(self) -> DirListing {
        DirListing {
            path: self.dir,
            entries: self.entries,
            truncated: self.truncated,
        }
    }
}

/// Reads READDIR replies into a listing (§6.2): `next` yields one reply's entries, `None` at
/// EOF. Stops at EOF, at the caps of [`ListingBuilder`], or at `deadline`. At the
/// deadline the entries read so far come back as a `truncated` listing; `timeout{listing}` only
/// when none arrived.
pub async fn collect_listing<F, Fut>(
    dir: &str,
    deadline: tokio::time::Instant,
    mut next: F,
) -> Result<DirListing, AppError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Option<Vec<russh_sftp::protocol::File>>, AppError>>,
{
    let mut builder = ListingBuilder::new(dir);
    let read_all = async {
        while let Some(files) = next().await? {
            for f in files {
                if !builder.push_long(f.filename, &f.longname, &f.attrs) {
                    return Ok(());
                }
            }
        }
        Ok::<(), AppError>(())
    };
    match tokio::time::timeout_at(deadline, read_all).await {
        Ok(Ok(())) => Ok(builder.finish()),
        Ok(Err(e)) => Err(e),
        Err(_) if builder.is_empty() => Err(AppError::new(ErrorCode::Timeout {
            op: TimeoutOp::Listing,
        })),
        Err(_) => {
            tracing::info!("listing timed out; returning the entries read so far");
            builder.truncated = true;
            Ok(builder.finish())
        }
    }
}

/// Stable token for an SFTP status code (`sftpFailure.status`).
pub fn status_token(code: StatusCode) -> &'static str {
    match code {
        StatusCode::Ok => "ok",
        StatusCode::Eof => "eof",
        StatusCode::NoSuchFile => "noSuchFile",
        StatusCode::PermissionDenied => "permissionDenied",
        StatusCode::Failure => "failure",
        StatusCode::BadMessage => "badMessage",
        StatusCode::NoConnection => "noConnection",
        StatusCode::ConnectionLost => "connectionLost",
        StatusCode::OpUnsupported => "opUnsupported",
    }
}

pub fn is_status(e: &SftpError, code: StatusCode) -> bool {
    matches!(e, SftpError::Status(Status { status_code, .. }) if *status_code == code)
}

/// The session's SFTP channel on one connection snapshot.
#[derive(Clone)]
pub struct Sftp {
    pub session_id: String,
    pub conn: Arc<Connection>,
    pub raw: Arc<RawSftpSession>,
    /// `posix-rename@openssh.com` advertised at init (tests may turn it off to exercise the
    /// rename fallback of §6.4).
    pub posix_rename: bool,
    /// `fsync@openssh.com` advertised at init.
    pub fsync: bool,
    /// Test hook, run right before an in-place write opens its target (integration tests swap
    /// the directory entry here to prove the swap is detected). Always `None` in the app.
    pub in_place_hook: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Test hook, run after an atomic write closed its temp file and before the target is
    /// checked again and replaced (integration tests change the target here). Always
    /// `None` in the app.
    pub commit_hook: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Sftp {
    /// `notConnected` without a live connection, `sftpUnavailable` without SFTP.
    pub async fn for_session(session: &SessionHandle) -> Result<Self, AppError> {
        let conn = session.connection.read().await.clone().ok_or_else(|| {
            AppError::new(ErrorCode::NotConnected {
                session_id: session.id.clone(),
            })
        })?;
        let raw = conn
            .sftp
            .clone()
            .ok_or_else(|| AppError::new(ErrorCode::SftpUnavailable))?;
        Ok(Self {
            session_id: session.id.clone(),
            posix_rename: conn.has_sftp_extension("posix-rename@openssh.com"),
            fsync: conn.has_sftp_extension("fsync@openssh.com"),
            in_place_hook: None,
            commit_hook: None,
            conn,
            raw,
        })
    }

    pub fn not_connected(&self) -> AppError {
        AppError::new(ErrorCode::NotConnected {
            session_id: self.session_id.clone(),
        })
    }

    pub fn read_chunk(&self) -> u32 {
        let limit = self.conn.sftp_limits.read_len.unwrap_or(u64::from(CHUNK));
        u32::try_from(limit.min(u64::from(CHUNK)))
            .unwrap_or(CHUNK)
            .max(1)
    }

    pub fn write_chunk(&self) -> usize {
        // A WRITE packet carries ~13 bytes + the handle besides the data; stay well below
        // `max_write_len` and the 256 KiB packet cap.
        let limit = self.conn.sftp_limits.write_len.unwrap_or(u64::from(CHUNK));
        usize::try_from(limit.min(u64::from(CHUNK)))
            .unwrap_or(CHUNK as usize)
            .max(1)
    }

    /// §4.3 mapping of an SFTP error concerning `path`.
    pub fn map_err(&self, e: &SftpError, path: Option<&str>) -> AppError {
        let p = || path.unwrap_or_default().to_owned();
        let kind = match e {
            SftpError::Status(s) => match s.status_code {
                StatusCode::NoSuchFile => ErrorCode::RemoteNotFound { path: p() },
                StatusCode::PermissionDenied => ErrorCode::PermissionDenied { path: p() },
                StatusCode::NoConnection | StatusCode::ConnectionLost => {
                    return self.not_connected().with_detail(e.to_string());
                }
                code => ErrorCode::SftpFailure {
                    path: path.map(str::to_owned),
                    status: status_token(code).to_owned(),
                },
            },
            _ if self.conn.is_gone() => return self.not_connected().with_detail(e.to_string()),
            SftpError::Timeout => ErrorCode::Timeout {
                op: TimeoutOp::SftpRequest,
            },
            SftpError::IO(_) | SftpError::UnexpectedBehavior(_) => ErrorCode::SftpFailure {
                path: path.map(str::to_owned),
                status: "io".to_owned(),
            },
            SftpError::Limited(_) => ErrorCode::SftpFailure {
                path: path.map(str::to_owned),
                status: "limited".to_owned(),
            },
            SftpError::UnexpectedPacket => ErrorCode::SftpFailure {
                path: path.map(str::to_owned),
                status: "badMessage".to_owned(),
            },
        };
        // Server text (status message) goes to `detail` only.
        AppError::new(kind).with_detail(e.to_string())
    }

    pub async fn lstat(&self, p: &str) -> Result<FileAttributes, AppError> {
        self.raw
            .lstat(p)
            .await
            .map(|a| a.attrs)
            .map_err(|e| self.map_err(&e, Some(p)))
    }

    pub async fn stat(&self, p: &str) -> Result<FileAttributes, AppError> {
        self.raw
            .stat(p)
            .await
            .map(|a| a.attrs)
            .map_err(|e| self.map_err(&e, Some(p)))
    }

    /// `None` if the path does not exist (`lstat`: links are not followed).
    pub async fn lstat_opt(&self, p: &str) -> Result<Option<FileAttributes>, AppError> {
        match self.raw.lstat(p).await {
            Ok(a) => Ok(Some(a.attrs)),
            Err(e) if is_status(&e, StatusCode::NoSuchFile) => Ok(None),
            Err(e) => Err(self.map_err(&e, Some(p))),
        }
    }

    pub async fn realpath(&self, p: &str) -> Result<String, AppError> {
        let name = self
            .raw
            .realpath(p)
            .await
            .map_err(|e| self.map_err(&e, Some(p)))?;
        name.files
            .into_iter()
            .next()
            .map(|f| f.filename)
            .ok_or_else(|| {
                AppError::new(ErrorCode::SftpFailure {
                    path: Some(p.to_owned()),
                    status: "badMessage".to_owned(),
                })
            })
    }

    /// `FileEntry` of one path (`lstat`; for a symlink also the target's kind).
    pub async fn entry(&self, p: &str) -> Result<FileEntry, AppError> {
        let attrs = self.lstat(p).await?;
        let link = if kind_of(&attrs) == FileKind::Symlink {
            Some(match self.raw.stat(p).await {
                Ok(t) => kind_of(&t.attrs),
                Err(_) => FileKind::Other, // broken link
            })
        } else {
            None
        };
        Ok(entry_from(
            p.to_owned(),
            path::basename(p).to_owned(),
            &attrs,
            link,
        ))
    }

    /// After a `Failure` (or ambiguous error) on create/mkdir/rename: does `target` exist?
    async fn exists_error(&self, e: &SftpError, target: &str, op_path: &str) -> AppError {
        if is_status(e, StatusCode::Failure) && matches!(self.lstat_opt(target).await, Ok(Some(_)))
        {
            return AppError::new(ErrorCode::RemoteExists {
                path: target.to_owned(),
            })
            .with_detail(e.to_string());
        }
        self.map_err(e, Some(op_path))
    }

    /// `sftp_list` (§6.2): bounded listing, `.`/`..` dropped, symlink targets stat'ed.
    pub async fn list(&self, dir: &str) -> Result<DirListing, AppError> {
        // A connection the session gave up: fail at once instead of queueing a request that
        // may sit behind a full channel window until the request timeout.
        if self.conn.is_gone() {
            return Err(self.not_connected());
        }
        let deadline = tokio::time::Instant::now() + LIST_TIMEOUT;
        let timeout = || {
            AppError::new(ErrorCode::Timeout {
                op: TimeoutOp::Listing,
            })
        };
        let handle = match tokio::time::timeout_at(deadline, self.raw.opendir(dir)).await {
            Err(_) => return Err(timeout()),
            Ok(Ok(h)) => h.handle,
            Ok(Err(e)) => return Err(self.opendir_error(&e, dir).await),
        };
        let outcome = collect_listing(dir, deadline, || async {
            match self.raw.readdir(handle.as_str()).await {
                Ok(name) => Ok(Some(name.files)),
                Err(e) if is_status(&e, StatusCode::Eof) => Ok(None),
                Err(e) => Err(self.map_err(&e, Some(dir))),
            }
        })
        .await;
        // Close without waiting longer than needed (the handle is freed server-side either way).
        let _ = tokio::time::timeout(Duration::from_secs(5), self.raw.close(handle.as_str())).await;
        let mut listing = outcome?;
        // Symlink targets: first 1 000 links, ≤ 8 concurrent stats, within the listing budget.
        let links: Vec<usize> = listing
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.kind == FileKind::Symlink)
            .map(|(i, _)| i)
            .take(LIST_MAX_SYMLINK_STATS)
            .collect();
        let entries = &listing.entries;
        let stats = futures_util::stream::iter(links.into_iter().map(|i| {
            let p = entries.get(i).map(|e| e.path.clone()).unwrap_or_default();
            async move {
                let kind = match self.raw.stat(p).await {
                    Ok(a) => kind_of(&a.attrs),
                    Err(_) => FileKind::Other,
                };
                (i, kind)
            }
        }))
        .buffer_unordered(LIST_STAT_CONCURRENCY)
        .collect::<Vec<_>>();
        if let Ok(results) = tokio::time::timeout_at(deadline, stats).await {
            for (i, kind) in results {
                if let Some(e) = listing.entries.get_mut(i) {
                    e.link_target_kind = Some(kind);
                }
            }
        }
        Ok(listing)
    }

    /// OpenSSH reports `opendir` on a file as `noSuchFile`/`failure`: stat to tell them apart.
    async fn opendir_error(&self, e: &SftpError, dir: &str) -> AppError {
        if is_status(e, StatusCode::NoSuchFile) || is_status(e, StatusCode::Failure) {
            match self.raw.stat(dir).await {
                Ok(a) if kind_of(&a.attrs) != FileKind::Dir => {
                    return AppError::new(ErrorCode::NotADirectory {
                        path: dir.to_owned(),
                    });
                }
                Err(se) if is_status(&se, StatusCode::NoSuchFile) => {
                    return AppError::new(ErrorCode::RemoteNotFound {
                        path: dir.to_owned(),
                    });
                }
                _ => {}
            }
        }
        self.map_err(e, Some(dir))
    }

    /// `sftp_mkdir`: never replaces anything (`remoteExists`).
    pub async fn mkdir(&self, p: &str) -> Result<FileEntry, AppError> {
        if let Err(e) = self.raw.mkdir(p, FileAttributes::empty()).await {
            return Err(self.exists_error(&e, p, p).await);
        }
        self.entry(p).await
    }

    /// `sftp_create_file`: empty file, mode 0644 (minus the server's umask); `remoteExists` if
    /// anything exists at `p`.
    pub async fn create_file(&self, p: &str) -> Result<FileEntry, AppError> {
        let attrs = FileAttributes {
            permissions: Some(0o644),
            ..FileAttributes::empty()
        };
        let handle = match self
            .raw
            .open(
                p,
                OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE,
                attrs,
            )
            .await
        {
            Ok(h) => h.handle,
            Err(e) => return Err(self.exists_error(&e, p, p).await),
        };
        self.raw
            .close(handle)
            .await
            .map_err(|e| self.map_err(&e, Some(p)))?;
        self.entry(p).await
    }

    /// `sftp_rename`: never overwrites (checked first; SFTP v3 `rename` also refuses on OpenSSH).
    pub async fn rename(&self, from: &str, to: &str) -> Result<FileEntry, AppError> {
        if self.lstat_opt(to).await?.is_some() {
            return Err(AppError::new(ErrorCode::RemoteExists {
                path: to.to_owned(),
            }));
        }
        if let Err(e) = self.raw.rename(from, to).await {
            // OpenSSH answers `noSuchFile` for a missing destination folder too; name
            // the path that is actually missing.
            if is_status(&e, StatusCode::NoSuchFile)
                && matches!(self.lstat_opt(from).await, Ok(Some(_)))
            {
                return Err(AppError::new(ErrorCode::RemoteNotFound {
                    path: path::parent(to).to_owned(),
                })
                .with_detail(e.to_string()));
            }
            return Err(self.exists_error(&e, to, from).await);
        }
        self.entry(to).await
    }

    /// `sftp_delete`: a file or symlink (`remove`), or an EMPTY directory (`rmdir`;
    /// `directoryNotEmpty` otherwise). Never recursive.
    pub async fn delete(&self, p: &str) -> Result<(), AppError> {
        let attrs = self.lstat(p).await?;
        if kind_of(&attrs) == FileKind::Dir {
            if let Err(e) = self.raw.rmdir(p).await {
                if is_status(&e, StatusCode::Failure) && self.dir_has_entries(p).await {
                    return Err(
                        AppError::new(ErrorCode::DirectoryNotEmpty { path: p.to_owned() })
                            .with_detail(e.to_string()),
                    );
                }
                return Err(self.map_err(&e, Some(p)));
            }
            return Ok(());
        }
        self.raw
            .remove(p)
            .await
            .map(|_| ())
            .map_err(|e| self.map_err(&e, Some(p)))
    }

    async fn dir_has_entries(&self, p: &str) -> bool {
        let Ok(h) = self.raw.opendir(p).await else {
            return false;
        };
        let raw = self.raw.clone();
        let handle = h.handle.clone();
        let found = probe_dir_entries(|| {
            let raw = raw.clone();
            let handle = handle.clone();
            async move {
                raw.readdir(handle)
                    .await
                    .ok()
                    .map(|name| name.files.into_iter().map(|f| f.filename).collect())
            }
        })
        .await;
        let _ = self.raw.close(h.handle).await;
        found
    }

    /// Open `p` for reading.
    pub async fn open_read(&self, p: &str) -> Result<String, AppError> {
        self.raw
            .open(p, OpenFlags::READ, FileAttributes::empty())
            .await
            .map(|h| h.handle)
            .map_err(|e| self.map_err(&e, Some(p)))
    }

    /// Best-effort close (errors are logged only).
    pub async fn close_quietly(&self, handle: String) {
        if let Err(e) = self.raw.close(handle).await {
            tracing::debug!(error = %e, "sftp close failed");
        }
    }

    /// Read a whole file (pipelined), at most `cap` bytes plus one: `Ok(bytes)` (its length tells
    /// whether the cap was exceeded).
    pub async fn read_capped(&self, p: &str, cap: u64) -> Result<Vec<u8>, AppError> {
        let handle = self.open_read(p).await?;
        let mut reader = PipelinedReader::new(
            self.raw.clone(),
            handle.clone(),
            self.read_chunk(),
            Some(cap.saturating_add(1)),
        );
        let mut out = Vec::new();
        let result = loop {
            match reader.next().await {
                Ok(Some(chunk)) => out.extend_from_slice(&chunk),
                Ok(None) => break Ok(out),
                Err(e) => break Err(self.map_err(&e, Some(p))),
            }
        };
        drop(reader);
        self.close_quietly(handle).await;
        result
    }

    /// `sftp_read_text` (§6.3).
    pub async fn read_text(
        &self,
        p: &str,
        encoding: Option<&str>,
        limit: u64,
    ) -> Result<TextDocument, AppError> {
        // A symlink is resolved once and the resolved file is read, so the buffer knows
        // exactly which file its content came from (`resolvedPath`, checked again on save).
        let resolved_path = if kind_of(&self.lstat(p).await?) == FileKind::Symlink {
            Some(self.realpath(p).await?)
        } else {
            None
        };
        let source = resolved_path.as_deref().unwrap_or(p);
        let attrs = self.stat(source).await?;
        ensure_regular(&attrs, p)?;
        let too_large = |size: u64| {
            AppError::new(ErrorCode::FileTooLarge {
                path: p.to_owned(),
                size,
                limit,
            })
        };
        if let Some(size) = attrs.size
            && size > limit
        {
            return Err(too_large(size));
        }
        let raw = self.read_capped(source, limit).await?;
        let len = u64::try_from(raw.len()).unwrap_or(u64::MAX);
        if len > limit {
            return Err(too_large(len));
        }
        let decoded = text::decode(p, &raw, encoding)?;
        Ok(TextDocument {
            path: p.to_owned(),
            text: decoded.text,
            encoding: decoded.encoding,
            has_bom: decoded.has_bom,
            eol: decoded.eol,
            mixed_eol: decoded.mixed_eol,
            permissions: mode_bits(&attrs),
            etag: Etag {
                size: len,
                mtime: attrs.mtime.map_or(0, i64::from),
                sha256: text::sha256_hex(&raw),
            },
            resolved_path,
        })
    }
}

/// How many requests a pipelined reader or writer may have in flight.
///
/// The window starts at `floor` and grows by one per request answered within [`SLOW_REQUEST`]
/// (so it doubles per round trip, like TCP slow start) up to `max`; a slower answer halves it,
/// never below `floor`. Every request above `floor` also holds a permit of the session's pool,
/// taken without waiting (none left ⇒ the window just does not grow there). So the transfers of
/// one session together keep at most `floor` × transfers + pool requests in flight, and a
/// transfer never waits on another one.
pub struct Window {
    floor: usize,
    max: usize,
    depth: usize,
    pool: Option<Arc<Semaphore>>,
    extra: Vec<OwnedSemaphorePermit>,
}

impl Window {
    /// A constant window of `depth` requests (editor reads and saves).
    pub fn fixed(depth: usize) -> Self {
        let depth = depth.max(1);
        Self {
            floor: depth,
            max: depth,
            depth,
            pool: None,
            extra: Vec::new(),
        }
    }

    /// A transfer's window: [`PIPELINE_DEPTH`] … [`TRANSFER_DEPTH`], extra requests from `pool`.
    pub fn transfer(pool: Arc<Semaphore>) -> Self {
        Self {
            floor: PIPELINE_DEPTH,
            max: TRANSFER_DEPTH,
            depth: PIPELINE_DEPTH,
            pool: Some(pool),
            extra: Vec::new(),
        }
    }

    /// The current window size.
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// May one more request go out while `inflight` are in flight?
    pub fn admit(&mut self, inflight: usize) -> bool {
        if inflight < self.floor {
            return true;
        }
        if inflight >= self.depth {
            return false;
        }
        let needed = inflight + 1 - self.floor;
        while self.extra.len() < needed {
            let Some(pool) = &self.pool else {
                return false;
            };
            match pool.clone().try_acquire_owned() {
                Ok(p) => self.extra.push(p),
                Err(_) => return false,
            }
        }
        true
    }

    /// A request was answered after `latency`; `inflight` requests remain. Pool permits that
    /// the remaining requests do not need go back at once.
    pub fn completed(&mut self, latency: Duration, inflight: usize) {
        if latency > SLOW_REQUEST {
            self.depth = (self.depth / 2).max(self.floor);
        } else if self.depth < self.max {
            self.depth += 1;
        }
        self.release_to(inflight);
    }

    /// Keep only the permits `inflight` requests need.
    pub fn release_to(&mut self, inflight: usize) {
        self.extra.truncate(inflight.saturating_sub(self.floor));
    }

    /// Pool permits held (tests).
    pub fn extra_permits(&self) -> usize {
        self.extra.len()
    }
}

type ReadFuture =
    Pin<Box<dyn Future<Output = (u64, u32, Result<Data, SftpError>, Duration)> + Send>>;

/// A READ reply cut to the `len` bytes requested, as soon as it arrives. Answers that completed
/// out of order wait in the reader until the earlier ones are in, so a hostile server answering
/// every 32 KiB request with a full 256 KiB packet would otherwise make the window hold 8× its
/// memory bound. An oversized reply is copied into a buffer of the requested size.
fn fit_read(mut data: Vec<u8>, len: u32) -> Vec<u8> {
    let len = len as usize;
    if data.len() > len {
        data.truncate(len);
        data.shrink_to_fit();
    }
    data
}

/// Sequential reads with up to a [`Window`] of requests in flight (§6.3; [`PIPELINE_DEPTH`]
/// unless [`PipelinedReader::with_window`]). Chunks are returned in file order. A short read
/// restarts the pipeline at the first missing byte; EOF ends it. With a `cap`, nothing at or
/// beyond that offset is requested.
pub struct PipelinedReader {
    raw: Arc<RawSftpSession>,
    handle: String,
    chunk: u32,
    cap: Option<u64>,
    requested: u64,
    delivered: u64,
    eof: bool,
    inflight: FuturesOrdered<ReadFuture>,
    window: Window,
}

impl PipelinedReader {
    pub fn new(raw: Arc<RawSftpSession>, handle: String, chunk: u32, cap: Option<u64>) -> Self {
        Self::with_window(raw, handle, chunk, cap, Window::fixed(PIPELINE_DEPTH))
    }

    pub fn with_window(
        raw: Arc<RawSftpSession>,
        handle: String,
        chunk: u32,
        cap: Option<u64>,
        window: Window,
    ) -> Self {
        Self {
            raw,
            handle,
            chunk: chunk.max(1),
            cap,
            requested: 0,
            delivered: 0,
            eof: false,
            inflight: FuturesOrdered::new(),
            window,
        }
    }

    /// Bytes delivered so far.
    pub fn position(&self) -> u64 {
        self.delivered
    }

    fn fill(&mut self) {
        while self.window.admit(self.inflight.len()) {
            let len = match self.cap {
                Some(cap) if self.requested >= cap => break,
                Some(cap) => u32::try_from((cap - self.requested).min(u64::from(self.chunk)))
                    .unwrap_or(self.chunk),
                None => self.chunk,
            };
            let raw = self.raw.clone();
            let handle = self.handle.clone();
            let off = self.requested;
            self.inflight.push_back(Box::pin(async move {
                // The request goes out on the first poll (right after `fill`).
                let sent = tokio::time::Instant::now();
                let res = raw.read(handle, off, len).await.map(|mut d| {
                    d.data = fit_read(d.data, len);
                    d
                });
                (off, len, res, sent.elapsed())
            }));
            self.requested += u64::from(len);
        }
        // `admit` may have taken a permit for a request that was not sent (cap reached).
        self.window.release_to(self.inflight.len());
    }

    /// Drop every request in flight (their answers are ignored by russh-sftp).
    fn reset(&mut self) {
        self.inflight = FuturesOrdered::new();
        self.window.release_to(0);
    }

    /// The next chunk, or `None` at EOF (or when the cap is reached).
    pub async fn next(&mut self) -> Result<Option<Vec<u8>>, SftpError> {
        if self.eof {
            return Ok(None);
        }
        self.fill();
        let Some((off, len, res, latency)) = self.inflight.next().await else {
            return Ok(None);
        };
        self.window.completed(latency, self.inflight.len());
        debug_assert_eq!(off, self.delivered);
        match res {
            Ok(data) => {
                let mut bytes = data.data;
                bytes.truncate(len as usize);
                if bytes.is_empty() {
                    self.eof = true;
                    self.reset();
                    return Ok(None);
                }
                self.delivered += bytes.len() as u64;
                if bytes.len() < len as usize {
                    // Short read: drop the requests behind it and continue at the gap.
                    self.reset();
                    self.requested = self.delivered;
                }
                Ok(Some(bytes))
            }
            Err(e) if is_status(&e, StatusCode::Eof) => {
                self.eof = true;
                self.reset();
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }
}

type WriteFuture = Pin<Box<dyn Future<Output = (Result<(), SftpError>, Duration)> + Send>>;

/// Writes at increasing offsets with up to a [`Window`] of requests in flight
/// ([`PIPELINE_DEPTH`] unless [`PipelinedWriter::with_window`]).
pub struct PipelinedWriter {
    raw: Arc<RawSftpSession>,
    handle: String,
    offset: u64,
    inflight: FuturesUnordered<WriteFuture>,
    window: Window,
}

impl PipelinedWriter {
    pub fn new(raw: Arc<RawSftpSession>, handle: String) -> Self {
        Self::with_window(raw, handle, Window::fixed(PIPELINE_DEPTH))
    }

    pub fn with_window(raw: Arc<RawSftpSession>, handle: String, window: Window) -> Self {
        Self {
            raw,
            handle,
            offset: 0,
            inflight: FuturesUnordered::new(),
            window,
        }
    }

    /// Wait for one queued write.
    async fn complete_one(&mut self) -> Result<(), SftpError> {
        if let Some((r, latency)) = self.inflight.next().await {
            self.window.completed(latency, self.inflight.len());
            r?;
        }
        Ok(())
    }

    /// Queue one chunk (callers keep chunks ≤ `Sftp::write_chunk`).
    pub async fn write(&mut self, data: Vec<u8>) -> Result<(), SftpError> {
        while !self.window.admit(self.inflight.len()) {
            self.complete_one().await?;
        }
        let raw = self.raw.clone();
        let handle = self.handle.clone();
        let off = self.offset;
        self.offset += data.len() as u64;
        self.inflight.push(Box::pin(async move {
            let sent = tokio::time::Instant::now();
            let res = raw.write(handle, off, data).await.map(|_| ());
            (res, sent.elapsed())
        }));
        // An async request goes out on its first poll: send it now (not only when the window is
        // full, so a throttled or slow source still keeps the link busy), and take any answers
        // that are already in.
        while let Some(Some((r, latency))) = self.inflight.next().now_or_never() {
            self.window.completed(latency, self.inflight.len());
            r?;
        }
        Ok(())
    }

    /// Wait for every queued write.
    pub async fn finish(&mut self) -> Result<(), SftpError> {
        while !self.inflight.is_empty() {
            self.complete_one().await?;
        }
        Ok(())
    }

    pub fn written(&self) -> u64 {
        self.offset
    }
}

/// Split `bytes` into chunks of `size` (for [`PipelinedWriter`]).
pub fn chunks(bytes: &[u8], size: usize) -> VecDeque<Vec<u8>> {
    bytes.chunks(size.max(1)).map(<[u8]>::to_vec).collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::*;

    #[test]
    fn kinds_and_modes() {
        let a = |p: Option<u32>| FileAttributes {
            permissions: p,
            ..FileAttributes::empty()
        };
        assert_eq!(kind_of(&a(Some(0o040_755))), FileKind::Dir);
        assert_eq!(kind_of(&a(Some(0o100_644))), FileKind::File);
        assert_eq!(kind_of(&a(Some(0o120_777))), FileKind::Symlink);
        assert_eq!(kind_of(&a(Some(0o010_644))), FileKind::Other);
        assert_eq!(kind_of(&a(None)), FileKind::Other);
        assert_eq!(mode_bits(&a(Some(0o104_755))), Some(0o4755));
        let e = entry_from(
            "/d/x\u{FFFD}".into(),
            "x\u{FFFD}".into(),
            &a(Some(0o100_600)),
            None,
        );
        assert!(e.lossy_name);
        assert_eq!(e.permissions, Some(0o600));
        assert_eq!((e.owner, e.group), (None, None));
    }

    /// Only regular files (or attributes without a type) may be read.
    #[test]
    fn only_regular_files_are_read() {
        let a = |p: Option<u32>| FileAttributes {
            permissions: p,
            ..FileAttributes::empty()
        };
        assert!(ensure_regular(&a(Some(0o100_644)), "/f").is_ok());
        assert!(ensure_regular(&a(None), "/f").is_ok());
        assert!(ensure_regular(&a(Some(0o644)), "/f").is_ok());
        assert!(matches!(
            ensure_regular(&a(Some(0o040_755)), "/d").unwrap_err().kind,
            ErrorCode::IsADirectory { .. }
        ));
        // FIFO, char device (/dev/zero), block device, socket, and a link that was not followed.
        for mode in [0o010_644, 0o020_666, 0o060_660, 0o140_777, 0o120_777] {
            assert_eq!(
                ensure_regular(&a(Some(mode)), "/x").unwrap_err().kind,
                ErrorCode::NotARegularFile { path: "/x".into() },
                "{mode:o}"
            );
        }
    }

    /// A server answering every READDIR with an empty NAME packet cannot make the
    /// emptiness probe spin forever; the probe stops after the packet cap (and then says "not
    /// empty", so nothing is removed).
    #[tokio::test]
    async fn dir_probe_is_bounded_against_endless_empty_replies() {
        let mut calls = 0usize;
        let found = probe_dir_entries(|| {
            calls += 1;
            async { Some(Vec::new()) }
        })
        .await;
        assert!(found);
        assert_eq!(calls, DIR_PROBE_MAX_PACKETS);

        // Only `.`/`..`, then EOF: empty.
        let mut replies = vec![
            Some(vec![".".to_owned(), "..".to_owned()]),
            Some(Vec::new()),
            None,
        ]
        .into_iter();
        assert!(!probe_dir_entries(|| std::future::ready(replies.next().flatten())).await);
        // A real entry in a later packet.
        let mut replies = vec![Some(vec![".".to_owned()]), Some(vec!["a".to_owned()])].into_iter();
        assert!(probe_dir_entries(|| std::future::ready(replies.next().flatten())).await);
    }

    /// A READDIR that never answers ends at the listing deadline.
    #[tokio::test(start_paused = true)]
    async fn dir_probe_has_a_deadline() {
        let found = probe_dir_entries(std::future::pending::<Option<Vec<String>>>).await;
        assert!(found);
    }

    /// Owner/group from the long name only when it is consistent; hostile long
    /// names never produce a name with controls, spaces, bidi characters or excess length.
    #[test]
    fn owner_and_group_from_longname() {
        let attrs = |size: u64| FileAttributes {
            permissions: Some(0o100_644),
            size: Some(size),
            uid: Some(1000),
            gid: Some(50),
            ..FileAttributes::empty()
        };
        let og = |l: &str, size: u64| owner_group_from_longname(l, &attrs(size));
        let some = |o: &str, g: &str| (Some(o.to_owned()), Some(g.to_owned()));
        assert_eq!(
            og(
                "-rw-r--r--    1 alice    staff        1234 Jan  1 12:00 report.pdf",
                1234
            ),
            some("alice", "staff")
        );
        // A name with spaces in the file name part does not matter.
        assert_eq!(
            og("drwxr-xr-x  12 root root 4096 Oct 21  2026 my dir", 4096),
            some("root", "root")
        );
        // ACL / xattr markers in the mode column.
        assert_eq!(
            og("-rw-r--r--+  1 www-data adm 7 Jan 1 2026 x", 7),
            (None, None),
            "11-char mode token is not the ls format"
        );
        assert_eq!(
            og("-rw-r--r-@ 1 bob wheel 7 Jan 1 2026 x", 7),
            some("bob", "wheel")
        );
        // Numeric owner/group = no name.
        assert_eq!(og("-rw-r--r-- 1 1000 50 9 Jan 1 2026 x", 9), (None, None));
        assert_eq!(
            og("-rw-r--r-- 1 alice 50 9 Jan 1 2026 x", 9),
            (Some("alice".into()), None)
        );
        // Inconsistent with the attributes: size mismatch, missing size, bad columns.
        assert_eq!(
            og("-rw-r--r-- 1 alice staff 10 Jan 1 2026 x", 9),
            (None, None)
        );
        assert_eq!(
            owner_group_from_longname(
                "-rw-r--r-- 1 alice staff 9 Jan 1 x",
                &FileAttributes::empty()
            ),
            (None, None)
        );
        for hostile in [
            "",
            "x",
            "report.pdf",
            "-rw-r--r-- x alice staff 9 Jan 1 x",
            "-rw-r--r-- 1 alice staff",
            "rm -rf / 1 alice staff 9",
            "-rw-r--r-- 1 NT AUTHORITY\\SYSTEM Administrators 9 Jan 1 x",
            "-rw-r--r-\u{1b} 1 alice staff 9 Jan 1 x",
        ] {
            assert_eq!(og(hostile, 9), (None, None), "{hostile:?}");
        }
        let long = "a".repeat(MAX_OWNER_CHARS + 1);
        assert_eq!(
            og(&format!("-rw-r--r-- 1 {long} staff 9 J 1 x"), 9),
            (None, Some("staff".into()))
        );
        assert_eq!(
            og("-rw-r--r-- 1 ev\u{202e}il st\u{200b}aff 9 J 1 x", 9),
            (None, None)
        );
        assert_eq!(
            og("-rw-r--r-- 1 ev\u{7f}il st\u{85}aff 9 J 1 x", 9),
            (None, None)
        );
        assert_eq!(
            og("-rw-r--r-- 1 алиса група 9 J 1 x", 9),
            some("алиса", "група")
        );

        // The listing carries them and charges them to the budget.
        let mut b = ListingBuilder::new("/d");
        assert!(b.push_long(
            "x".into(),
            "-rw-r--r-- 1 alice staff 1 Jan 1 2026 x",
            &file_attrs()
        ));
        let with_names = b.cost();
        let mut b2 = ListingBuilder::new("/d");
        assert!(b2.push("x".into(), &file_attrs()));
        assert_eq!(with_names, b2.cost() + 2 * ("alice".len() + "staff".len()));
        let l = b.finish();
        assert_eq!(l.entries[0].owner.as_deref(), Some("alice"));
        assert_eq!(l.entries[0].group.as_deref(), Some("staff"));
        assert_eq!(b2.finish().entries[0].owner, None);
    }

    fn file_attrs() -> FileAttributes {
        FileAttributes {
            permissions: Some(0o100_644),
            size: Some(1),
            ..FileAttributes::empty()
        }
    }

    /// `truncated` ⇔ an entry was left out; exactly `max` entries is a complete listing.
    #[test]
    fn listing_cap_boundaries() {
        let mut b = ListingBuilder::with_limits("/d", 3, usize::MAX);
        for n in ["a", "b", "c"] {
            assert!(b.push(n.into(), &file_attrs()));
        }
        let full = b.finish();
        assert_eq!((full.entries.len(), full.truncated), (3, false));
        let mut b = ListingBuilder::with_limits("/d", 3, usize::MAX);
        let pushed: Vec<bool> = ["a", ".", "..", "x/y", "b", "c", "d", "e"]
            .iter()
            .map(|n| b.push((*n).into(), &file_attrs()))
            .collect();
        assert_eq!(
            pushed,
            vec![true, true, true, true, true, true, false, false]
        );
        let l = b.finish();
        assert_eq!((l.entries.len(), l.truncated), (3, true));
        assert_eq!(l.entries[0].path, "/d/a");
        let names: Vec<_> = l.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    /// Path and JSON escapes count. A 4 KiB directory or control-character names stop
    /// the listing at the byte budget, and the serialized listing stays within it.
    #[test]
    fn listing_budget_counts_paths_and_escapes() {
        assert_eq!(json_str_len("ab"), 2);
        assert_eq!(json_str_len("a\"\\\n"), 7);
        assert_eq!(json_str_len("\u{1}"), 6);
        assert_eq!(json_str_len("ж"), 2);
        let s = "q\"\u{1}\u{7f}\n\\ж\u{85}";
        assert_eq!(json_str_len(s), serde_json::to_string(s).unwrap().len() - 2);

        let budget = 4 * 1024 * 1024;
        let long_dir = format!("/{}", "d".repeat(4095));
        let mut b = ListingBuilder::with_limits(&long_dir, LIST_MAX_ENTRIES, budget);
        let mut i = 0u32;
        while b.push(format!("{i:x}"), &file_attrs()) {
            i += 1;
        }
        let cost = b.cost();
        let l = b.finish();
        assert!(l.truncated);
        assert!(l.entries.len() < 600, "{}", l.entries.len());
        let json = serde_json::to_string(&l).unwrap().len();
        assert!(json <= budget && cost <= budget, "json {json}, cost {cost}");

        let mut b = ListingBuilder::with_limits("/tmp", LIST_MAX_ENTRIES, budget);
        let mut i = 0usize;
        while b.push(format!("{}{i}", "\u{1}".repeat(200)), &file_attrs()) {
            i += 1;
        }
        let l = b.finish();
        assert!(l.truncated);
        let json = serde_json::to_string(&l).unwrap().len();
        assert!(json <= budget, "json {json}");

        // The normal case still reaches 100 000 entries.
        let mut b = ListingBuilder::new("/home/someone/projects/app");
        for i in 0..LIST_MAX_ENTRIES {
            assert!(b.push(format!("file-{i:06}.txt"), &file_attrs()));
        }
        assert!(!b.push("one-more".into(), &file_attrs()));
        assert_eq!(b.finish().entries.len(), LIST_MAX_ENTRIES);
    }

    /// The window grows per fast answer up to TRANSFER_DEPTH, halves on a slow one (never
    /// below the floor), and requests above the floor need pool permits.
    #[test]
    fn transfer_window_grows_shrinks_and_is_bounded() {
        let fast = Duration::from_millis(50);
        let slow = SLOW_REQUEST + Duration::from_millis(1);
        let pool = Arc::new(Semaphore::new(SESSION_EXTRA_REQUESTS));
        let mut w = Window::transfer(pool.clone());
        // The floor never needs a permit.
        assert!((0..PIPELINE_DEPTH).all(|n| w.admit(n)));
        assert!(!w.admit(PIPELINE_DEPTH), "window still at the floor");
        assert_eq!(w.extra_permits(), 0);
        // Slow start: one more per fast answer, capped.
        for _ in 0..1000 {
            w.completed(fast, 0);
        }
        assert_eq!(w.depth(), TRANSFER_DEPTH);
        let mut inflight = 0;
        while w.admit(inflight) {
            inflight += 1;
        }
        assert_eq!(inflight, TRANSFER_DEPTH);
        assert_eq!(w.extra_permits(), TRANSFER_DEPTH - PIPELINE_DEPTH);
        assert_eq!(
            pool.available_permits(),
            SESSION_EXTRA_REQUESTS - (TRANSFER_DEPTH - PIPELINE_DEPTH)
        );
        // A slow answer halves the window; permits beyond what is in flight go back.
        w.completed(slow, inflight - 1);
        assert_eq!(w.depth(), TRANSFER_DEPTH / 2);
        assert_eq!(w.extra_permits(), TRANSFER_DEPTH - 1 - PIPELINE_DEPTH);
        for _ in 0..10 {
            w.completed(slow, 0);
        }
        assert_eq!(w.depth(), PIPELINE_DEPTH, "never below the floor");
        assert_eq!(w.extra_permits(), 0);
        assert_eq!(pool.available_permits(), SESSION_EXTRA_REQUESTS);
        // Dropping a window returns its permits.
        let mut w = Window::transfer(pool.clone());
        for _ in 0..100 {
            w.completed(fast, 0);
        }
        assert!(w.admit(PIPELINE_DEPTH + 3));
        assert_eq!(w.extra_permits(), 4);
        drop(w);
        assert_eq!(pool.available_permits(), SESSION_EXTRA_REQUESTS);
    }

    /// The transfers of one session share the pool; once it is empty, every transfer
    /// still keeps its floor (no transfer waits on another), so the session total is bounded by
    /// floor × transfers + pool.
    #[test]
    fn transfer_windows_share_the_session_pool() {
        let pool = Arc::new(Semaphore::new(SESSION_EXTRA_REQUESTS));
        let mut windows: Vec<Window> = (0..8).map(|_| Window::transfer(pool.clone())).collect();
        let mut total = 0;
        for w in &mut windows {
            for _ in 0..100 {
                w.completed(Duration::ZERO, 0);
            }
            let mut n = 0;
            while w.admit(n) {
                n += 1;
            }
            assert!(n >= PIPELINE_DEPTH);
            total += n;
        }
        assert_eq!(total, 8 * PIPELINE_DEPTH + SESSION_EXTRA_REQUESTS);
        assert_eq!(pool.available_permits(), 0);
        // A fixed window never touches a pool and never grows.
        let mut f = Window::fixed(PIPELINE_DEPTH);
        f.completed(Duration::ZERO, 0);
        assert_eq!(f.depth(), PIPELINE_DEPTH);
        assert!(!f.admit(PIPELINE_DEPTH));
        drop(windows);
        assert_eq!(pool.available_permits(), SESSION_EXTRA_REQUESTS);
    }

    fn file(name: &str) -> russh_sftp::protocol::File {
        russh_sftp::protocol::File {
            filename: name.to_owned(),
            longname: String::new(),
            attrs: file_attrs(),
        }
    }

    /// A listing that runs into its deadline returns what it read so far, marked
    /// truncated; with nothing read it is `timeout{listing}`.
    #[tokio::test(start_paused = true)]
    async fn slow_listing_returns_partial_entries() {
        let deadline = tokio::time::Instant::now() + LIST_TIMEOUT;
        let mut calls = 0;
        let l = collect_listing("/d", deadline, || {
            calls += 1;
            let first = calls == 1;
            async move {
                if first {
                    Ok(Some(vec![file("a"), file("b")]))
                } else {
                    std::future::pending().await
                }
            }
        })
        .await
        .unwrap();
        assert!(l.truncated);
        assert_eq!(l.entries.len(), 2);

        let e = collect_listing("/d", deadline + LIST_TIMEOUT, || {
            std::future::pending::<Result<Option<Vec<russh_sftp::protocol::File>>, AppError>>()
        })
        .await
        .unwrap_err();
        assert_eq!(
            e.kind,
            ErrorCode::Timeout {
                op: TimeoutOp::Listing
            }
        );

        // A complete listing is not truncated; an error passes through.
        let mut replies = vec![Ok(Some(vec![file("a")])), Ok(None)].into_iter();
        let l = collect_listing("/d", tokio::time::Instant::now() + LIST_TIMEOUT, || {
            std::future::ready(replies.next().unwrap())
        })
        .await
        .unwrap();
        assert!(!l.truncated);
        assert_eq!(l.entries.len(), 1);
        let e = collect_listing("/d", tokio::time::Instant::now() + LIST_TIMEOUT, || {
            std::future::ready(Err(AppError::new(ErrorCode::SftpUnavailable)))
        })
        .await
        .unwrap_err();
        assert_eq!(e.kind, ErrorCode::SftpUnavailable);
    }

    /// A READ reply longer than requested (hostile server) is cut and does not keep
    /// its oversized allocation while it waits in the window; normal replies are untouched.
    #[test]
    fn oversized_read_replies_are_cut_to_the_request() {
        let big = vec![7u8; 256 * 1024];
        let cut = fit_read(big, CHUNK);
        assert_eq!(cut.len(), CHUNK as usize);
        assert!(cut.capacity() < 2 * CHUNK as usize, "{}", cut.capacity());
        let short = fit_read(vec![1, 2, 3], CHUNK);
        assert_eq!(short, vec![1, 2, 3]);
        assert!(fit_read(Vec::new(), CHUNK).is_empty());
        assert_eq!(fit_read(vec![9; 10], 10).len(), 10);
    }

    #[test]
    fn chunking() {
        let c = chunks(&[0u8; 70_000], 32_768);
        assert_eq!(
            c.iter().map(Vec::len).collect::<Vec<_>>(),
            vec![32_768, 32_768, 4_464]
        );
        assert!(chunks(&[], 10).is_empty());
    }
}
