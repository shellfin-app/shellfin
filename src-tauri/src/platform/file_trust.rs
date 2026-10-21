//! File trust checks (§7.4.1), run **on the opened handle** by `local_path::open_local` (no
//! path-based check followed by a separate open). Only the `%ProgramData%\ssh` directory chain is
//! checked by path.
//!
//! | kind | Unix (fstat) | Windows (outside `%ProgramData%\ssh`) |
//! |---|---|---|
//! | user config | owner uid/root, `mode & 022 == 0` | owner ∈ own SIDs, no foreign write ACE |
//! | system config | owner root | (only under `%ProgramData%\ssh`: system row) |
//! | user known_hosts | owner uid/root, not world-writable | owner ∈ own SIDs, no foreign write ACE |
//! | global known_hosts | owner root | system row |
//! | private key | owner uid, `mode & 077 == 0` | owner ∈ own SIDs, no foreign write **or read** ACE |
//! | `.pub` | owner uid/root | owner ∈ own SIDs |
//! | upload source | regular file only | regular file only |
//!
//! Windows files under `%ProgramData%\ssh` (any kind except uploads) use the system row: the
//! file and every directory from `%ProgramData%\ssh` down to it must be owned by SYSTEM,
//! Administrators or TrustedInstaller with no write-type ACE for any other SID.

use std::fs::File;
#[cfg(windows)]
use std::path::Path;
use std::path::PathBuf;

/// What a local file is used for; selects the §7.4.1 row, the path source (§9.4) and the size cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    UserConfig,
    SystemConfig,
    UserKnownHosts,
    GlobalKnownHosts,
    PrivateKey,
    PublicKey,
    /// Upload source from the native dialog or an OS drop (`OsChosen`, §9.4).
    UploadSource,
    /// Bookmark import file from the native Open dialog (`OsChosen`, §9.4): regular file, capped.
    ImportSource,
}

/// Largest bookmark import file.
pub const IMPORT_MAX_BYTES: u64 = 5 * 1024 * 1024;
pub const KEY_MAX_BYTES: u64 = 64 * 1024;
pub const CONFIG_MAX_BYTES: u64 = 4 * 1024 * 1024;

impl FileKind {
    /// Read cap (§9.4); `None` = streamed (uploads).
    pub const fn max_bytes(self) -> Option<u64> {
        match self {
            Self::PrivateKey | Self::PublicKey => Some(KEY_MAX_BYTES),
            Self::UserConfig
            | Self::SystemConfig
            | Self::UserKnownHosts
            | Self::GlobalKnownHosts => Some(CONFIG_MAX_BYTES),
            Self::UploadSource => None,
            Self::ImportSource => Some(IMPORT_MAX_BYTES),
        }
    }

    /// `true` for paths from config/bookmarks (strict allowlist), `false` for OS-chosen paths.
    pub const fn is_config_source(self) -> bool {
        !matches!(self, Self::UploadSource | Self::ImportSource)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TrustError {
    /// Owner/mode/ACL policy violated (or could not be evaluated — fail closed).
    #[error("untrusted file: {0}")]
    Untrusted(String),
    #[error("not a regular file")]
    NotRegularFile,
}

/// Environment of the check (overridable in tests).
#[derive(Debug, Clone)]
pub struct TrustContext {
    #[cfg(unix)]
    pub uid: u32,
    #[cfg(windows)]
    pub user_sid: Option<String>,
    /// `%ProgramData%\ssh` (Windows); unused on Unix.
    pub program_data_ssh: Option<PathBuf>,
}

impl TrustContext {
    /// The real process context.
    pub fn current() -> Self {
        #[cfg(unix)]
        {
            // SAFETY: getuid has no preconditions.
            let uid = unsafe { libc::getuid() };
            Self {
                uid,
                program_data_ssh: None,
            }
        }
        #[cfg(windows)]
        {
            let program_data = std::env::var_os("ProgramData")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
            Self {
                user_sid: super::win_security::current_user_sid().ok(),
                program_data_ssh: Some(program_data.join("ssh")),
            }
        }
    }
}

/// Check an opened file against its §7.4.1 row. Only the handle is used: on Windows the
/// location decision takes the handle's final path (`GetFinalPathNameByHandleW`).
pub fn check_file(file: &File, kind: FileKind, ctx: &TrustContext) -> Result<(), TrustError> {
    let meta = file
        .metadata()
        .map_err(|e| TrustError::Untrusted(format!("fstat failed: {e}")))?;
    if !meta.is_file() {
        return Err(TrustError::NotRegularFile);
    }
    if matches!(kind, FileKind::UploadSource | FileKind::ImportSource) {
        return Ok(());
    }
    platform_check(file, &meta, kind, ctx)
}

#[cfg(unix)]
fn platform_check(
    _file: &File,
    meta: &std::fs::Metadata,
    kind: FileKind,
    ctx: &TrustContext,
) -> Result<(), TrustError> {
    use std::os::unix::fs::MetadataExt;
    unix_rule(kind, meta.uid(), meta.mode(), ctx.uid)
}

/// Pure Unix rule (unit-tested per row).
#[cfg(unix)]
pub fn unix_rule(kind: FileKind, owner: u32, mode: u32, uid: u32) -> Result<(), TrustError> {
    let owner_user_or_root = owner == uid || owner == 0;
    let (ok, why) = match kind {
        FileKind::UserConfig => (
            owner_user_or_root && mode & 0o022 == 0,
            "owner must be you or root, mode & 022 == 0",
        ),
        FileKind::SystemConfig | FileKind::GlobalKnownHosts => (owner == 0, "owner must be root"),
        FileKind::UserKnownHosts => (
            owner_user_or_root && mode & 0o002 == 0,
            "owner must be you or root, not world-writable",
        ),
        FileKind::PrivateKey => (
            owner == uid && mode & 0o077 == 0,
            "owner must be you, mode & 077 == 0",
        ),
        FileKind::PublicKey => (owner_user_or_root, "owner must be you or root"),
        FileKind::UploadSource | FileKind::ImportSource => (true, ""),
    };
    if ok {
        Ok(())
    } else {
        Err(TrustError::Untrusted(format!(
            "{why} (owner {owner}, mode {:o})",
            mode & 0o7777
        )))
    }
}

#[cfg(windows)]
fn platform_check(
    file: &File,
    _meta: &std::fs::Metadata,
    kind: FileKind,
    ctx: &TrustContext,
) -> Result<(), TrustError> {
    use super::win_security::{Policy, evaluate, final_path, security_of_file};

    let Some(user) = ctx.user_sid.as_deref() else {
        return Err(TrustError::Untrusted("current user SID unknown".into()));
    };
    let final_path =
        final_path(file).map_err(|e| TrustError::Untrusted(format!("final path: {e}")))?;
    let info = security_of_file(file)
        .map_err(|e| TrustError::Untrusted(format!("GetSecurityInfo: {e}")))?;

    if let Some(root) = ctx.program_data_ssh.as_deref()
        && let Some(chain) = system_chain(&final_path, root)
    {
        evaluate(&info, Policy::SystemOnly, user).map_err(TrustError::Untrusted)?;
        return check_dir_chain(&chain, user);
    }
    let policy = match kind {
        FileKind::PrivateKey => Policy::UserPrivate,
        FileKind::PublicKey => Policy::UserOwnerOnly,
        _ => Policy::UserWritable,
    };
    evaluate(&info, policy, user).map_err(TrustError::Untrusted)
}

/// Every directory from `%ProgramData%\ssh` down to the file's parent, by path (§7.4.1).
#[cfg(windows)]
fn check_dir_chain(chain: &[PathBuf], user: &str) -> Result<(), TrustError> {
    use super::win_security::{Policy, evaluate, security_of_path};
    for dir in chain {
        let info = security_of_path(dir).map_err(|e| {
            TrustError::Untrusted(format!("GetNamedSecurityInfoW {}: {e}", dir.display()))
        })?;
        evaluate(&info, Policy::SystemOnly, user)
            .map_err(|why| TrustError::Untrusted(format!("{}: {why}", dir.display())))?;
    }
    Ok(())
}

/// Strip the `\\?\` verbatim prefix for comparisons.
#[cfg(windows)]
fn strip_verbatim(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    s.strip_prefix(r"\\?\")
        .map_or_else(|| p.to_path_buf(), PathBuf::from)
}

/// If `file` lies under `root` (case-insensitive, component-wise), the directories from `root`
/// down to the file's parent (inclusive); otherwise `None`.
#[cfg(windows)]
pub(crate) fn system_chain(file: &Path, root: &Path) -> Option<Vec<PathBuf>> {
    let file = strip_verbatim(file);
    let root = strip_verbatim(root);
    let fc: Vec<_> = file.components().collect();
    let root_len = root.components().count();
    if fc.len() <= root_len {
        return None;
    }
    let under = root
        .components()
        .zip(&fc)
        .all(|(a, b)| a.as_os_str().eq_ignore_ascii_case(b.as_os_str()));
    if !under {
        return None;
    }
    let mut chain = vec![root.clone()];
    let mut cur = root;
    for comp in fc.iter().skip(root_len).take(fc.len() - root_len - 1) {
        cur = cur.join(comp.as_os_str());
        chain.push(cur.clone());
    }
    Some(chain)
}

/// Startup check of an app storage directory (§7.1: Windows runs the user-config row on
/// `app_config_dir` and `app_log_dir`). Unix dirs are created 0700 by `harden_process`.
#[cfg(windows)]
pub fn check_private_dir(dir: &Path) -> Result<(), String> {
    use super::win_security::{Policy, current_user_sid, evaluate, security_of_path};
    let user = current_user_sid().map_err(|e| e.to_string())?;
    let info = security_of_path(dir).map_err(|e| e.to_string())?;
    evaluate(&info, Policy::UserWritable, &user)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn caps() {
        assert_eq!(FileKind::PrivateKey.max_bytes(), Some(64 * 1024));
        assert_eq!(FileKind::UserConfig.max_bytes(), Some(4 * 1024 * 1024));
        assert_eq!(FileKind::UploadSource.max_bytes(), None);
        assert!(!FileKind::UploadSource.is_config_source());
        assert_eq!(FileKind::ImportSource.max_bytes(), Some(5 * 1024 * 1024));
        assert!(!FileKind::ImportSource.is_config_source());
    }

    #[cfg(unix)]
    mod unix_rows {
        use super::super::*;

        const ME: u32 = 1000;
        const OTHER: u32 = 1001;

        #[test]
        fn user_config() {
            assert!(unix_rule(FileKind::UserConfig, ME, 0o644, ME).is_ok());
            assert!(unix_rule(FileKind::UserConfig, 0, 0o644, ME).is_ok());
            assert!(unix_rule(FileKind::UserConfig, ME, 0o664, ME).is_err()); // 0664 rejected
            assert!(unix_rule(FileKind::UserConfig, ME, 0o646, ME).is_err());
            assert!(unix_rule(FileKind::UserConfig, OTHER, 0o600, ME).is_err());
        }

        #[test]
        fn known_hosts() {
            assert!(unix_rule(FileKind::UserKnownHosts, ME, 0o664, ME).is_ok()); // umask 002 accepted
            assert!(unix_rule(FileKind::UserKnownHosts, ME, 0o666, ME).is_err()); // world-writable
            assert!(unix_rule(FileKind::UserKnownHosts, OTHER, 0o644, ME).is_err());
            assert!(unix_rule(FileKind::GlobalKnownHosts, 0, 0o666, ME).is_ok());
            assert!(unix_rule(FileKind::GlobalKnownHosts, ME, 0o644, ME).is_err());
        }

        #[test]
        fn keys() {
            assert!(unix_rule(FileKind::PrivateKey, ME, 0o600, ME).is_ok());
            assert!(unix_rule(FileKind::PrivateKey, ME, 0o640, ME).is_err()); // 0640 rejected
            assert!(unix_rule(FileKind::PrivateKey, 0, 0o600, ME).is_err());
            assert!(unix_rule(FileKind::PublicKey, ME, 0o644, ME).is_ok());
            assert!(unix_rule(FileKind::PublicKey, OTHER, 0o644, ME).is_err());
        }

        #[test]
        fn system_config() {
            assert!(unix_rule(FileKind::SystemConfig, 0, 0o666, ME).is_ok());
            assert!(unix_rule(FileKind::SystemConfig, ME, 0o644, ME).is_err());
        }
    }

    #[cfg(windows)]
    #[test]
    fn system_chain_detection() {
        let root = Path::new(r"C:\ProgramData\ssh");
        assert_eq!(
            system_chain(Path::new(r"\\?\C:\programdata\SSH\conf.d\x.conf"), root),
            Some(vec![
                PathBuf::from(r"C:\ProgramData\ssh"),
                PathBuf::from(r"C:\ProgramData\ssh\conf.d")
            ])
        );
        assert_eq!(
            system_chain(Path::new(r"C:\ProgramData\ssh\ssh_config"), root),
            Some(vec![root.to_path_buf()])
        );
        assert_eq!(
            system_chain(Path::new(r"C:\ProgramData\sshx\ssh_config"), root),
            None
        );
        assert_eq!(
            system_chain(Path::new(r"C:\Users\alice\.ssh\config"), root),
            None
        );
        assert_eq!(system_chain(root, root), None);
    }
}
