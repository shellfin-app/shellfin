//! Local paths (§9.4): the ONLY way to open local files. `open_local` validates the path for its
//! source, opens it, checks the opened object is a regular file, runs the §7.4.1 trust check on
//! the handle and (Windows, `Config` source) re-checks the final path is not UNC.
//!
//! **Strict allowlist (Windows, `Config` source), applied before any metadata/security/open call:**
//! 1. `Path::components()` starts with `Prefix::Disk`/`VerbatimDisk` followed by `RootDir`
//!    (rejects UNC, `\\.\…`, `\??\…`, `\foo`, `C:foo`, `//srv/…`);
//! 2. no reserved device name (`CON`, `NUL`, `COM1`…, any extension) in any component;
//! 3. `GetDriveTypeW` is not `DRIVE_REMOTE` (mapped network drives) nor `DRIVE_NO_ROOT_DIR`;
//! 4. a symlink/junction as the final component is resolved once (`read_link`) and its target
//!    must pass 1–3; a link to a link is rejected;
//! 5. after opening, `GetFinalPathNameByHandleW` must not start with `\\?\UNC\`.
//!
//! Residual (documented in §9.4): a directory link in an *earlier* component that points to a
//! network location is only detected by step 5, after the OS has followed it.

use std::fs::File;
use std::io::{self, Read};
#[cfg(windows)]
use std::path::{Component, Prefix};
use std::path::{Path, PathBuf};

use super::file_trust::{FileKind, TrustContext, TrustError, check_file};

#[derive(Debug, thiserror::Error)]
pub enum LocalPathError {
    /// The path itself is not allowed (UNC, device, network drive, relative, reserved name, link chain).
    #[error("untrusted path: {0}")]
    UntrustedPath(String),
    #[error("not found")]
    NotFound,
    #[error("permission denied")]
    PermissionDenied,
    #[error("not a regular file")]
    NotAFile,
    #[error("file larger than {0} bytes")]
    TooLarge(u64),
    /// Owner/permission policy of §7.4.1 failed (keys: `insecurePermissions`, configs: `untrustedFile`).
    #[error("insecure permissions: {0}")]
    Insecure(String),
    #[error(transparent)]
    Io(io::Error),
}

impl From<io::Error> for LocalPathError {
    fn from(e: io::Error) -> Self {
        match e.kind() {
            io::ErrorKind::NotFound => Self::NotFound,
            io::ErrorKind::PermissionDenied => Self::PermissionDenied,
            _ => Self::Io(e),
        }
    }
}

impl From<TrustError> for LocalPathError {
    fn from(e: TrustError) -> Self {
        match e {
            TrustError::NotRegularFile => Self::NotAFile,
            TrustError::Untrusted(why) => Self::Insecure(why),
        }
    }
}

const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM0", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
    "COM8", "COM9", "COM¹", "COM²", "COM³", "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6",
    "LPT7", "LPT8", "LPT9", "LPT¹", "LPT²", "LPT³", "CONIN$", "CONOUT$",
];

/// Resolve `.` and `..` lexically, the way Win32 (`GetFullPathNameW`) does before any file-system
/// access: `C:\ProgramData\x\..\ssh\ssh_config` must be recognised as below
/// `%ProgramData%\ssh` by the system-location precheck. `..` never climbs above the root.
#[cfg(windows)]
fn lexically_normalized(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    let mut depth = 0usize;
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if depth > 0 {
                    out.pop();
                    depth -= 1;
                }
            }
            Component::Normal(name) => {
                out.push(name);
                depth += 1;
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `true` for Windows reserved device names (`NUL`, `nul.txt`, `COM1.tar.gz`, `CON .x`, …).
pub fn is_reserved_device_name(component: &str) -> bool {
    // Windows ends a device name at `.` and at `:` (`NUL:`, `COM1:`).
    let stem = component
        .split(['.', ':'])
        .next()
        .unwrap_or(component)
        .trim_end_matches([' ', '.']);
    RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem))
}

/// Step 1–2 of the allowlist (pure syntax; testable on every OS for Windows-shaped input via
/// [`windows_syntax_ok`] on Windows). Returns the drive letter.
#[cfg(windows)]
fn windows_syntax_ok(path: &Path) -> Result<u8, String> {
    let mut comps = path.components();
    let letter = match comps.next() {
        Some(Component::Prefix(p)) => match p.kind() {
            Prefix::Disk(l) | Prefix::VerbatimDisk(l) => l,
            other => return Err(format!("path prefix {other:?} is not a local drive")),
        },
        _ => return Err("path does not start with a drive letter".into()),
    };
    if comps.next() != Some(Component::RootDir) {
        return Err("drive-relative path".into());
    }
    for c in comps {
        if let Component::Normal(name) = c {
            let name = name.to_string_lossy();
            if is_reserved_device_name(&name) {
                return Err(format!("reserved device name {name:?}"));
            }
            // No `:` inside a component (alternate data streams, `NUL:`-style device
            // names) — configs and keys never need it.
            if name.contains(':') {
                return Err(format!("':' in path component {name:?}"));
            }
        }
    }
    Ok(letter.to_ascii_uppercase())
}

/// Steps 1–3 (syntax + live drive type).
#[cfg(windows)]
fn windows_path_allowed(path: &Path) -> Result<(), String> {
    let letter = windows_syntax_ok(path)?;
    if !super::win_security::drive_is_local(letter) {
        return Err(format!(
            "drive {}: is a network or missing drive",
            char::from(letter)
        ));
    }
    Ok(())
}

/// `true` if `path` passes the source allowlist without touching the file system beyond the drive
/// type (used for Include patterns before globbing, §7.4).
pub fn config_path_allowed(path: &Path) -> bool {
    #[cfg(windows)]
    {
        windows_path_allowed(path).is_ok()
    }
    #[cfg(not(windows))]
    {
        path.is_absolute()
    }
}

/// Validate a `Config`-source path (§9.4) and return the path to open. On Windows a symlink in
/// the final component is resolved once and its target validated.
pub fn validate_config_path(path: &Path) -> Result<PathBuf, LocalPathError> {
    #[cfg(windows)]
    {
        windows_path_allowed(path).map_err(LocalPathError::UntrustedPath)?;
        let meta = std::fs::symlink_metadata(path)?;
        if meta.file_type().is_symlink() {
            let target = std::fs::read_link(path)?;
            let target = if target.is_absolute() {
                target
            } else {
                path.parent().map(|p| p.join(&target)).unwrap_or(target)
            };
            windows_path_allowed(&target)
                .map_err(|why| LocalPathError::UntrustedPath(format!("link target: {why}")))?;
            if std::fs::symlink_metadata(&target)?.file_type().is_symlink() {
                return Err(LocalPathError::UntrustedPath("link to a link".into()));
            }
            return Ok(target);
        }
        Ok(path.to_path_buf())
    }
    #[cfg(not(windows))]
    {
        if !path.is_absolute() {
            return Err(LocalPathError::UntrustedPath("relative path".into()));
        }
        Ok(path.to_path_buf())
    }
}

fn open_raw(path: &Path) -> io::Result<File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // FIFOs cannot block the open; symlinks are followed (dotfile setups), fstat judges the target.
        opts.custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    opts.open(path)
}

/// `%ProgramData%\ssh` is user-creatable on machines without sshd, so it could be a
/// directory symlink to `\\attacker\share` (Developer Mode) or a planted directory holding an
/// oplocked file. Before ANY file-system access below it, inspect every directory from the root
/// down to the file's parent without following reparse points (`symlink_metadata`): each must be
/// a plain directory (no reparse point) owned by SYSTEM/Administrators/TrustedInstaller with no
/// foreign write rights. Its parent `C:\ProgramData` is admin-owned, so the probe of the root
/// itself cannot be redirected.
#[cfg(windows)]
fn system_location_precheck(path: &Path, ctx: &TrustContext) -> Result<(), LocalPathError> {
    use std::os::windows::fs::MetadataExt;

    use super::win_security::{Policy, evaluate, security_of_path};
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

    let Some(root) = ctx.program_data_ssh.as_deref() else {
        return Ok(());
    };
    let Some(chain) = super::file_trust::system_chain(path, root) else {
        return Ok(()); // not below %ProgramData%\ssh
    };
    let Some(user) = ctx.user_sid.as_deref() else {
        return Err(LocalPathError::UntrustedPath(
            "current user SID unknown".into(),
        ));
    };
    for dir in chain {
        let meta = match std::fs::symlink_metadata(&dir) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(LocalPathError::NotFound),
            Err(e) => {
                return Err(LocalPathError::UntrustedPath(format!(
                    "{}: {e}",
                    dir.display()
                )));
            }
        };
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 || !meta.is_dir() {
            return Err(LocalPathError::UntrustedPath(format!(
                "{} is a reparse point or not a plain directory",
                dir.display()
            )));
        }
        let info = security_of_path(&dir)
            .map_err(|e| LocalPathError::UntrustedPath(format!("{}: {e}", dir.display())))?;
        evaluate(&info, Policy::SystemOnly, user)
            .map_err(|why| LocalPathError::UntrustedPath(format!("{}: {why}", dir.display())))?;
    }
    Ok(())
}

/// Open a local file for `kind` (validation → open → regular file → §7.4.1 → final-path check).
pub fn open_local(path: &Path, kind: FileKind) -> Result<File, LocalPathError> {
    open_local_with(path, kind, &TrustContext::current())
}

/// [`open_local`] with an explicit trust context (tests).
pub fn open_local_with(
    path: &Path,
    kind: FileKind,
    ctx: &TrustContext,
) -> Result<File, LocalPathError> {
    // Every check below, and the open, use the normalised path (what Win32 opens).
    #[cfg(windows)]
    let normalized = lexically_normalized(path);
    #[cfg(windows)]
    let path = if kind.is_config_source() {
        system_location_precheck(&normalized, ctx)?;
        normalized.as_path()
    } else {
        path
    };
    let to_open = if kind.is_config_source() {
        validate_config_path(path)?
    } else {
        path.to_path_buf()
    };
    let file = open_raw(&to_open)?;
    #[cfg(windows)]
    if kind.is_config_source() {
        let fp = super::win_security::final_path(&file)?;
        if fp
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with(r"\\?\UNC\")
        {
            return Err(LocalPathError::UntrustedPath(
                "resolves to a network path".into(),
            ));
        }
    }
    check_file(&file, kind, ctx)?;
    Ok(file)
}

/// Open and read a small file completely, enforcing `kind.max_bytes()`.
pub fn read_local(path: &Path, kind: FileKind) -> Result<Vec<u8>, LocalPathError> {
    read_local_with(path, kind, &TrustContext::current())
}

pub fn read_local_with(
    path: &Path,
    kind: FileKind,
    ctx: &TrustContext,
) -> Result<Vec<u8>, LocalPathError> {
    let file = open_local_with(path, kind, ctx)?;
    let cap = kind.max_bytes().unwrap_or(u64::MAX);
    // Pre-size to the file length (bounded by the cap) so `read_to_end` does not reallocate and
    // leave freed, unzeroized partial copies of key material behind.
    let len = file.metadata().map(|m| m.len()).unwrap_or(0).min(cap);
    let mut buf = Vec::with_capacity(usize::try_from(len).unwrap_or(0).saturating_add(1));
    file.take(cap.saturating_add(1)).read_to_end(&mut buf)?;
    if u64::try_from(buf.len()).unwrap_or(u64::MAX) > cap {
        return Err(LocalPathError::TooLarge(cap));
    }
    Ok(buf)
}

/// §5.5 agent endpoint grammar (pure, every OS): `\\.\pipe\<name>` with the prefix compared
/// case-insensitively, `<name>` non-empty, without `\`, `/`, NUL, and not `.`/`..`. Win32
/// normalises `.`/`..` inside `\\.\` paths, so a prefix check alone is escapable
/// (`\\.\pipe\..\UNC\host\share\x` reaches SMB).
pub fn valid_agent_pipe_name(endpoint: &str) -> bool {
    const PREFIX: &str = r"\\.\pipe\";
    if endpoint.len() <= PREFIX.len() || !endpoint.is_char_boundary(PREFIX.len()) {
        return false;
    }
    let (prefix, name) = endpoint.split_at(PREFIX.len());
    prefix.eq_ignore_ascii_case(PREFIX)
        && !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['\\', '/', '\0'])
        // Win32 strips trailing dots/spaces, so `openssh-ssh-agent.` would open the same pipe
        // while being classified differently.
        && !name.ends_with(['.', ' '])
}

/// Canonical `\\.\pipe\<name>` for an agent endpoint, or `None` if it fails the grammar.
/// Besides the exact `\\.\pipe\<name>` form, the forward-slash form `//./pipe/<name>` is accepted
/// (same exact-prefix rule, same `<name>` restrictions): OpenSSH's config lexer collapses `\\` to
/// `\`, so `pageant --openssh-config` and Win32-OpenSSH users write `IdentityAgent //./pipe/…`.
pub fn normalize_agent_pipe(endpoint: &str) -> Option<String> {
    const SLASH_PREFIX: &str = "//./pipe/";
    if endpoint.len() > SLASH_PREFIX.len()
        && endpoint.is_char_boundary(SLASH_PREFIX.len())
        && endpoint[..SLASH_PREFIX.len()].eq_ignore_ascii_case(SLASH_PREFIX)
    {
        let candidate = format!(r"\\.\pipe\{}", &endpoint[SLASH_PREFIX.len()..]);
        return valid_agent_pipe_name(&candidate).then_some(candidate);
    }
    valid_agent_pipe_name(endpoint).then(|| endpoint.to_owned())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// Bookmark import files come from the native dialog (`OsChosen`): any regular file is
    /// readable up to the 5 MiB cap; directories and oversized files are refused.
    #[test]
    fn import_source_is_capped_and_regular_files_only() {
        let dir = crate::platform::test_support::private_tempdir();
        let ok = dir.path().join("b.json");
        std::fs::write(&ok, b"{}").unwrap();
        assert_eq!(read_local(&ok, FileKind::ImportSource).unwrap(), b"{}");

        let cap = usize::try_from(crate::platform::file_trust::IMPORT_MAX_BYTES).unwrap();
        let exact = dir.path().join("exact.json");
        std::fs::write(&exact, vec![b' '; cap]).unwrap();
        assert_eq!(
            read_local(&exact, FileKind::ImportSource).unwrap().len(),
            cap
        );
        let big = dir.path().join("big.json");
        std::fs::write(&big, vec![b' '; cap + 1]).unwrap();
        assert!(matches!(
            read_local(&big, FileKind::ImportSource),
            Err(LocalPathError::TooLarge(_))
        ));
        assert!(matches!(
            read_local(dir.path(), FileKind::ImportSource),
            Err(LocalPathError::NotAFile
                | LocalPathError::PermissionDenied
                | LocalPathError::Io(_))
        ));
        assert!(matches!(
            read_local(&dir.path().join("missing.json"), FileKind::ImportSource),
            Err(LocalPathError::NotFound)
        ));
    }

    #[test]
    fn reserved_names() {
        for r in [
            "NUL",
            "nul",
            "NUL.txt",
            "con",
            "COM1.tar.gz",
            "lpt9",
            "CON .x",
            "aux.",
            "CONOUT$",
            "NUL:",
            "com1:",
            "LPT1:x",
            "COM0",
            "lpt0.txt",
            "Com¹",
        ] {
            assert!(is_reserved_device_name(r), "{r}");
        }
        for ok in [
            "config",
            "known_hosts",
            "nul1",
            "com10",
            "console",
            "id_ed25519",
        ] {
            assert!(!is_reserved_device_name(ok), "{ok}");
        }
    }

    /// `.`/`..` are resolved before the `%ProgramData%\ssh` precheck, as Win32 does.
    #[cfg(windows)]
    #[test]
    fn dot_segments_are_normalised_before_the_system_precheck() {
        let root = Path::new(r"C:\ProgramData\ssh");
        for (raw, want) in [
            (
                r"C:\ProgramData\x\..\ssh\ssh_config",
                r"C:\ProgramData\ssh\ssh_config",
            ),
            (
                r"C:\ProgramData\.\ssh\.\conf.d\a.conf",
                r"C:\ProgramData\ssh\conf.d\a.conf",
            ),
            (r"C:\..\..\ProgramData\ssh\x", r"C:\ProgramData\ssh\x"),
            (r"C:\Users\a\.ssh\config", r"C:\Users\a\.ssh\config"),
        ] {
            assert_eq!(
                lexically_normalized(Path::new(raw)),
                Path::new(want),
                "{raw}"
            );
        }
        let n = lexically_normalized(Path::new(r"C:\ProgramData\x\..\ssh\ssh_config"));
        assert!(super::super::file_trust::system_chain(&n, root).is_some());
        // Climbing out of the system directory is not "below" it.
        let n = lexically_normalized(Path::new(r"C:\ProgramData\ssh\..\evil\ssh_config"));
        assert!(super::super::file_trust::system_chain(&n, root).is_none());
    }

    #[test]
    fn agent_pipe_grammar() {
        for ok in [
            r"\\.\pipe\openssh-ssh-agent",
            r"\\.\PIPE\pageant.user.abc",
            r"\\.\Pipe\x",
        ] {
            assert!(valid_agent_pipe_name(ok), "{ok}");
        }
        for bad in [
            r"\\.\pipe\..\UNC\h\s\x",
            r"\\.\pipe\..\C:\x",
            r"\\.\PIPE\..\GLOBALROOT\??\C:\x",
            r"\\.\pipe\../C:/x",
            r"\\.\pipe\a/../..\x",
            r"\\.\pipe\.",
            r"\\.\pipe\..",
            r"\\.\pipe\",
            r"\\host\pipe\x",
            r"\??\pipe\x",
            "\\\\.\\pipe\\a\0b",
            r"\\.\pipex",
            r"\\.\pipe\openssh-ssh-agent.",
            r"\\.\pipe\openssh-ssh-agent ",
            r"\\.\pipe\. .",
            "",
        ] {
            assert!(!valid_agent_pipe_name(bad), "{bad:?}");
            assert_eq!(normalize_agent_pipe(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn agent_pipe_forward_slash_form() {
        assert_eq!(
            normalize_agent_pipe("//./pipe/pageant.alice.abc").as_deref(),
            Some(r"\\.\pipe\pageant.alice.abc")
        );
        assert_eq!(
            normalize_agent_pipe(r"\\.\pipe\openssh-ssh-agent").as_deref(),
            Some(r"\\.\pipe\openssh-ssh-agent")
        );
        for bad in [
            "//./pipe/../UNC/h/s/x",
            "//./pipe/a/../x",
            r"//./pipe/a\..\x",
            "//./pipe/..",
            "//./pipe/",
            "//host/pipe/x",
            "//?/pipe/x",
            "/./pipe/x",
        ] {
            assert_eq!(normalize_agent_pipe(bad), None, "{bad:?}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_allowlist_syntax() {
        for bad in [
            r"\??\UNC\host\share\id",
            r"\??\C:\Users\alice\.ssh\id",
            r"C:foo",
            r"\foo",
            r"\\srv\s\id",
            r"//srv/s/id",
            r"\\.\x",
            r"\\.\pipe\x",
            r"\\?\UNC\srv\s\id",
            r"C:\Users\alice\.ssh\NUL.txt",
            r"C:\CON\x",
            r"relative\path",
            r"C:\x\NUL:",
            r"C:\x\COM1:",
            r"C:\x\config:stream",
            r"C:\x\a:b\c",
        ] {
            assert!(windows_syntax_ok(Path::new(bad)).is_err(), "{bad}");
        }
        assert_eq!(
            windows_syntax_ok(Path::new(r"C:\Users\alice\.ssh\config")),
            Ok(b'C')
        );
        assert_eq!(
            windows_syntax_ok(Path::new(r"\\?\c:\Users\alice\.ssh\config")),
            Ok(b'C')
        );
    }

    #[cfg(windows)]
    #[test]
    fn unc_and_device_paths_never_touch_the_file_system() {
        // Rejected by syntax before any metadata/open call (no SMB connection is attempted).
        for p in [
            r"\\nonexistent-host.invalid\share\id",
            r"\??\UNC\nonexistent-host.invalid\s\id",
        ] {
            assert!(
                matches!(
                    validate_config_path(Path::new(p)),
                    Err(LocalPathError::UntrustedPath(_))
                ),
                "{p}"
            );
            assert!(!config_path_allowed(Path::new(p)));
        }
    }

    #[cfg(windows)]
    #[test]
    fn symlink_to_unc_target_rejected() {
        let dir = crate::platform::test_support::private_tempdir();
        let link = dir.path().join("id_link");
        // Creating symlinks needs Developer Mode or admin; skip silently if unavailable.
        if std::os::windows::fs::symlink_file(r"\\nonexistent-host.invalid\share\id", &link)
            .is_err()
        {
            eprintln!("symlink creation not permitted; skipping");
            return;
        }
        assert!(matches!(
            validate_config_path(&link),
            Err(LocalPathError::UntrustedPath(_))
        ));
    }

    fn write(dir: &Path, name: &str, data: &[u8]) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, data).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        p
    }

    #[test]
    fn reads_regular_file_with_cap() {
        let dir = crate::platform::test_support::private_tempdir();
        let ok = write(dir.path(), "config", b"Host x\n");
        assert_eq!(read_local(&ok, FileKind::UserConfig).unwrap(), b"Host x\n");
        let big = write(dir.path(), "id_big", &vec![b'a'; 64 * 1024 + 1]);
        assert!(matches!(
            read_local(&big, FileKind::PrivateKey),
            Err(LocalPathError::TooLarge(_))
        ));
        let exact = write(dir.path(), "id_exact", &vec![b'a'; 64 * 1024]);
        assert_eq!(
            read_local(&exact, FileKind::PrivateKey).unwrap().len(),
            64 * 1024
        );
        assert!(matches!(
            read_local(&dir.path().join("missing"), FileKind::UserConfig),
            Err(LocalPathError::NotFound)
        ));
        // A directory is not a regular file.
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let err = read_local(&sub, FileKind::UserConfig).unwrap_err();
        assert!(
            matches!(
                err,
                LocalPathError::NotAFile | LocalPathError::PermissionDenied
            ),
            "{err:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn fifo_does_not_block_and_is_rejected() {
        let dir = crate::platform::test_support::private_tempdir();
        let fifo = dir.path().join("fifo");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert!(matches!(
            read_local(&fifo, FileKind::UserConfig),
            Err(LocalPathError::NotAFile)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn unix_permissions_enforced_and_symlink_judged_by_target() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::platform::test_support::private_tempdir();
        let key = write(dir.path(), "id", b"k");
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o640)).unwrap();
        assert!(matches!(
            read_local(&key, FileKind::PrivateKey),
            Err(LocalPathError::Insecure(_))
        ));
        let cfg = write(dir.path(), "real_config", b"Host x\n");
        std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o664)).unwrap();
        let link = dir.path().join("config");
        std::os::unix::fs::symlink(&cfg, &link).unwrap();
        assert!(matches!(
            read_local(&link, FileKind::UserConfig),
            Err(LocalPathError::Insecure(_))
        ));
        std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_local(&link, FileKind::UserConfig).is_ok());
        assert!(matches!(
            validate_config_path(Path::new("rel/config")),
            Err(LocalPathError::UntrustedPath(_))
        ));
    }

    #[cfg(windows)]
    mod windows_acl {
        //! Temp files with crafted DACLs (SDDL) must be rejected; NULL DACL ⇒ untrusted.
        use super::super::*;
        use crate::platform::test_support::{private_tempdir, set_dacl};

        #[test]
        fn crafted_acls() {
            let dir = private_tempdir();
            let me = crate::platform::win_security::current_user_sid().unwrap();
            let key = super::write(dir.path(), "id_test", b"k");
            // Owner-only (protected) DACL: accepted for a private key.
            set_dacl(&key, &format!("D:P(A;;FA;;;{me})(A;;FA;;;SY)"));
            assert!(read_local(&key, FileKind::PrivateKey).is_ok());
            // Everyone may read ⇒ private key rejected, config accepted.
            set_dacl(&key, &format!("D:P(A;;FA;;;{me})(A;;FR;;;WD)"));
            assert!(matches!(
                read_local(&key, FileKind::PrivateKey),
                Err(LocalPathError::Insecure(_))
            ));
            assert!(read_local(&key, FileKind::UserConfig).is_ok());
            // Authenticated Users may write ⇒ config rejected (and read stays possible for us).
            set_dacl(&key, &format!("D:P(A;;FA;;;{me})(A;;FW;;;AU)"));
            assert!(matches!(
                read_local(&key, FileKind::UserConfig),
                Err(LocalPathError::Insecure(_))
            ));
            // NULL DACL ⇒ untrusted for every row with a DACL check.
            set_dacl(&key, "D:NO_ACCESS_CONTROL");
            assert!(matches!(
                read_local(&key, FileKind::UserKnownHosts),
                Err(LocalPathError::Insecure(_))
            ));
            assert!(read_local(&key, FileKind::PublicKey).is_ok()); // owner-only row
            set_dacl(&key, &format!("D:P(A;;FA;;;{me})"));
        }

        #[test]
        fn programdata_planted_by_non_admin_is_ignored() {
            // Simulate %ProgramData%\ssh with a directory created by us (a non-admin from the
            // system row's point of view): the system row must reject files below it.
            let dir = crate::platform::test_support::private_tempdir();
            let root = dir.path().join("ssh");
            std::fs::create_dir(&root).unwrap();
            let cfg = super::write(&root, "ssh_config", b"Host *\n");
            let ctx = TrustContext {
                user_sid: crate::platform::win_security::current_user_sid().ok(),
                program_data_ssh: Some(root.clone()),
            };
            // Rejected by the no-follow precheck, before the file is opened.
            assert!(matches!(
                read_local_with(&cfg, FileKind::SystemConfig, &ctx),
                Err(LocalPathError::UntrustedPath(_))
            ));
            // The same file outside the system location is a normal user file.
            let ctx_other = TrustContext {
                program_data_ssh: Some(dir.path().join("elsewhere")),
                ..ctx
            };
            assert!(read_local_with(&cfg, FileKind::UserConfig, &ctx_other).is_ok());
        }

        /// `%ProgramData%\ssh` as a junction (reparse point) is never followed, even if
        /// its target would look fine.
        #[test]
        #[allow(clippy::disallowed_methods)] // test-only: `mklink /J` needs cmd.exe
        fn programdata_reparse_point_is_never_followed() {
            let dir = crate::platform::test_support::private_tempdir();
            let real = dir.path().join("real");
            std::fs::create_dir(&real).unwrap();
            super::write(&real, "ssh_config", b"Host *\n");
            let root = dir.path().join("ssh");
            let st = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(&root)
                .arg(&real)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            assert!(st.success(), "mklink /J failed");
            let ctx = TrustContext {
                user_sid: crate::platform::win_security::current_user_sid().ok(),
                program_data_ssh: Some(root.clone()),
            };
            let err = read_local_with(&root.join("ssh_config"), FileKind::SystemConfig, &ctx)
                .unwrap_err();
            assert!(
                matches!(&err, LocalPathError::UntrustedPath(why) if why.contains("reparse")),
                "{err:?}"
            );
        }
    }
}
