//! Storage file primitives (§7.1, §7.2): atomic replace with private permissions, JSON loading
//! with quarantine of unparsable files.
//!
//! - Writes: serialize → a fresh `.<file>.<8 random chars>.tmp` created exclusively (Unix: mode
//!   0600, `O_NOFOLLOW`) → `sync_all` → `rename` over the target; the temp file is removed on
//!   error. A unique name means concurrent writers never share a temp file, and an unrelated
//!   `<file>.tmp` next to a user-chosen export target is never touched.
//! - A leading UTF-8 BOM is ignored when reading.
//! - Unparsable files are moved aside as `<stem>.corrupt-<unix ms>.json` (never deleted).
//! - Warnings shown to the UI carry **file names only** (`settings.json`,
//!   `settings.corrupt-1792500000000.json`), never full paths.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::error::{AppError, ErrorCode, IoOp};

/// Result of [`load_json`].
#[derive(Debug)]
pub enum LoadOutcome {
    /// The file does not exist (first start).
    Missing,
    /// Parsed JSON (not yet validated or migrated).
    Loaded(Value),
    /// The file was unparsable and has been renamed to `quarantined_as`.
    Corrupt {
        quarantined_as: PathBuf,
        reason: String,
    },
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Longest part of the target's file name kept in a temp name (the rest is `.` + 8 random
/// characters + `.tmp`), so the temp name stays within the usual 255-byte limit.
const TMP_BASE_MAX_BYTES: usize = 200;
/// Attempts to find a free temp name (a collision of 8 random characters is not expected).
const TMP_ATTEMPTS: usize = 8;

/// A fresh `.<file name>.<8 random chars>.tmp` next to `path` (never a fixed name, so two
/// writers, or an unrelated user file `<name>.tmp` next to an export, are never touched).
fn tmp_path(path: &Path) -> io::Result<PathBuf> {
    use ring::rand::SecureRandom;
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut cut = name.len().min(TMP_BASE_MAX_BYTES);
    while !name.is_char_boundary(cut) {
        cut -= 1;
    }
    let base = name.get(..cut).unwrap_or_default();
    let mut raw = [0u8; 8];
    ring::rand::SystemRandom::new()
        .fill(&mut raw)
        .map_err(|_| io::Error::other("no secure random source"))?;
    let suffix: String = raw
        .iter()
        .map(|b| {
            char::from(
                ALPHABET
                    .get(usize::from(*b) % ALPHABET.len())
                    .copied()
                    .unwrap_or(b'x'),
            )
        })
        .collect();
    Ok(path.with_file_name(format!(".{base}.{suffix}.tmp")))
}

/// Create a new file for writing with private permissions (Unix 0600, never through a symlink;
/// Windows: inherited ACL of the private app directory, §7.1). Fails if `path` exists.
fn create_private(path: &Path) -> io::Result<fs::File> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = opts.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // The umask can only remove bits; make it exactly 0600.
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

/// A new private temp file next to `path` ([`tmp_path`], exclusive create).
fn create_tmp(path: &Path) -> io::Result<(PathBuf, fs::File)> {
    let mut last = io::Error::new(io::ErrorKind::AlreadyExists, "no free temp name");
    for _ in 0..TMP_ATTEMPTS {
        let tmp = tmp_path(path)?;
        match create_private(&tmp) {
            Ok(f) => return Ok((tmp, f)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = e,
            Err(e) => return Err(e),
        }
    }
    Err(last)
}

/// Atomically replace `path` with `bytes` (unique tmp file + `sync_all` + `rename`).
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let (tmp, file) = create_tmp(path)?;
    let result = (|| {
        let mut file = file;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    #[cfg(unix)]
    if result.is_ok()
        && let Some(dir) = path.parent()
        && let Ok(d) = fs::File::open(dir)
    {
        // Best effort: persist the rename itself.
        let _ = d.sync_all();
    }
    result
}

/// Serialize `value` as pretty JSON and [`atomic_write`] it.
pub fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    atomic_write(path, &bytes)
}

/// `<stem>.<tag>-<ms>.json` next to `path` that does not exist yet.
fn sibling_name(path: &Path, tag: &str) -> PathBuf {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ms = now_ms();
    let mut candidate = path.with_file_name(format!("{stem}.{tag}-{ms}.json"));
    let mut n = 1u32;
    while candidate.exists() && n < 1000 {
        candidate = path.with_file_name(format!("{stem}.{tag}-{ms}-{n}.json"));
        n += 1;
    }
    candidate
}

/// Move an unparsable file aside: `<stem>.corrupt-<ms>.json`. Returns the new path.
pub fn quarantine(path: &Path) -> io::Result<PathBuf> {
    let target = sibling_name(path, "corrupt");
    fs::rename(path, &target)?;
    Ok(target)
}

/// Keep a private copy of `bytes` (the original file content) as `<stem>.corrupt-<ms>.json`,
/// used when individual invalid entries are dropped on load (§7.2).
pub fn quarantine_copy(path: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
    let target = sibling_name(path, "corrupt");
    atomic_write(&target, bytes)?;
    Ok(target)
}

/// `bytes` without one leading UTF-8 byte order mark (Notepad and PowerShell 5.1 write
/// one; it is not JSON).
pub fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes)
}

/// Largest store file that is read. Far above any legitimate store (5000 bookmarks of a
/// few KiB each); a bigger file is treated like an unparsable one instead of being read whole.
pub const MAX_STORE_BYTES: u64 = 64 * 1024 * 1024;

/// Read and parse a JSON file without changing anything on disk: `Ok(None)` when it does not
/// exist, `Ok(Some(Err(reason)))` when it is not JSON or larger than [`MAX_STORE_BYTES`].
pub fn read_json(path: &Path) -> io::Result<Option<Result<Value, String>>> {
    read_json_capped(path, MAX_STORE_BYTES)
}

fn read_json_capped(path: &Path, cap: u64) -> io::Result<Option<Result<Value, String>>> {
    use std::io::Read;
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let too_large = |n: u64| Ok(Some(Err(format!("file too large ({n} bytes > {cap})"))));
    let len = file.metadata()?.len();
    if len > cap {
        return too_large(len);
    }
    // `take`: a file that grows after the size check is still not read past the cap.
    let mut bytes = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    file.take(cap.saturating_add(1)).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > cap {
        return too_large(bytes.len() as u64);
    }
    Ok(Some(
        serde_json::from_slice::<Value>(strip_bom(&bytes)).map_err(|e| e.to_string()),
    ))
}

/// Read and parse a JSON file. Unparsable content is quarantined (renamed) and reported as
/// [`LoadOutcome::Corrupt`]; I/O errors other than "not found" are returned as `Err`.
pub fn load_json(path: &Path) -> io::Result<LoadOutcome> {
    match read_json(path)? {
        None => Ok(LoadOutcome::Missing),
        Some(Ok(v)) => Ok(LoadOutcome::Loaded(v)),
        Some(Err(reason)) => {
            let quarantined_as = quarantine(path)?;
            Ok(LoadOutcome::Corrupt {
                quarantined_as,
                reason,
            })
        }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `storageCorrupt{file, quarantinedAs}` with file names only.
pub fn corrupt_warning(path: &Path, quarantined_as: &Path, reason: impl Into<String>) -> AppError {
    AppError::new(ErrorCode::StorageCorrupt {
        file: file_name(path),
        quarantined_as: file_name(quarantined_as),
    })
    .with_detail(reason)
}

/// `io{op: readStorage|writeStorage, path}`.
pub fn io_error(op: IoOp, path: &Path, err: &io::Error) -> AppError {
    AppError::io(op, Some(path.display().to_string()), err)
}

/// Write a file the user picked in a native Save dialog (`bookmarks_export`,
/// `app_save_text_file`): [`atomic_write`] (exclusive private temp next to it, then rename). A
/// failure is `io{writeLocalFile, path}` with the **file name only** (§4.4: the chosen path is
/// never returned to the WebView).
pub fn write_chosen_file(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    atomic_write(path, bytes)
        .map_err(|e| AppError::io(IoOp::WriteLocalFile, Some(file_name(path)), &e))
}

/// Create the storage directory if missing (Unix 0700). Normally `platform::harden_process`
/// already did this at startup.
pub fn ensure_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn atomic_write_replaces_and_leaves_no_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("settings.json");
        atomic_write(&p, b"one").unwrap();
        atomic_write(&p, b"two").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"two");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    /// An unrelated `<name>.tmp` next to the target (an export into the user's folder)
    /// is never truncated or renamed away.
    #[test]
    fn atomic_write_leaves_a_foreign_tmp_file_alone() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bookmarks.json");
        let foreign = dir.path().join("bookmarks.json.tmp");
        fs::write(&foreign, b"user data").unwrap();
        atomic_write(&p, b"{}").unwrap();
        assert_eq!(fs::read(&foreign).unwrap(), b"user data");
        assert_eq!(fs::read(&p).unwrap(), b"{}");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    /// Concurrent writers never share a temp file; every write is complete.
    #[test]
    fn concurrent_atomic_writes_do_not_interleave() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("recents.json");
        let writers: Vec<_> = (0..8u8)
            .map(|n| {
                let p = p.clone();
                std::thread::spawn(move || {
                    for _ in 0..20 {
                        atomic_write(&p, &[b'a' + n; 64 * 1024]).unwrap();
                    }
                })
            })
            .collect();
        for w in writers {
            w.join().unwrap();
        }
        let bytes = fs::read(&p).unwrap();
        assert_eq!(bytes.len(), 64 * 1024);
        assert!(bytes.iter().all(|b| *b == bytes[0]), "mixed content");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1, "temp left");
    }

    /// Long target names still give a temp name within 255 bytes (multi-byte names cut on a
    /// character boundary).
    #[test]
    fn temp_names_are_bounded_and_unique() {
        let dir = Path::new("/x");
        let long = "é".repeat(127);
        let a = tmp_path(&dir.join(&long)).unwrap();
        let b = tmp_path(&dir.join(&long)).unwrap();
        assert_ne!(a, b);
        let name = a.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.len() <= 255, "{}", name.len());
        assert!(name.starts_with('.') && name.ends_with(".tmp"), "{name}");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bookmarks.json");
        atomic_write(&p, b"{}").unwrap();
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn create_private_does_not_follow_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim");
        fs::write(&victim, b"keep").unwrap();
        let link = dir.path().join(".x.json.abc.tmp");
        std::os::unix::fs::symlink(&victim, &link).unwrap();
        assert!(create_private(&link).is_err());
        assert_eq!(fs::read(&victim).unwrap(), b"keep");
    }

    /// A failed write to a user-chosen file reports the file name, never the full path.
    #[test]
    fn chosen_file_errors_carry_the_file_name_only() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("missing-dir").join("export.json");
        let e = write_chosen_file(&target, b"{}").unwrap_err();
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["code"], "io");
        assert_eq!(v["params"]["op"], "writeLocalFile");
        assert_eq!(v["params"]["path"], "export.json");
        let parent = dir.path().display().to_string();
        assert!(!serde_json::to_string(&e).unwrap().contains(&parent));
        write_chosen_file(&dir.path().join("ok.json"), b"{}").unwrap();
        assert_eq!(fs::read(dir.path().join("ok.json")).unwrap(), b"{}");
    }

    /// A UTF-8 BOM before the JSON is ignored, not a reason to quarantine the file.
    #[test]
    fn load_json_skips_a_bom() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("settings.json");
        fs::write(&p, b"\xEF\xBB\xBF{\"version\":1}").unwrap();
        assert!(matches!(load_json(&p).unwrap(), LoadOutcome::Loaded(v) if v["version"] == 1));
        // Only one BOM is skipped; two are still corrupt.
        fs::write(&p, b"\xEF\xBB\xBF\xEF\xBB\xBF{}").unwrap();
        assert!(matches!(
            load_json(&p).unwrap(),
            LoadOutcome::Corrupt { .. }
        ));
        assert_eq!(strip_bom(b"{}"), b"{}");
    }

    #[test]
    fn load_json_missing_loaded_and_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("recents.json");
        assert!(matches!(load_json(&p).unwrap(), LoadOutcome::Missing));
        fs::write(&p, br#"{"version":1}"#).unwrap();
        assert!(matches!(load_json(&p).unwrap(), LoadOutcome::Loaded(v) if v["version"] == 1));
        fs::write(&p, b"{not json").unwrap();
        match load_json(&p).unwrap() {
            LoadOutcome::Corrupt { quarantined_as, .. } => {
                assert!(!p.exists());
                let name = quarantined_as
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                assert!(
                    name.starts_with("recents.corrupt-") && name.ends_with(".json"),
                    "{name}"
                );
                assert_eq!(fs::read(&quarantined_as).unwrap(), b"{not json");
                let w = corrupt_warning(&p, &quarantined_as, "x");
                let v = serde_json::to_value(&w).unwrap();
                assert_eq!(v["params"]["file"], "recents.json");
                assert_eq!(v["params"]["quarantinedAs"], name);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    /// A store file over the cap is not read whole; it counts as unparsable
    /// (quarantined by `load_json`, untouched by `read_json`).
    #[test]
    fn oversized_store_files_are_corrupt_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bookmarks.json");
        fs::write(&p, br#"{"version":1,"pad":"0123456789"}"#).unwrap();
        let len = fs::metadata(&p).unwrap().len();
        assert!(matches!(read_json_capped(&p, len).unwrap(), Some(Ok(_))));
        match read_json_capped(&p, len - 1).unwrap() {
            Some(Err(reason)) => assert!(reason.contains("too large"), "{reason}"),
            other => panic!("unexpected {other:?}"),
        }
        assert!(p.exists(), "read_json never changes the disk");
        // The real cap, through load_json: a sparse file just over it is quarantined.
        let big = dir.path().join("recents.json");
        let f = fs::File::create(&big).unwrap();
        f.set_len(MAX_STORE_BYTES + 1).unwrap();
        drop(f);
        match load_json(&big).unwrap() {
            LoadOutcome::Corrupt { reason, .. } => assert!(reason.contains("too large")),
            other => panic!("unexpected {other:?}"),
        }
        assert!(!big.exists());
    }

    #[test]
    fn quarantine_names_do_not_collide() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.json");
        let a = quarantine_copy(&p, b"1").unwrap();
        let b = quarantine_copy(&p, b"2").unwrap();
        assert_ne!(a, b);
        assert_eq!(fs::read(a).unwrap(), b"1");
        assert_eq!(fs::read(b).unwrap(), b"2");
    }
}
