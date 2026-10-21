//! Own known_hosts writer (§5.3 "Writing"). Never uses russh's `learn_known_hosts_path`.
//!
//! - The lookup is validated (`valid_hostname`, or `[host]:port` built from a valid host).
//! - `HashKnownHosts yes` → `|1|base64(salt)|base64(HMAC-SHA1(salt, lookup))`, 20-byte salt.
//! - Unix: missing parent directory created with 0700 (one level only); a **new** file is
//!   created `O_CREAT|O_EXCL|O_NOFOLLOW` with mode 0600; an **existing** file is opened
//!   `O_APPEND|O_NOFOLLOW` and its mode is left unchanged.
//! - Windows: learning only into files under `profile_root` (`%USERPROFILE%`); the handle is
//!   opened with `read(true).append(true)` because `File::lock` needs read access (App. A #29).
//! - Writes are serialized by a process-wide mutex plus an exclusive `File::lock()`; exactly one
//!   line is appended, prefixed with `\n` if the file does not end with one.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path};
use std::sync::{Mutex, PoisonError};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ring::hmac;
use ring::rand::{SecureRandom, SystemRandom};

use super::HostKeyRef;
use crate::sshconfig::validate::valid_hostname;

static LEARN_LOCK: Mutex<()> = Mutex::new(());

const SALT_LEN: usize = 20;

#[derive(Debug, thiserror::Error)]
pub enum LearnError {
    #[error("invalid known_hosts lookup name")]
    InvalidLookup,
    #[error("invalid host key")]
    InvalidKey,
    #[error("known_hosts file is outside the user profile")]
    OutsideProfile,
    #[error("known_hosts path is not a regular file")]
    NotAFile,
    #[error("random number generation failed")]
    Random,
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// `true` for `host` (valid hostname) or `[host]:port` (valid host, port 1–65535).
pub fn valid_lookup(lookup: &str) -> bool {
    if let Some(rest) = lookup.strip_prefix('[') {
        let Some((host, port)) = rest.split_once("]:") else {
            return false;
        };
        let port_ok = !port.is_empty()
            && port.bytes().all(|b| b.is_ascii_digit())
            && port.parse::<u16>().is_ok_and(|p| p != 0);
        return port_ok && valid_hostname(host);
    }
    valid_hostname(lookup)
}

fn valid_key(key: &HostKeyRef<'_>) -> bool {
    !key.key_type.is_empty()
        && key.key_type.chars().all(|c| c.is_ascii_graphic())
        && !key.blob.is_empty()
        && super::blob_key_type(key.blob) == Some(key.key_type)
}

/// Is `file` (absolute, no `.`/`..`) strictly inside `root`? Component-wise; case-insensitive
/// and `\\?\`-prefix-insensitive on Windows. Also decides the learn target in `ssh::connect`.
pub(crate) fn is_under(file: &Path, root: &Path) -> bool {
    if !file.is_absolute()
        || file
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return false;
    }
    if root.components().any(|c| matches!(c, Component::ParentDir)) {
        return false;
    }
    let f = normalized_components(file);
    let r = normalized_components(root);
    f.len() > r.len() && f.starts_with(&r)
}

fn normalized_components(p: &Path) -> Vec<String> {
    p.components()
        .filter(|c| !matches!(c, Component::CurDir))
        .map(|c| match c {
            #[cfg(windows)]
            Component::Prefix(prefix) => match prefix.kind() {
                std::path::Prefix::Disk(l) | std::path::Prefix::VerbatimDisk(l) => {
                    format!("{}:", char::from(l.to_ascii_lowercase()))
                }
                _ => prefix.as_os_str().to_string_lossy().to_lowercase(),
            },
            other => {
                let s = other.as_os_str().to_string_lossy();
                if cfg!(windows) {
                    s.to_lowercase()
                } else {
                    s.into_owned()
                }
            }
        })
        .collect()
}

fn hashed_host(lookup: &str) -> Result<String, LearnError> {
    let mut salt = [0u8; SALT_LEN];
    SystemRandom::new()
        .fill(&mut salt)
        .map_err(|_| LearnError::Random)?;
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, &salt);
    let tag = hmac::sign(&key, lookup.as_bytes());
    Ok(format!(
        "|1|{}|{}",
        STANDARD.encode(salt),
        STANDARD.encode(tag.as_ref())
    ))
}

/// Append one `<lookup-or-hash> <key-type> <base64>` line to `file`.
///
/// `profile_root`: when `Some`, the file must lie inside it (used on Windows with
/// `%USERPROFILE%`). The caller has already validated the path syntax (§9.4) and decided that
/// the file is a writable user file (`canRemember`).
pub fn learn(
    file: &Path,
    lookup: &str,
    key: HostKeyRef<'_>,
    hash: bool,
    profile_root: Option<&Path>,
) -> Result<(), LearnError> {
    if !valid_lookup(lookup) {
        return Err(LearnError::InvalidLookup);
    }
    if !valid_key(&key) {
        return Err(LearnError::InvalidKey);
    }
    if let Some(root) = profile_root
        && !is_under(file, root)
    {
        return Err(LearnError::OutsideProfile);
    }
    let host_field = if hash {
        hashed_host(lookup)?
    } else {
        lookup.to_owned()
    };
    let line = format!(
        "{host_field} {} {}\n",
        key.key_type,
        STANDARD.encode(key.blob)
    );

    let _guard = LEARN_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    ensure_parent_dir(file)?;
    let mut f = open_for_append(file)?;
    if !f.metadata()?.is_file() {
        return Err(LearnError::NotAFile);
    }
    lock_bounded(&f, LOCK_WAIT)?;
    let result = append_line(&mut f, &line);
    let unlock = f.unlock();
    result?;
    unlock?;
    Ok(())
}

/// How long `learn` waits for another process's lock on the file. Learning is not
/// fatal, so giving up is fine; waiting forever would pin a blocking thread while holding the
/// process-wide `LEARN_LOCK`.
pub const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// `File::lock` with a deadline: retry `try_lock` every 20 ms, then fail with `TimedOut`.
fn lock_bounded(f: &File, wait: std::time::Duration) -> io::Result<()> {
    let deadline = std::time::Instant::now() + wait;
    loop {
        match f.try_lock() {
            Ok(()) => return Ok(()),
            Err(std::fs::TryLockError::Error(e)) => return Err(e),
            Err(std::fs::TryLockError::WouldBlock) => {
                if std::time::Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "known_hosts is locked by another process",
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
    }
}

fn append_line(f: &mut File, line: &str) -> Result<(), LearnError> {
    let len = f.metadata()?.len();
    let mut needs_newline = false;
    if len > 0 {
        f.seek(SeekFrom::End(-1))?;
        let mut last = [0u8; 1];
        f.read_exact(&mut last)?;
        needs_newline = last != *b"\n";
    }
    let mut out = Vec::with_capacity(line.len() + 1);
    if needs_newline {
        out.push(b'\n');
    }
    out.extend_from_slice(line.as_bytes());
    // Append mode: the write goes to the end regardless of the read position.
    f.write_all(&out)?;
    f.sync_data()?;
    Ok(())
}

/// Create the immediate parent directory if missing (0700 on Unix). Never creates more than one
/// level (e.g. `~/.ssh`, not `~`).
fn ensure_parent_dir(file: &Path) -> io::Result<()> {
    let Some(parent) = file.parent() else {
        return Ok(());
    };
    if parent.as_os_str().is_empty() || parent.exists() {
        return Ok(());
    }
    #[cfg(unix)]
    let created = {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(parent)
    };
    #[cfg(not(unix))]
    let created = std::fs::DirBuilder::new().create(parent);
    match created {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(unix)]
fn open_for_append(file: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    let created = OpenOptions::new()
        .read(true)
        .append(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(file);
    match created {
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => OpenOptions::new()
            .read(true)
            .append(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(file),
        other => other,
    }
}

#[cfg(not(unix))]
fn open_for_append(file: &Path) -> io::Result<File> {
    // read(true) is required for File::lock on Windows (append-only handles fail, App. A #29).
    OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(file)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use crate::known_hosts::parse::{Origin, parse};
    use crate::known_hosts::test_keys::{b64, ed25519, rsa};
    use crate::known_hosts::verify::{Verdict, verify};
    use crate::known_hosts::{blob_key_type, lookup_name};

    fn key(blob: &[u8]) -> HostKeyRef<'_> {
        HostKeyRef {
            key_type: blob_key_type(blob).unwrap(),
            blob,
        }
    }

    /// A file locked by someone else is waited for a bounded time, then `TimedOut`;
    /// once the lock is gone it succeeds.
    #[test]
    fn file_lock_wait_is_bounded() {
        let dir = crate::platform::test_support::private_tempdir();
        let path = dir.path().join("known_hosts");
        std::fs::write(&path, b"").unwrap();
        let holder = open_for_append(&path).unwrap();
        holder.lock().unwrap();
        let other = open_for_append(&path).unwrap();
        let started = std::time::Instant::now();
        let err = lock_bounded(&other, std::time::Duration::from_millis(150)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() >= std::time::Duration::from_millis(150));
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        holder.unlock().unwrap();
        lock_bounded(&other, std::time::Duration::from_millis(150)).unwrap();
        other.unlock().unwrap();
    }

    fn verify_file(path: &Path, lookup: &str, blob: &[u8]) -> Verdict {
        let text = std::fs::read(path).unwrap();
        let (f, issues) = parse(path, "kh", Origin::User, &text);
        assert!(issues.is_empty(), "{issues:?}");
        verify(lookup, key(blob), &[f])
    }

    #[test]
    fn lookup_validation() {
        for ok in [
            "example.com",
            "[example.com]:2222",
            "[2001:db8::1]:22",
            "myalias",
            "::1",
        ] {
            assert!(valid_lookup(ok), "{ok}");
        }
        for bad in [
            "",
            "x,*",
            "*",
            "a b",
            "[x]:0",
            "[x]:",
            "[x]:99999",
            "[x,*]:22",
            "[x]",
            "[x]:22x",
            "-x",
            "%h",
            "h\n",
        ] {
            assert!(!valid_lookup(bad), "{bad:?}");
        }
        let tmp = tempfile::tempdir().unwrap();
        let k = ed25519(1);
        assert!(matches!(
            learn(&tmp.path().join("kh"), "x,*", key(&k), false, None),
            Err(LearnError::InvalidLookup)
        ));
        assert!(!tmp.path().join("kh").exists());
    }

    #[test]
    fn rejects_inconsistent_key() {
        let tmp = tempfile::tempdir().unwrap();
        let k = ed25519(1);
        let bad = HostKeyRef {
            key_type: "ssh-rsa",
            blob: &k,
        };
        assert!(matches!(
            learn(&tmp.path().join("kh"), "h", bad, false, None),
            Err(LearnError::InvalidKey)
        ));
        let empty = HostKeyRef {
            key_type: "ssh-ed25519",
            blob: &[],
        };
        assert!(matches!(
            learn(&tmp.path().join("kh"), "h", empty, false, None),
            Err(LearnError::InvalidKey)
        ));
    }

    #[test]
    fn plain_line_and_alias_lookup() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("known_hosts");
        let k = ed25519(1);
        let lookup = lookup_name("10.0.0.5", 2222, Some("MyAlias"));
        learn(&path, &lookup, key(&k), false, None).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, format!("myalias ssh-ed25519 {}\n", b64(&k)));
        assert_eq!(
            verify_file(&path, "myalias", &k),
            Verdict::Trusted {
                file: "kh".into(),
                line: 1
            }
        );
    }

    #[test]
    fn host_port_line() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("known_hosts");
        let r = rsa(4);
        learn(&path, "[h.example]:2222", key(&r), false, None).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            format!("[h.example]:2222 ssh-rsa {}\n", b64(&r))
        );
    }

    #[test]
    fn hashed_line_verifies() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("known_hosts");
        let k = ed25519(2);
        learn(&path, "[example.com]:2222", key(&k), true, None).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("|1|"), "{text}");
        assert!(!text.contains("example.com"));
        assert_eq!(text.lines().count(), 1);
        assert_eq!(
            verify_file(&path, "[example.com]:2222", &k),
            Verdict::Trusted {
                file: "kh".into(),
                line: 1
            }
        );
        assert_eq!(
            verify_file(&path, "example.com", &k),
            Verdict::Unknown {
                other_types: vec![]
            }
        );
        // Two hashes of the same lookup use different salts.
        learn(&path, "[example.com]:2222", key(&k), true, None).unwrap();
        let lines: Vec<String> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        assert_ne!(lines[0], lines[1]);
    }

    #[test]
    fn leading_newline_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("known_hosts");
        let (old, new) = (ed25519(1), ed25519(2));
        std::fs::write(&path, format!("old ssh-ed25519 {}", b64(&old))).unwrap();
        learn(&path, "new", key(&new), false, None).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            text,
            format!(
                "old ssh-ed25519 {}\nnew ssh-ed25519 {}\n",
                b64(&old),
                b64(&new)
            )
        );
        // File already ends with '\n' → no extra blank line.
        learn(&path, "third", key(&old), false, None).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 3);
        assert!(!text.contains("\n\n"));
    }

    #[test]
    fn concurrent_learns_produce_intact_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("known_hosts");
        let threads: Vec<_> = (0u8..8)
            .map(|t| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for i in 0u8..25 {
                        let k = ed25519(t.wrapping_mul(31).wrapping_add(i));
                        let lookup = format!("host-{t}-{i}.example");
                        learn(&path, &lookup, key(&k), t % 2 == 0, None).unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let text = std::fs::read(&path).unwrap();
        let (f, issues) = parse(&path, "kh", Origin::User, &text);
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(f.entries.len(), 200);
        assert!(text.ends_with(b"\n"));
        let k = ed25519(3u8.wrapping_mul(31).wrapping_add(7));
        assert!(matches!(
            verify("host-3-7.example", key(&k), &[f]),
            Verdict::Trusted { .. }
        ));
    }

    #[test]
    fn profile_root_confinement() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("profile");
        std::fs::create_dir(&root).unwrap();
        let k = ed25519(1);
        let inside = root.join(".ssh").join("known_hosts");
        learn(&inside, "h", key(&k), false, Some(&root)).unwrap();
        assert!(inside.is_file());

        let outside = tmp.path().join("elsewhere").join("known_hosts");
        assert!(matches!(
            learn(&outside, "h", key(&k), false, Some(&root)),
            Err(LearnError::OutsideProfile)
        ));
        let escape = root.join("..").join("elsewhere").join("known_hosts");
        assert!(matches!(
            learn(&escape, "h", key(&k), false, Some(&root)),
            Err(LearnError::OutsideProfile)
        ));
        assert!(matches!(
            learn(&root, "h", key(&k), false, Some(&root)),
            Err(LearnError::OutsideProfile)
        ));
        assert!(matches!(
            learn(
                Path::new("relative/known_hosts"),
                "h",
                key(&k),
                false,
                Some(&root)
            ),
            Err(LearnError::OutsideProfile)
        ));
        assert!(!tmp.path().join("elsewhere").exists());
    }

    #[test]
    fn only_one_directory_level_is_created() {
        let tmp = tempfile::tempdir().unwrap();
        let deep = tmp.path().join("a").join("b").join("known_hosts");
        let k = ed25519(1);
        assert!(matches!(
            learn(&deep, "h", key(&k), false, None),
            Err(LearnError::Io(_))
        ));
        assert!(!tmp.path().join("a").exists());
    }

    #[test]
    fn directory_target_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("kh");
        std::fs::create_dir(&dir).unwrap();
        let k = ed25519(1);
        assert!(learn(&dir, "h", key(&k), false, None).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_prefix_and_case_insensitive_profile_check() {
        let root = Path::new(r"C:\Users\alice");
        assert!(is_under(
            Path::new(r"c:\USERS\Alice\.ssh\known_hosts"),
            root
        ));
        assert!(is_under(
            Path::new(r"\\?\C:\Users\alice\.ssh\known_hosts"),
            root
        ));
        assert!(!is_under(
            Path::new(r"C:\Users\alice2\.ssh\known_hosts"),
            root
        ));
        assert!(!is_under(
            Path::new(r"D:\Users\alice\.ssh\known_hosts"),
            root
        ));
        assert!(!is_under(
            Path::new(r"\\server\share\alice\known_hosts"),
            root
        ));
    }

    #[cfg(unix)]
    mod unix {
        use std::os::unix::fs::PermissionsExt;

        use super::*;

        fn mode(p: &Path) -> u32 {
            std::fs::metadata(p).unwrap().permissions().mode() & 0o777
        }

        #[test]
        fn new_file_0600_and_dir_0700() {
            let tmp = tempfile::tempdir().unwrap();
            let dir = tmp.path().join(".ssh");
            let path = dir.join("known_hosts");
            learn(&path, "h", key(&ed25519(1)), false, None).unwrap();
            assert_eq!(mode(&dir), 0o700);
            assert_eq!(mode(&path), 0o600);
        }

        #[test]
        fn existing_file_mode_is_left_unchanged() {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("known_hosts");
            std::fs::write(&path, "").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o664)).unwrap();
            learn(&path, "h", key(&ed25519(1)), false, None).unwrap();
            assert_eq!(mode(&path), 0o664);
        }

        #[test]
        fn symlink_is_not_followed() {
            let tmp = tempfile::tempdir().unwrap();
            let target = tmp.path().join("target");
            std::fs::write(&target, "").unwrap();
            let link = tmp.path().join("known_hosts");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            assert!(learn(&link, "h", key(&ed25519(1)), false, None).is_err());
            assert_eq!(std::fs::read(&target).unwrap(), b"");
            // Dangling symlink: O_EXCL|O_NOFOLLOW refuses to create through it.
            let dangling = tmp.path().join("dangling");
            std::os::unix::fs::symlink(tmp.path().join("nowhere"), &dangling).unwrap();
            assert!(learn(&dangling, "h", key(&ed25519(1)), false, None).is_err());
            assert!(!tmp.path().join("nowhere").exists());
        }
    }
}
