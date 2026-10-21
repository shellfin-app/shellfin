//! Atomic, conflict-safe, ownership-preserving writes (§6.4), shared by `sftp_write_text` and
//! uploads (§6.6).
//!
//! ```text
//! tmp = dir/.<name>.sshb-<6 random [a-z0-9]>.tmp
//! open(tmp, CREATE|EXCLUDE|WRITE, {permissions: perms})   PermissionDenied ⇒ InPlace
//! t = fstat(tmp)
//!   t.uid ≠ cur.uid ⇒ close, remove(tmp) ⇒ InPlace                 (owner would change)
//!   t.gid ≠ cur.gid ⇒ fsetstat(tmp, {uid: t.uid, gid: cur.gid})    (always as a pair: russh-sftp
//!                     sends 0 for a missing member); failure ⇒ InPlace
//! Atomic:   pipelined writes; fsetstat(tmp, {permissions}) BEFORE close; fsync if supported;
//!           fstat (the new etag); close; [save without force: lstat(target) is still
//!           `cur`, else remoteChanged]; posix-rename(tmp, target)  |  RenameFallback:
//!           rename(target, bak); rename(tmp, target); remove(bak)
//! InPlace:  open(target, WRITE); fstat == resolved; fsetstat{size:0}; write; fsync?; fstat;
//!           close   (owner, group, ACLs, links kept)
//! any error: best-effort remove(tmp) (and restore bak in RenameFallback)
//! ```
//! A new file without `replace` is moved into place with a plain SFTP `rename`, which never
//! overwrites (OpenSSH links + unlinks), so a file created meanwhile is not clobbered.
//!
//! The RenameFallback backup is `dir/.<name>.sshb-<6 random>.bak` (unique, length-capped
//! like the temp name, so a stale backup never blocks later saves). If the connection drops
//! between its two renames, the original survives only as that backup file (and the new content
//! as the temp file); nothing is lost, but the user has to rename it back by hand.

use std::future::Future;
use std::sync::Arc;

use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use tokio::sync::{Semaphore, watch};

use crate::error::{AppError, ErrorCode};
use crate::ipc::dto::{Etag, FileKind, SaveRequest, SaveResult, SaveStrategy};

use super::client::{
    PIPELINE_DEPTH, PipelinedWriter, Sftp, Window, ensure_regular, is_status, kind_of, mode_bits,
};
use super::{path, text};

/// Default mode for files created by uploads and saves of new files (§6.6).
pub const NEW_FILE_MODE: u32 = 0o644;
/// Longest part of the target name kept in temp and backup names (bytes).
const SIDE_NAME_MAX: usize = 200;
/// Temp-name tries before giving up (a collision of 6 random chars is unlikely).
const TMP_TRIES: usize = 3;

/// Where the data comes from (a buffer for saves, a local file for uploads).
pub trait DataSource: Send {
    /// The next chunk of at most `max` bytes, or `None` at the end.
    fn next_chunk(
        &mut self,
        max: usize,
    ) -> impl Future<Output = Result<Option<Vec<u8>>, AppError>> + Send;
}

/// An in-memory source.
pub struct BytesSource<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> BytesSource<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }
}

impl DataSource for BytesSource<'_> {
    async fn next_chunk(&mut self, max: usize) -> Result<Option<Vec<u8>>, AppError> {
        let rest = self.bytes.get(self.pos..).unwrap_or_default();
        if rest.is_empty() {
            return Ok(None);
        }
        let n = rest.len().min(max.max(1));
        self.pos += n;
        Ok(Some(rest.get(..n).unwrap_or_default().to_vec()))
    }
}

/// Progress and cancellation hooks of a write (uploads); saves never cancel.
pub struct Hooks<'a> {
    pub progress: &'a mut (dyn FnMut(u64) + Send),
    pub cancelled: &'a (dyn Fn() -> bool + Send + Sync),
    /// The transfer's cancel flag as a signal: a write or local read stuck on a stalled
    /// server is abandoned as soon as the transfer is cancelled, not after the request timeout.
    pub cancel_signal: Option<watch::Receiver<bool>>,
    /// The session's transfer pool; with it the writes use a transfer [`Window`]
    /// (up to `TRANSFER_DEPTH` requests), without it the fixed `PIPELINE_DEPTH` (saves).
    pub window_pool: Option<Arc<Semaphore>>,
}

impl Hooks<'_> {
    pub fn check(&self) -> Result<(), AppError> {
        if (self.cancelled)() {
            Err(AppError::new(ErrorCode::Cancelled))
        } else {
            Ok(())
        }
    }
}

/// Resolves once `signal` turns `true` (never without a signal, or when its sender is gone).
pub async fn until_cancelled(signal: Option<&mut watch::Receiver<bool>>) {
    if let Some(rx) = signal
        && rx.wait_for(|c| *c).await.is_ok()
    {
        return;
    }
    std::future::pending::<()>().await;
}

/// `fut`, or `cancelled` as soon as `signal` fires.
pub async fn or_cancelled<T>(
    signal: &mut Option<watch::Receiver<bool>>,
    fut: impl Future<Output = Result<T, AppError>>,
) -> Result<T, AppError> {
    tokio::select! {
        biased;
        () = until_cancelled(signal.as_mut()) => Err(AppError::new(ErrorCode::Cancelled)),
        r = fut => r,
    }
}

/// Six random characters `[a-z0-9]`.
fn random_suffix() -> String {
    use ring::rand::SecureRandom;
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut bytes = [0u8; 6];
    if ring::rand::SystemRandom::new().fill(&mut bytes).is_err() {
        // Fall back to the UUID generator's randomness.
        bytes.copy_from_slice(uuid::Uuid::new_v4().as_bytes().get(..6).unwrap_or(&[0; 6]));
    }
    bytes
        .iter()
        .map(|b| {
            char::from(
                *ALPHABET
                    .get(usize::from(*b) % ALPHABET.len())
                    .unwrap_or(&b'x'),
            )
        })
        .collect()
}

/// `dir/.<name>.sshb-XXXXXX.<ext>` (the name part is capped at 200 bytes so the result stays
/// below common `NAME_MAX` limits).
fn side_path(target: &str, ext: &str) -> String {
    let name = path::basename(target);
    let mut cut = name.len().min(SIDE_NAME_MAX);
    while !name.is_char_boundary(cut) {
        cut -= 1;
    }
    let short = name.get(..cut).unwrap_or(name);
    path::join(
        path::parent(target),
        &format!(".{short}.sshb-{}.{ext}", random_suffix()),
    )
}

/// `dir/.<name>.sshb-XXXXXX.tmp` (name part ≤ 200 bytes).
pub fn temp_path(target: &str) -> String {
    side_path(target, "tmp")
}

/// The RenameFallback backup `dir/.<name>.sshb-XXXXXX.bak` (unique and length-capped).
pub fn bak_path(target: &str) -> String {
    side_path(target, "bak")
}

/// The file a write lands in: a symlink is resolved (its target is written, §6.4), and the
/// current attributes (`None` = does not exist yet).
pub async fn resolve_target(
    sftp: &Sftp,
    p: &str,
) -> Result<(String, Option<FileAttributes>), AppError> {
    let Some(l) = sftp.lstat_opt(p).await? else {
        return Ok((p.to_owned(), None));
    };
    let target = if kind_of(&l) == FileKind::Symlink {
        sftp.realpath(p).await?
    } else {
        p.to_owned()
    };
    match sftp.raw.stat(target.as_str()).await {
        Ok(a) => Ok((target, Some(a.attrs))),
        // A dangling link: write a new file at the link's target.
        Err(e) if is_status(&e, StatusCode::NoSuchFile) => Ok((target, None)),
        Err(e) => Err(sftp.map_err(&e, Some(&target))),
    }
}

async fn remove_quietly(sftp: &Sftp, p: &str) {
    if let Err(e) = sftp.raw.remove(p).await {
        tracing::debug!(error = %e, "could not remove a temporary file");
    }
}

/// After a cancel or on a lost connection, cleanup requests (close, remove) can queue behind a
/// full SSH channel window that will never drain (a deep transfer window); each waits at
/// most this long instead of the 30 s request timeout.
const STOP_CLEANUP_WAIT: std::time::Duration = std::time::Duration::from_millis(500);

/// The write is being given up: cancelled, or the connection is gone.
fn stopping(sftp: &Sftp, hooks: &Hooks<'_>) -> bool {
    (hooks.cancelled)() || sftp.conn.is_gone()
}

/// Close the handle after the writes; bounded (and its result ignored) when the write failed
/// because it is being given up (`give_up`: failed and [`stopping`]).
async fn close_after(sftp: &Sftp, handle: &str, give_up: bool) -> Result<(), SftpError> {
    if give_up {
        let _ = tokio::time::timeout(STOP_CLEANUP_WAIT, sftp.raw.close(handle)).await;
        return Ok(());
    }
    sftp.raw.close(handle).await.map(|_| ())
}

/// [`remove_quietly`] for the temp file, bounded when the write is being given up.
async fn remove_after(sftp: &Sftp, stop: bool, p: &str) {
    if stop {
        let _ = tokio::time::timeout(STOP_CLEANUP_WAIT, remove_quietly(sftp, p)).await;
    } else {
        remove_quietly(sftp, p).await;
    }
}

/// Write everything from `source` into `handle` (pipelined).
async fn pump<S: DataSource>(
    sftp: &Sftp,
    handle: &str,
    target: &str,
    source: &mut S,
    hooks: &mut Hooks<'_>,
) -> Result<u64, AppError> {
    let window = hooks
        .window_pool
        .clone()
        .map_or_else(|| Window::fixed(PIPELINE_DEPTH), Window::transfer);
    let mut writer = PipelinedWriter::with_window(sftp.raw.clone(), handle.to_owned(), window);
    let chunk = sftp.write_chunk();
    let mut cancel = hooks.cancel_signal.clone();
    loop {
        hooks.check()?;
        let Some(data) = or_cancelled(&mut cancel, source.next_chunk(chunk)).await? else {
            break;
        };
        or_cancelled(&mut cancel, async {
            writer
                .write(data)
                .await
                .map_err(|e| sftp.map_err(&e, Some(target)))
        })
        .await?;
        (hooks.progress)(writer.written());
    }
    or_cancelled(&mut cancel, async {
        writer
            .finish()
            .await
            .map_err(|e| sftp.map_err(&e, Some(target)))
    })
    .await?;
    Ok(writer.written())
}

async fn fsync_if_supported(sftp: &Sftp, handle: &str, target: &str) -> Result<(), AppError> {
    if sftp.fsync {
        sftp.raw
            .fsync(handle)
            .await
            .map_err(|e| sftp.map_err(&e, Some(target)))?;
    }
    Ok(())
}

/// A created temp file, or the reason to write in place.
enum Temp {
    Created { path: String, handle: String },
    InPlace,
}

async fn create_temp(sftp: &Sftp, target: &str, perms: u32) -> Result<Temp, AppError> {
    let attrs = FileAttributes {
        permissions: Some(perms),
        ..FileAttributes::empty()
    };
    let mut last: Option<SftpError> = None;
    for _ in 0..TMP_TRIES {
        let tmp = temp_path(target);
        match sftp
            .raw
            .open(
                tmp.as_str(),
                OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE,
                attrs.clone(),
            )
            .await
        {
            Ok(h) => {
                return Ok(Temp::Created {
                    path: tmp,
                    handle: h.handle,
                });
            }
            Err(e) if is_status(&e, StatusCode::PermissionDenied) => return Ok(Temp::InPlace),
            // OpenSSH reports EEXIST as `failure`: try another name.
            Err(e) if is_status(&e, StatusCode::Failure) => last = Some(e),
            Err(e) => return Err(sftp.map_err(&e, Some(target))),
        }
    }
    let e = last.unwrap_or(SftpError::UnexpectedBehavior("temp file".into()));
    Err(sftp.map_err(&e, Some(target)))
}

/// The `fsetstat` attributes that move the temp file to group `gid`: uid and gid always travel as
/// a pair (russh-sftp would send 0 for a missing member, §2.3), and the uid is the temp file's own.
/// `None` if the temp file's uid is unknown.
pub fn chown_attrs(temp: &FileAttributes, gid: u32) -> Option<FileAttributes> {
    Some(FileAttributes {
        uid: Some(temp.uid?),
        gid: Some(gid),
        ..FileAttributes::empty()
    })
}

/// Ownership step of §6.4: `Ok(true)` keeps the atomic strategy, `Ok(false)` means InPlace.
/// Also makes sure the temp file has `perms` before any data is written (a server that ignores
/// the `permissions` attribute of OPEN would otherwise expose a 0600 file's content under its
/// umask mode until the final `fsetstat`).
async fn keep_ownership(
    sftp: &Sftp,
    handle: &str,
    cur: &FileAttributes,
    perms: u32,
) -> Result<bool, AppError> {
    let t = match sftp.raw.fstat(handle).await {
        Ok(a) => a.attrs,
        Err(e) => return Err(sftp.map_err(&e, None)),
    };
    if t.permissions.map(|p| p & 0o7777) != Some(perms) {
        let mode = FileAttributes {
            permissions: Some(perms),
            ..FileAttributes::empty()
        };
        if sftp.raw.fsetstat(handle, mode).await.is_err() {
            return Ok(false);
        }
    }
    if let (Some(tu), Some(cu)) = (t.uid, cur.uid)
        && tu != cu
    {
        return Ok(false);
    }
    if let (Some(tg), Some(cg)) = (t.gid, cur.gid)
        && tg != cg
    {
        let (Some(tu), Some(chown)) = (t.uid, chown_attrs(&t, cg)) else {
            return Ok(false);
        };
        if sftp.raw.fsetstat(handle, chown).await.is_err() {
            return Ok(false);
        }
        match sftp.raw.fstat(handle).await {
            Ok(a) if a.attrs.gid == Some(cg) && a.attrs.uid == Some(tu) => {}
            _ => return Ok(false),
        }
    }
    Ok(true)
}

/// `true` if the opened file is the one resolved earlier: a regular file with the same owner,
/// group, mode, size and mtime.
pub fn same_file(opened: &FileAttributes, resolved: &FileAttributes) -> bool {
    kind_of(opened) == FileKind::File
        && opened.uid == resolved.uid
        && opened.gid == resolved.gid
        && opened.permissions == resolved.permissions
        && opened.size == resolved.size
        && opened.mtime == resolved.mtime
}

/// The target is still the file the save started from (`lstat`, no link following), right
/// before the temp file replaces it. Narrows the lost-update window from "read the buffer …
/// rename" to "this request … rename" (SFTP v3 has no compare-and-swap rename).
async fn still_same(sftp: &Sftp, target: &str, cur: &FileAttributes) -> Result<(), AppError> {
    match sftp.lstat_opt(target).await? {
        Some(now) if same_file(&now, cur) => Ok(()),
        Some(now) => Err(changed(target, &now)),
        None => Err(changed(target, &FileAttributes::empty())),
    }
}

fn changed(target: &str, now: &FileAttributes) -> AppError {
    AppError::new(ErrorCode::RemoteChanged {
        path: target.to_owned(),
        size: now.size.unwrap_or(0),
        mtime: now.mtime.map_or(0, i64::from),
        target: None,
    })
}

/// InPlace: rewrite the existing file (keeps owner, group, ACLs, xattrs, hard links).
///
/// SFTP v3 has no `O_NOFOLLOW`, and in-place is used exactly for files owned by someone
/// else, who may swap the entry for a symlink (e.g. to this user's `~/.ssh/authorized_keys`)
/// between resolution and open. So the target is opened `WRITE` only — no `TRUNCATE`, no
/// `CREATE` — and the **opened** file is compared with the resolved one (`cur`); a different
/// file ⇒ `remoteChanged`, nothing written. Only then is it truncated through the handle
/// (`fsetstat{size: 0}`, OpenSSH: `ftruncate`), so a later swap cannot redirect the write.
async fn write_in_place<S: DataSource>(
    sftp: &Sftp,
    target: &str,
    cur: &FileAttributes,
    source: &mut S,
    hooks: &mut Hooks<'_>,
    times: Option<(u32, u32)>,
) -> Result<(u64, Option<FileAttributes>), AppError> {
    hooks.check()?;
    if let Some(hook) = &sftp.in_place_hook {
        hook();
    }
    let handle = sftp
        .raw
        .open(target, OpenFlags::WRITE, FileAttributes::empty())
        .await
        .map_err(|e| sftp.map_err(&e, Some(target)))?
        .handle;
    let result = async {
        let now = sftp
            .raw
            .fstat(handle.as_str())
            .await
            .map_err(|e| sftp.map_err(&e, Some(target)))?
            .attrs;
        if !same_file(&now, cur) {
            tracing::warn!("in-place write: the opened file is not the resolved one; refused");
            return Err(changed(target, &now));
        }
        let truncate = FileAttributes {
            size: Some(0),
            ..FileAttributes::empty()
        };
        sftp.raw
            .fsetstat(handle.as_str(), truncate)
            .await
            .map_err(|e| sftp.map_err(&e, Some(target)))?;
        let n = pump(sftp, &handle, target, source, hooks).await?;
        set_times_quietly(sftp, &handle, times).await;
        fsync_if_supported(sftp, &handle, target).await?;
        Ok::<_, AppError>((n, fstat_quietly(sftp, &handle).await))
    }
    .await;
    let give_up = result.is_err() && stopping(sftp, hooks);
    let closed = close_after(sftp, &handle, give_up).await;
    let n = result?;
    closed.map_err(|e| sftp.map_err(&e, Some(target)))?;
    Ok(n)
}

/// The written file's attributes through its still-open handle (the new etag must
/// describe what *we* wrote; a `stat` after close or rename could see someone else's write).
async fn fstat_quietly(sftp: &Sftp, handle: &str) -> Option<FileAttributes> {
    match sftp.raw.fstat(handle).await {
        Ok(a) => Some(a.attrs),
        Err(e) => {
            tracing::debug!(error = %e, "fstat of the written file failed");
            None
        }
    }
}

/// Move the finished temp file into place.
async fn commit(
    sftp: &Sftp,
    tmp: &str,
    target: &str,
    exists: bool,
    replace: bool,
    warnings: &mut Vec<AppError>,
) -> Result<SaveStrategy, AppError> {
    if !exists && !replace {
        // Never overwrite: plain rename (fails if something appeared at `target` meanwhile).
        return match sftp.raw.rename(tmp, target).await {
            Ok(_) => Ok(SaveStrategy::Atomic),
            Err(e) => {
                if is_status(&e, StatusCode::Failure)
                    && matches!(sftp.lstat_opt(target).await, Ok(Some(_)))
                {
                    Err(AppError::new(ErrorCode::RemoteExists {
                        path: target.to_owned(),
                    }))
                } else {
                    Err(sftp.map_err(&e, Some(target)))
                }
            }
        };
    }
    if sftp.posix_rename {
        let payload: Vec<u8> = russh_sftp::extensions::HardlinkExtension {
            oldpath: tmp.to_owned(),
            newpath: target.to_owned(),
        }
        .try_into()
        .map_err(|e| AppError::internal(format!("posix-rename payload: {e:?}")))?;
        let reply = sftp
            .raw
            .extended("posix-rename@openssh.com", payload)
            .await
            .map_err(|e| sftp.map_err(&e, Some(target)))?;
        return match reply {
            russh_sftp::protocol::Packet::Status(s) if s.status_code == StatusCode::Ok => {
                Ok(SaveStrategy::Atomic)
            }
            russh_sftp::protocol::Packet::Status(s) => {
                Err(sftp.map_err(&SftpError::Status(s), Some(target)))
            }
            _ => Err(sftp.map_err(&SftpError::UnexpectedPacket, Some(target))),
        };
    }
    if !exists {
        sftp.raw
            .rename(tmp, target)
            .await
            .map_err(|e| sftp.map_err(&e, Some(target)))?;
        return Ok(SaveStrategy::Atomic);
    }
    // RenameFallback: target → bak, tmp → target, remove bak (restore bak on failure). The
    // backup name is random; plain `rename` never overwrites, so an existing name fails
    // safely instead of clobbering anything.
    let bak = bak_path(target);
    sftp.raw
        .rename(target, bak.as_str())
        .await
        .map_err(|e| sftp.map_err(&e, Some(target)))?;
    if let Err(e) = sftp.raw.rename(tmp, target).await {
        if let Err(re) = sftp.raw.rename(bak.as_str(), target).await {
            tracing::warn!(error = %re, "could not restore the original file from its backup");
        }
        return Err(sftp.map_err(&e, Some(target)));
    }
    if let Err(e) = sftp.raw.remove(bak.as_str()).await {
        warnings.push(sftp.map_err(&e, Some(&bak)));
    }
    Ok(SaveStrategy::RenameFallback)
}

/// What [`write_file`] writes to.
#[derive(Clone, Copy)]
pub struct WriteSpec<'a> {
    /// Already resolved by [`resolve_target`].
    pub target: &'a str,
    /// The target's current attributes (`None` = new file, or an entry that is replaced as a
    /// whole, see `exists`). Only regular files can be written into.
    pub cur: Option<&'a FileAttributes>,
    /// Something exists at `target` (with `cur: None`: an entry — e.g. a symlink — that is
    /// replaced by the new file instead of being written through).
    pub exists: bool,
    /// Mode of a new file.
    pub new_mode: u32,
    /// A new file may replace something that appears at `target` meanwhile.
    pub replace: bool,
    /// `(atime, mtime)` set on the open handle
    /// after the data, before close. A refusal is ignored.
    pub times: Option<(u32, u32)>,
    /// Right before the atomic rename, `lstat` the target again and refuse with
    /// `remoteChanged` (temp file removed) unless it is still the file `cur` describes. Saves
    /// without `force` set it; uploads replace on purpose and do not.
    pub recheck: bool,
}

/// Set `(atime, mtime)` through the open handle (best effort: some servers refuse `utimes`).
async fn set_times_quietly(sftp: &Sftp, handle: &str, times: Option<(u32, u32)>) {
    let Some((atime, mtime)) = times else { return };
    let attrs = FileAttributes {
        atime: Some(atime),
        mtime: Some(mtime),
        ..FileAttributes::empty()
    };
    if let Err(e) = sftp.raw.fsetstat(handle, attrs).await {
        tracing::info!(error = %e, "the server did not accept the file times");
    }
}

/// `notARegularFile`: writes go only into regular files.
pub fn not_regular(target: &str) -> AppError {
    AppError::new(ErrorCode::NotARegularFile {
        path: target.to_owned(),
    })
}

/// What [`write_file`] did.
#[derive(Debug, Clone)]
pub struct Written {
    pub strategy: SaveStrategy,
    pub bytes: u64,
    /// `fstat` of the written file before its handle was closed, if the server answered.
    pub attrs: Option<FileAttributes>,
}

/// Write `source` to `spec.target`. New files get `new_mode`; existing files keep their
/// permission bits, owner and group (or are written in place).
pub async fn write_file<S: DataSource>(
    sftp: &Sftp,
    spec: &WriteSpec<'_>,
    source: &mut S,
    hooks: &mut Hooks<'_>,
    warnings: &mut Vec<AppError>,
) -> Result<Written, AppError> {
    let WriteSpec {
        target,
        cur,
        exists,
        new_mode,
        replace,
        times,
        recheck,
    } = *spec;
    match cur.map(kind_of) {
        None | Some(FileKind::File) => {}
        Some(FileKind::Dir) => {
            return Err(AppError::new(ErrorCode::IsADirectory {
                path: target.to_owned(),
            }));
        }
        Some(_) => return Err(not_regular(target)),
    }
    let perms = cur.and_then(mode_bits).unwrap_or(new_mode);
    let (tmp, handle) = match create_temp(sftp, target, perms).await? {
        Temp::Created { path, handle } => (path, handle),
        Temp::InPlace => {
            let Some(c) = cur else {
                // Nothing to write in place: the directory is not writable.
                return Err(AppError::new(ErrorCode::PermissionDenied {
                    path: path::parent(target).to_owned(),
                }));
            };
            let (bytes, attrs) = write_in_place(sftp, target, c, source, hooks, times).await?;
            return Ok(Written {
                strategy: SaveStrategy::InPlace,
                bytes,
                attrs,
            });
        }
    };

    if let Some(c) = cur {
        let keep = match keep_ownership(sftp, &handle, c, perms).await {
            Ok(k) => k,
            Err(e) => {
                sftp.close_quietly(handle).await;
                remove_quietly(sftp, &tmp).await;
                return Err(e);
            }
        };
        if !keep {
            sftp.close_quietly(handle).await;
            remove_quietly(sftp, &tmp).await;
            let (bytes, attrs) = write_in_place(sftp, target, c, source, hooks, times).await?;
            return Ok(Written {
                strategy: SaveStrategy::InPlace,
                bytes,
                attrs,
            });
        }
    }

    let written = async {
        let n = pump(sftp, &handle, target, source, hooks).await?;
        // The server applied its umask at open: set the exact bits before close.
        let mode = FileAttributes {
            permissions: Some(perms),
            ..FileAttributes::empty()
        };
        sftp.raw
            .fsetstat(handle.as_str(), mode)
            .await
            .map_err(|e| sftp.map_err(&e, Some(target)))?;
        set_times_quietly(sftp, &handle, times).await;
        fsync_if_supported(sftp, &handle, target).await?;
        // A rename keeps size and mtime, so these are the target's attributes afterwards.
        Ok::<_, AppError>((n, fstat_quietly(sftp, &handle).await))
    }
    .await;
    let give_up = written.is_err() && stopping(sftp, hooks);
    let closed = close_after(sftp, &handle, give_up).await;
    let outcome = match (written, closed) {
        (Ok(n), Ok(())) => hooks.check().map(|()| n),
        (Err(e), _) => Err(e),
        (Ok(_), Err(e)) => Err(sftp.map_err(&e, Some(target))),
    };
    let (bytes, attrs) = match outcome {
        Ok(n) => n,
        Err(e) => {
            let stop = stopping(sftp, hooks);
            remove_after(sftp, stop, &tmp).await;
            return Err(e);
        }
    };
    if let Some(hook) = &sftp.commit_hook {
        hook();
    }
    if recheck
        && let Some(c) = cur
        && let Err(e) = still_same(sftp, target, c).await
    {
        remove_quietly(sftp, &tmp).await;
        return Err(e);
    }
    match commit(
        sftp,
        &tmp,
        target,
        exists || cur.is_some(),
        replace,
        warnings,
    )
    .await
    {
        Ok(strategy) => Ok(Written {
            strategy,
            bytes,
            attrs,
        }),
        Err(e) => {
            remove_quietly(sftp, &tmp).await;
            Err(e)
        }
    }
}

/// `sftp_write_text` (§6.4): conflict check against `expected`, encode, write, new etag.
pub async fn save_text(sftp: &Sftp, req: &SaveRequest, limit: u64) -> Result<SaveResult, AppError> {
    // Note: the editor's size limit applies to saves too (checked before and after
    // encoding; UTF-16 can double the size).
    let too_large = |size: usize| {
        AppError::new(ErrorCode::FileTooLarge {
            path: req.path.clone(),
            size: u64::try_from(size).unwrap_or(u64::MAX),
            limit,
        })
    };
    if u64::try_from(req.text.len()).unwrap_or(u64::MAX) > limit {
        return Err(too_large(req.text.len()));
    }
    let (target, cur) = resolve_target(sftp, &req.path).await?;
    // Never read (conflict check) or write a FIFO, socket or device.
    if let Some(c) = &cur {
        ensure_regular(c, &req.path)?;
    }
    // Tell the UI where a forced save would write when that is not `path` itself.
    let shown_target = (target != req.path).then(|| target.clone());
    let remote_changed = |size: u64, mtime: i64| {
        AppError::new(ErrorCode::RemoteChanged {
            path: req.path.clone(),
            size,
            mtime,
            target: shown_target.clone(),
        })
    };
    let cur_size_mtime = |c: &FileAttributes| (c.size.unwrap_or(0), c.mtime.map_or(0, i64::from));
    if !req.force {
        // The path resolves to another file than the one the buffer was read from.
        // A buffer read from a regular file (`resolvedPath: null`) may omit
        // `expectedTarget`; it then expects `path` itself, so a regular file swapped for a
        // symlink is a conflict whatever the frontend sends.
        let expected_target = req
            .expected_target
            .as_deref()
            .or_else(|| req.expected.is_some().then_some(req.path.as_str()));
        if let Some(expected_target) = expected_target
            && expected_target != target
        {
            let (size, mtime) = cur.as_ref().map(cur_size_mtime).unwrap_or((0, 0));
            return Err(remote_changed(size, mtime));
        }
        match (&cur, &req.expected) {
            (None, Some(_)) => return Err(remote_changed(0, 0)),
            // A file exists where the buffer expected none.
            (Some(c), None) => {
                let (size, mtime) = cur_size_mtime(c);
                return Err(remote_changed(size, mtime));
            }
            (Some(c), Some(expected)) => {
                // The content hash decides, also when size and mtime are unchanged
                // (SFTP v3 mtimes have 1 s resolution: a same-second, same-size edit would
                // slip through). A touch (mtime only) or same-content rewrite is not a conflict
                // (§6.4). Only a successful read with another hash is a conflict;
                // other read errors (e.g. `notConnected`) pass through.
                let (size, mtime) = cur_size_mtime(c);
                if size > limit {
                    return Err(remote_changed(size, mtime));
                }
                match sftp.read_capped(&target, limit).await {
                    Ok(raw) if text::sha256_hex(&raw) == expected.sha256 => {}
                    Ok(_) => return Err(remote_changed(size, mtime)),
                    Err(AppError {
                        kind: ErrorCode::RemoteNotFound { .. },
                        ..
                    }) => return Err(remote_changed(0, 0)),
                    Err(e) => return Err(e),
                }
            }
            (None, None) => {}
        }
    }
    let bytes = text::encode(&req.text, &req.encoding, req.has_bom, req.eol)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
        return Err(too_large(bytes.len()));
    }
    // A conflict found while writing (in-place identity check, exclusive create) is reported
    // for the buffer's path, with the target shown.
    let relabel = |e: AppError| match e.kind {
        ErrorCode::RemoteChanged { size, mtime, .. } => remote_changed(size, mtime),
        _ => e,
    };
    let mut warnings = Vec::new();
    let mut progress = |_: u64| {};
    let never = || false;
    let mut hooks = Hooks {
        progress: &mut progress,
        cancelled: &never,
        cancel_signal: None,
        window_pool: None,
    };
    let (strategy, written) = if cur.is_none() {
        // New file (§6.4): CREATE|EXCLUDE, write, close.
        let attrs = write_new_exclusive(sftp, &target, &bytes)
            .await
            .map_err(relabel)?;
        (SaveStrategy::Atomic, attrs)
    } else {
        let w = write_file(
            sftp,
            &WriteSpec {
                target: &target,
                cur: cur.as_ref(),
                exists: true,
                new_mode: NEW_FILE_MODE,
                replace: true,
                times: None,
                recheck: !req.force,
            },
            &mut BytesSource::new(&bytes),
            &mut hooks,
            &mut warnings,
        )
        .await
        .map_err(relabel)?;
        (w.strategy, w.attrs)
    };
    let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    let mtime = match written {
        Some(a) => new_etag_mtime(&a, len),
        // The server did not answer the fstat: a post-save stat, trusted only when its size is
        // what we wrote.
        None => new_etag_mtime(&sftp.stat(&target).await?, len),
    };
    Ok(SaveResult {
        etag: Etag {
            size: len,
            mtime,
            sha256: text::sha256_hex(&bytes),
        },
        strategy,
        warnings,
    })
}

/// The mtime of the new etag. The size is always what we wrote; if the attributes show
/// another size, someone else wrote meanwhile and the mtime is dropped (0), so the next save
/// compares content hashes instead of trusting a size/mtime pair that is not ours.
pub fn new_etag_mtime(attrs: &FileAttributes, written: u64) -> i64 {
    match (attrs.size, attrs.mtime) {
        (Some(size), Some(mtime)) if size == written => i64::from(mtime),
        (None, Some(mtime)) => i64::from(mtime),
        _ => 0,
    }
}

/// A save of a file that does not exist (yet): exclusive create, write, close. Returns the
/// `fstat` of the written file before close.
async fn write_new_exclusive(
    sftp: &Sftp,
    target: &str,
    bytes: &[u8],
) -> Result<Option<FileAttributes>, AppError> {
    let attrs = FileAttributes {
        permissions: Some(NEW_FILE_MODE),
        ..FileAttributes::empty()
    };
    let handle = match sftp
        .raw
        .open(
            target,
            OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE,
            attrs,
        )
        .await
    {
        Ok(h) => h.handle,
        Err(e) => {
            if is_status(&e, StatusCode::Failure)
                && matches!(sftp.lstat_opt(target).await, Ok(Some(_)))
            {
                return Err(AppError::new(ErrorCode::RemoteChanged {
                    path: target.to_owned(),
                    size: 0,
                    mtime: 0,
                    target: None,
                }));
            }
            return Err(sftp.map_err(&e, Some(target)));
        }
    };
    let mut progress = |_: u64| {};
    let never = || false;
    let mut hooks = Hooks {
        progress: &mut progress,
        cancelled: &never,
        cancel_signal: None,
        window_pool: None,
    };
    let result = async {
        pump(
            sftp,
            &handle,
            target,
            &mut BytesSource::new(bytes),
            &mut hooks,
        )
        .await?;
        fsync_if_supported(sftp, &handle, target).await?;
        Ok::<_, AppError>(fstat_quietly(sftp, &handle).await)
    }
    .await;
    let closed = sftp.raw.close(handle.as_str()).await;
    let attrs = result?;
    closed.map_err(|e| sftp.map_err(&e, Some(target)))?;
    Ok(attrs)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn temp_names() {
        let t = temp_path("/srv/app/config.yml");
        assert!(t.starts_with("/srv/app/.config.yml.sshb-"), "{t}");
        assert!(t.ends_with(".tmp"));
        let suffix = t
            .trim_start_matches("/srv/app/.config.yml.sshb-")
            .trim_end_matches(".tmp");
        assert_eq!(suffix.len(), 6);
        assert!(
            suffix
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        );
        assert_ne!(temp_path("/a/b"), temp_path("/a/b"));
        let long = format!("/d/{}", "ж".repeat(200)); // 400 bytes
        let t = temp_path(&long);
        assert!(path::basename(&t).len() <= 200 + ".sshb-xxxxxx.tmp".len() + 1);
        assert!(t.starts_with("/d/."));
    }

    /// The rename-fallback backup is unique per save and fits `NAME_MAX` for any target
    /// name (255 bytes, multi-byte), so a stale backup never blocks a save.
    #[test]
    fn backup_names_are_unique_and_capped() {
        let b = bak_path("/srv/app/config.yml");
        assert!(b.starts_with("/srv/app/.config.yml.sshb-"), "{b}");
        assert!(b.ends_with(".bak"));
        assert_ne!(bak_path("/a/b"), bak_path("/a/b"));
        for name in ["x".repeat(255), "ж".repeat(127), "😀".repeat(63)] {
            let b = bak_path(&format!("/d/{name}"));
            assert!(
                path::basename(&b).len() <= 255,
                "{}",
                path::basename(&b).len()
            );
            assert!(b.starts_with("/d/."));
        }
    }

    /// The new etag keeps our size; a size that is not ours drops the mtime.
    #[test]
    fn new_etag_trusts_only_our_size() {
        let a = |size: Option<u64>, mtime: Option<u32>| FileAttributes {
            size,
            mtime,
            ..FileAttributes::empty()
        };
        assert_eq!(new_etag_mtime(&a(Some(5), Some(77)), 5), 77);
        assert_eq!(new_etag_mtime(&a(None, Some(77)), 5), 77);
        assert_eq!(new_etag_mtime(&a(Some(6), Some(77)), 5), 0);
        assert_eq!(new_etag_mtime(&a(Some(5), None), 5), 0);
    }

    /// A step stuck on a stalled server ends as soon as the transfer is cancelled.
    #[tokio::test]
    async fn stalled_steps_end_on_cancel() {
        let (tx, rx) = watch::channel(false);
        let mut signal = Some(rx);
        let stalled = std::future::pending::<Result<(), AppError>>();
        let task = tokio::spawn(async move { or_cancelled(&mut signal, stalled).await });
        tokio::task::yield_now().await;
        tx.send(true).unwrap();
        let r = tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r.unwrap_err().kind, ErrorCode::Cancelled);
        // Without a signal (saves) the step simply runs.
        let mut none = None;
        assert_eq!(or_cancelled(&mut none, async { Ok(3) }).await.unwrap(), 3);
    }

    #[tokio::test]
    async fn bytes_source_chunks() {
        let data = vec![7u8; 10];
        let mut s = BytesSource::new(&data);
        let mut got = Vec::new();
        while let Some(c) = s.next_chunk(4).await.unwrap() {
            got.push(c.len());
        }
        assert_eq!(got, vec![4, 4, 2]);
    }

    /// The in-place identity check accepts only the very file that was resolved.
    #[test]
    fn same_file_requires_identical_regular_file() {
        let resolved = FileAttributes {
            uid: Some(1001),
            gid: Some(1001),
            permissions: Some(0o100_664),
            size: Some(10),
            mtime: Some(1_700_000_000),
            ..FileAttributes::empty()
        };
        assert!(same_file(&resolved.clone(), &resolved));
        let variants = [
            FileAttributes {
                uid: Some(1000),
                ..resolved.clone()
            },
            FileAttributes {
                gid: Some(0),
                ..resolved.clone()
            },
            FileAttributes {
                permissions: Some(0o100_600),
                ..resolved.clone()
            },
            FileAttributes {
                size: Some(11),
                ..resolved.clone()
            },
            FileAttributes {
                mtime: Some(1),
                ..resolved.clone()
            },
            FileAttributes {
                permissions: Some(0o010_664), // FIFO with the same bits
                ..resolved.clone()
            },
        ];
        for v in variants {
            assert!(!same_file(&v, &resolved), "{v:?}");
        }
    }

    /// The gid change always carries the temp file's uid (never a defaulted 0), and
    /// nothing else; without a known uid there is no fsetstat at all.
    #[test]
    fn ownership_attributes_are_paired() {
        let temp = FileAttributes {
            uid: Some(1000),
            gid: Some(1000),
            permissions: Some(0o100_600),
            ..FileAttributes::empty()
        };
        let chown = chown_attrs(&temp, 2000).unwrap();
        assert_eq!((chown.uid, chown.gid), (Some(1000), Some(2000)));
        assert!(chown.size.is_none() && chown.permissions.is_none());
        assert!(chown.atime.is_none() && chown.mtime.is_none());
        let unknown = FileAttributes { uid: None, ..temp };
        assert!(chown_attrs(&unknown, 2000).is_none());
    }
}
