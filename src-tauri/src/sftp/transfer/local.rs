//! The local side of a transfer (§6.6, §9.4): the download's `.part` file, provenance mark and
//! final rename, the upload source opened through `local_path`, and the timestamp conversions.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::AsyncReadExt;

use crate::error::{AppError, ErrorCode, IoOp};
use crate::platform::file_trust::FileKind as LocalKind;
use crate::platform::local_path::{LocalPathError, open_local};

use super::super::path;
use super::super::save::DataSource;
use super::super::throttle::Limiter;

/// Local read size for uploads.
const LOCAL_READ: usize = 256 * 1024;
/// `.part` name attempts (`<chosen>.part`, `<chosen>.1.part`, …).
const PART_TRIES: u32 = 100;

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

pub(super) fn local_error(e: &io::Error, path: &Path, op: IoOp) -> AppError {
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
pub(super) fn finish_part_with(
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

/// `true` if something (file, directory, link, even a dangling one) is at `path`.
pub(super) fn local_target_exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

// ---------------------------------------------------------------------------------------------
// upload
// ---------------------------------------------------------------------------------------------

/// A throttle plus the transfer's cancel check (waits abort on cancel).
type Throttle<'a> = (&'a Limiter, &'a (dyn Fn() -> bool + Send + Sync));

/// A local file opened through `local_path` (source `OsChosen`: regular files only, §9.4).
pub(super) struct LocalSource<'a> {
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
    pub(super) fn new(
        file: tokio::fs::File,
        path: PathBuf,
        throttle: Option<Throttle<'a>>,
    ) -> Self {
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
pub(super) fn upload_info(local: &Path) -> (String, Option<u64>) {
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

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
}
