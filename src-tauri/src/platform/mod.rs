//! Platform helpers (§3, §7.1, §9.4, §9.6): `Paths`, `harden_process()`, the local-path
//! allowlist (`local_path`), file trust checks (`file_trust`) and Win32 security helpers.

pub mod file_trust;
pub mod local_path;
pub mod os_info;
#[cfg(windows)]
pub mod win_security;

use std::io;
use std::path::{Path, PathBuf};

/// Well-known locations, resolved once at startup.
#[derive(Debug, Clone)]
pub struct Paths {
    pub home: PathBuf,
    /// `~/.ssh`
    pub user_ssh_dir: PathBuf,
    /// `~/.ssh/config`
    pub user_ssh_config: PathBuf,
    /// `~/.ssh/known_hosts`
    pub user_known_hosts: PathBuf,
    /// `<app_config_dir>` (bookmarks, settings, recents) — private (0700 on Unix).
    pub app_config_dir: PathBuf,
    /// `<app_log_dir>` — private (0700 on Unix).
    pub app_log_dir: PathBuf,
}

impl Paths {
    pub fn new(home: PathBuf, app_config_dir: PathBuf, app_log_dir: PathBuf) -> Self {
        let user_ssh_dir = home.join(".ssh");
        Self {
            user_ssh_config: user_ssh_dir.join("config"),
            user_known_hosts: user_ssh_dir.join("known_hosts"),
            user_ssh_dir,
            home,
            app_config_dir,
            app_log_dir,
        }
    }
}

/// The local account name (`%u`, default remote user, `Match localuser`).
/// Unix: `getpwuid_r(getuid())`, falling back to `$USER`/`$LOGNAME`; Windows: `%USERNAME%`.
pub fn local_user_name() -> String {
    #[cfg(unix)]
    {
        if let Some(name) = passwd_name() {
            return name;
        }
        for var in ["USER", "LOGNAME"] {
            if let Ok(v) = std::env::var(var)
                && !v.is_empty()
            {
                return v;
            }
        }
        "user".to_owned()
    }
    #[cfg(not(unix))]
    {
        std::env::var("USERNAME")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "user".to_owned())
    }
}

#[cfg(unix)]
fn passwd_name() -> Option<String> {
    let mut buf = vec![0 as libc::c_char; 4096];
    // SAFETY: zeroed passwd is a valid out-parameter; `buf` outlives the call and `result` only
    // points into `pwd`/`buf`.
    unsafe {
        let mut pwd: libc::passwd = std::mem::zeroed();
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let rc = libc::getpwuid_r(
            libc::getuid(),
            &mut pwd,
            buf.as_mut_ptr(),
            buf.len(),
            &mut result,
        );
        if rc != 0 || result.is_null() || pwd.pw_name.is_null() {
            return None;
        }
        std::ffi::CStr::from_ptr(pwd.pw_name)
            .to_str()
            .ok()
            .map(str::to_owned)
    }
}

/// Process hardening at startup (§9.6), run **before** logging starts:
/// - Linux: `prctl(PR_SET_DUMPABLE, 0)`; macOS/Linux: `setrlimit(RLIMIT_CORE, 0)`.
/// - Create the app config and log directories private (Unix `0700`, also tightening
///   pre-existing ones). No process-wide umask change.
/// - Windows: no WER manipulation in v1; both directories (under `%APPDATA%`/`%LOCALAPPDATA%`)
///   get the §7.4.1 user-config row check (owner ∈ user/SYSTEM/Administrators, no write-type
///   ACE for other SIDs); a violation is logged as a warning (§7.1).
///
/// Returns English notes for anything that failed; the caller logs them once logging is up.
pub fn harden_process(paths: &Paths) -> Vec<String> {
    let mut issues = Vec::new();
    if let Err(err) = disable_core_dumps() {
        issues.push(format!("could not disable core dumps: {err}"));
    }
    for dir in [&paths.app_config_dir, &paths.app_log_dir] {
        if let Err(err) = create_private_dir(dir) {
            issues.push(format!(
                "could not prepare private directory {}: {err}",
                dir.display()
            ));
            continue;
        }
        #[cfg(windows)]
        if let Err(why) = file_trust::check_private_dir(dir) {
            issues.push(format!(
                "app directory {} is not private to the current user: {why}",
                dir.display()
            ));
        }
    }
    issues
}

#[cfg(unix)]
fn disable_core_dumps() -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: plain syscall wrapper with integer arguments; no pointers involved.
        let rc = unsafe {
            libc::prctl(
                libc::PR_SET_DUMPABLE,
                0 as libc::c_ulong,
                0 as libc::c_ulong,
                0 as libc::c_ulong,
                0 as libc::c_ulong,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    let limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is a valid, initialized `rlimit` that outlives the call.
    let rc = unsafe { libc::setrlimit(libc::RLIMIT_CORE, &limit) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(unix))]
fn disable_core_dumps() -> io::Result<()> {
    Ok(())
}

/// Create `dir` (and missing parents) private to the current user.
#[cfg(unix)]
pub fn create_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let meta = std::fs::metadata(dir)?;
    if !meta.is_dir() {
        return Err(io::Error::other("not a directory"));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Create `dir` (and missing parents). On Windows the directories live under
/// `%APPDATA%`/`%LOCALAPPDATA%`, whose inherited ACL grants only the user, SYSTEM and
/// Administrators; verifying that ACL is done by `file_trust`.
#[cfg(not(unix))]
pub fn create_private_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    if std::fs::metadata(dir)?.is_dir() {
        Ok(())
    } else {
        Err(io::Error::other("not a directory"))
    }
}

/// Test helpers shared by modules whose tests go through the §7.4.1 trust checks; also used by
/// the Docker integration suite (`--features it`, where dev-dependencies are not available to
/// the library, hence no `tempfile` outside `cfg(test)`).
#[cfg(any(test, feature = "it"))]
pub mod test_support {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::path::Path;

    /// A temp dir whose ACL is private to the current user (Windows: protected DACL for the user,
    /// SYSTEM and Administrators, inherited by children). Developer machines may have extra
    /// inheritable ACEs on `%TEMP%` (e.g. sandbox groups), which the trust checks correctly reject.
    #[cfg(test)]
    pub fn private_tempdir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        make_private(dir.path());
        dir
    }

    /// Make an existing directory (and everything created in it later) private to the current
    /// user: Windows protected DACL (user, SYSTEM, Administrators; inheritable), Unix mode 0700.
    pub fn make_private(dir: &Path) {
        #[cfg(windows)]
        {
            let me = super::win_security::current_user_sid().unwrap();
            set_dacl(
                dir,
                &format!("D:P(A;OICI;FA;;;{me})(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)"),
            );
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    /// Replace the DACL of `path` with the one in `sddl` (e.g. `"D:P(A;;FA;;;WD)"`).
    #[cfg(windows)]
    pub fn set_dacl(path: &Path, sddl: &str) {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::{HLOCAL, LocalFree};
        use windows_sys::Win32::Security::Authorization::{
            ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
        };
        use windows_sys::Win32::Security::{
            DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SetFileSecurityW,
        };

        let w: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
        let p: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: test-only FFI with NUL-terminated buffers; `sd` is freed with LocalFree.
        unsafe {
            assert_ne!(
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    w.as_ptr(),
                    SDDL_REVISION_1,
                    &mut sd,
                    std::ptr::null_mut()
                ),
                0,
                "bad SDDL {sddl}"
            );
            let ok = SetFileSecurityW(p.as_ptr(), DACL_SECURITY_INFORMATION, sd);
            LocalFree(sd as HLOCAL);
            assert_ne!(ok, 0, "SetFileSecurityW failed");
        }
    }

    #[cfg(not(windows))]
    pub fn set_dacl(_path: &Path, _sddl: &str) {}
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::test_support::private_tempdir;
    use super::*;

    #[test]
    fn paths_layout() {
        let p = Paths::new(
            PathBuf::from("/home/u"),
            PathBuf::from("/cfg"),
            PathBuf::from("/log"),
        );
        assert_eq!(p.user_ssh_config, Path::new("/home/u/.ssh/config"));
        assert_eq!(p.user_known_hosts, Path::new("/home/u/.ssh/known_hosts"));
    }

    #[test]
    fn private_dir_is_created() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("a").join("b");
        create_private_dir(&dir).unwrap();
        assert!(dir.is_dir());
        create_private_dir(&dir).unwrap(); // idempotent
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
            create_private_dir(&dir).unwrap();
            assert_eq!(
                std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn harden_process_reports_no_issues_for_private_dirs() {
        let tmp = private_tempdir();
        let paths = Paths::new(
            tmp.path().to_path_buf(),
            tmp.path().join("cfg"),
            tmp.path().join("log"),
        );
        assert!(harden_process(&paths).is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn harden_process_warns_about_foreign_writable_app_dir() {
        let tmp = private_tempdir();
        let cfg = tmp.path().join("cfg");
        std::fs::create_dir(&cfg).unwrap();
        test_support::set_dacl(&cfg, "D:P(A;OICI;FA;;;WD)");
        let paths = Paths::new(tmp.path().to_path_buf(), cfg, tmp.path().join("log"));
        let issues = harden_process(&paths);
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(issues[0].contains("not private"), "{issues:?}");
    }
}
