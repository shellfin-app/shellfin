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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalUser {
    pub name: String,
    /// `name` is exactly OpenSSH's `pw_name` (what its `%u`, default user and `%C` use). When
    /// `false` the name is a best-effort fallback: `%u`, and `%r`/`%C` of a hop that uses it,
    /// are unavailable (§7.4) instead of naming a file OpenSSH does not use.
    pub openssh_exact: bool,
}

/// Unix: `getpwuid_r(getuid())` (exact, as OpenSSH), falling back to `$USER`/`$LOGNAME`.
/// Windows: the Win32-OpenSSH form ([`openssh_windows_user`]: the account name of the token
/// user, lowercased), falling back to `%USERNAME%` as is.
pub fn local_user() -> LocalUser {
    #[cfg(unix)]
    {
        if let Some(name) = passwd_name() {
            return LocalUser {
                name,
                openssh_exact: true,
            };
        }
        let name = ["USER", "LOGNAME"]
            .into_iter()
            .find_map(|var| std::env::var(var).ok().filter(|v| !v.is_empty()))
            .unwrap_or_else(|| "user".to_owned());
        LocalUser {
            name,
            openssh_exact: false,
        }
    }
    #[cfg(not(unix))]
    {
        #[cfg(windows)]
        {
            if let Some(name) =
                win_security::current_account().and_then(|(user, domain, computer)| {
                    openssh_windows_user(&user, &domain, &computer)
                })
            {
                return LocalUser {
                    name,
                    openssh_exact: true,
                };
            }
        }
        LocalUser {
            name: std::env::var("USERNAME")
                .ok()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| "user".to_owned()),
            openssh_exact: false,
        }
    }
}

/// Win32-OpenSSH's `pw_name` for the current user (`contrib/win32/win32compat/pwd.c`
/// `get_passwd`): `LookupAccountSidW` gives `user` and `domain`; if `domain` equals the
/// NetBIOS computer name (`_wcsicmp`) it is a local account and the name is `user`, otherwise
/// `domain\user`; the result is lowercased (`_wcslwr_s`). Win32-OpenSSH 9.5p2 `ssh -G` confirms
/// the local case (`Smith` → `smith`).
///
/// Returns `None` (the caller falls back and `%u`/`%C` become unavailable) where the exact
/// result is not certain:
/// - a domain (or Azure AD) account: the `domain\user` form is read from the source, but not
///   checked against a real domain-joined `ssh -G`, and logging in as `domain\user` would change
///   which account Shellfin uses;
/// - any non-ASCII name: `_wcsicmp`/`_wcslwr_s` depend on the C runtime locale ssh sets;
/// - an empty part.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn openssh_windows_user(user: &str, domain: &str, computer: &str) -> Option<String> {
    let plain = |s: &str| !s.is_empty() && s.is_ascii();
    if !(plain(user) && plain(domain) && plain(computer)) {
        return None;
    }
    domain
        .eq_ignore_ascii_case(computer)
        .then(|| user.to_ascii_lowercase())
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

/// The local host name for the ssh_config `%l`/`%L` tokens (and the `%C` hash), without spawning
/// a process: Unix `gethostname()` (macOS gives e.g. `name.local`, which is what OpenSSH uses),
/// Windows `GetComputerNameExW(ComputerNameDnsHostname)` (what Win32-OpenSSH's `gethostname`
/// returns). Never `$HOSTNAME`/`%COMPUTERNAME%`. `None` if unavailable or not a plain ASCII host
/// name ([`plain_local_host`]): the tokens are then unsupported, never guessed.
pub fn local_host_name() -> Option<String> {
    raw_local_host_name().and_then(|n| plain_local_host(&n).then_some(n))
}

/// Accept only a name that is safe as a path component and as hash input: ASCII letters,
/// digits, `-`, `_`, `.`; not starting with `-` or `.`; at most 253 bytes; and
/// `valid_hostname`. Everything else (`/`, `\`, `..`, `%`, whitespace, controls, non-ASCII,
/// empty, too long) makes the name unavailable.
pub fn plain_local_host(name: &str) -> bool {
    crate::sshconfig::validate::valid_hostname(name)
        && !name.starts_with(['-', '.'])
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

#[cfg(unix)]
fn raw_local_host_name() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: `buf` is a valid, writable buffer of `buf.len()` bytes that outlives the call.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast::<libc::c_char>(), buf.len()) };
    if rc != 0 {
        return None;
    }
    // A truncated name is not guaranteed to be NUL-terminated: treat that as unavailable.
    std::ffi::CStr::from_bytes_until_nul(&buf)
        .ok()?
        .to_str()
        .ok()
        .map(str::to_owned)
}

#[cfg(windows)]
fn raw_local_host_name() -> Option<String> {
    use windows_sys::Win32::System::SystemInformation::{
        ComputerNameDnsHostname, GetComputerNameExW,
    };
    let mut buf = [0u16; 256];
    let mut len = u32::try_from(buf.len()).ok()?;
    // SAFETY: `buf` holds `len` UTF-16 units and outlives the call; on success `len` is the
    // number of units written (without the terminating NUL).
    let ok = unsafe { GetComputerNameExW(ComputerNameDnsHostname, buf.as_mut_ptr(), &mut len) };
    if ok == 0 {
        return None;
    }
    let units = buf.get(..usize::try_from(len).ok()?)?;
    String::from_utf16(units).ok()
}

#[cfg(not(any(unix, windows)))]
fn raw_local_host_name() -> Option<String> {
    None
}

/// The numeric local user id for the ssh_config `%i` token: Unix `getuid()`; Windows `1`, as
/// Win32-OpenSSH (which has no numeric uid and reports 1).
pub fn local_uid() -> Option<String> {
    #[cfg(unix)]
    {
        // SAFETY: `getuid` has no preconditions and cannot fail.
        Some(unsafe { libc::getuid() }.to_string())
    }
    #[cfg(windows)]
    {
        Some("1".to_owned())
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
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

    /// HIGH (#7 review): the Win32-OpenSSH form of the local account. `Alice` on a machine
    /// `LAPTOP` is `alice`; a domain account, non-ASCII or empty parts are not guessed.
    #[test]
    fn openssh_windows_user_form() {
        assert_eq!(
            openssh_windows_user("Alice", "LAPTOP", "laptop").as_deref(),
            Some("alice")
        );
        assert_eq!(
            openssh_windows_user("SMITH", "Desktop-AB12", "DESKTOP-AB12").as_deref(),
            Some("smith")
        );
        for (user, domain, computer) in [
            ("alice", "CONTOSO", "LAPTOP"),
            ("alice", "AzureAD", "LAPTOP"),
            ("Ålice", "LAPTOP", "LAPTOP"),
            ("alice", "ПК", "ПК"),
            ("", "LAPTOP", "LAPTOP"),
            ("alice", "", ""),
        ] {
            assert_eq!(
                openssh_windows_user(user, domain, computer),
                None,
                "{user} {domain} {computer}"
            );
        }
    }

    /// This machine's account: on Windows a local account comes out lowercased.
    #[test]
    fn local_user_is_plausible() {
        let u = local_user();
        assert!(!u.name.is_empty());
        #[cfg(windows)]
        if u.openssh_exact {
            assert_eq!(u.name, u.name.to_ascii_lowercase());
        }
    }

    #[test]
    fn local_host_name_and_uid() {
        // Whatever this machine reports, it is either unavailable or a plain name.
        if let Some(name) = local_host_name() {
            assert!(plain_local_host(&name), "{name:?}");
        }
        let uid = local_uid().unwrap();
        assert!(!uid.is_empty() && uid.bytes().all(|b| b.is_ascii_digit()));
        #[cfg(windows)]
        assert_eq!(uid, "1");
    }

    /// (hostile input) a host name that could act as path syntax, a token, an option or
    /// invisible text is never used for `%l`/`%L`/`%C`.
    #[test]
    fn plain_local_host_rejects_hostile_names() {
        for ok in [
            "laptop",
            "laptop.example.org",
            "DESKTOP-AB12",
            "build_01",
            "mac.local",
        ] {
            assert!(plain_local_host(ok), "{ok}");
        }
        let long = "a".repeat(300);
        let rtl = format!("lap{}top", '\u{202e}');
        let nul = format!("lap{}top", char::from(0));
        let esc = format!("lap{}top", char::from(0x1b));
        for bad in [
            "",
            "a/b",
            "..",
            ".",
            ".hidden",
            r"a\b",
            "%h",
            "-x",
            "a b",
            "a:b",
            "${X}",
            "ноутбук",
            long.as_str(),
            rtl.as_str(),
            nul.as_str(),
            esc.as_str(),
        ] {
            assert!(!plain_local_host(bad), "{bad:?}");
        }
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
