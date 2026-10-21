//! End-to-end test isolation (cargo feature `e2e`; never enabled in release artifacts).
//!
//! In an `e2e` build the environment variable `SHELLFIN_E2E_ROOT=<absolute dir>` is
//! **required** and redirects every piece of user state below that directory:
//!
//! | state | normal build | `e2e` build |
//! |---|---|---|
//! | home (`~/.ssh/config`, `known_hosts`, default identities, watcher, key scan, `${HOME}`) | OS home | `<root>/home` |
//! | system ssh dir (`ssh_config`, `ssh_known_hosts`) | `/etc/ssh`, `%ProgramData%\ssh` | `<root>/system-ssh` |
//! | app config (bookmarks, settings, recents) | Tauri `app_config_dir` | `<root>/config` |
//! | logs | Tauri `app_log_dir` | `<root>/logs` |
//! | WebView profile (localStorage, caches) | platform default | `<root>/webview` |
//! | keychain | OS keychain | `<root>/keyring.json` (plaintext test fake) |
//! | default agent (`SSH_AUTH_SOCK`) | env, then the Windows OpenSSH pipe | only `SHELLFIN_E2E_SSH_AUTH_SOCK`; no pipe fallback |
//!
//! Fail closed: an `e2e` build without the variable, or with a relative path, refuses to start
//! (it never falls back to the real user state). Without the feature this module compiles to
//! constants: the variables are never read and their names are not in the binary (CI greps the
//! release binaries for the marker). An optimised `e2e` build does not compile.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[cfg(all(feature = "e2e", not(debug_assertions)))]
compile_error!(
    "the `e2e` feature redirects user state for automated tests and must never be part of an \
     optimised (release) build; build it with `tauri build --debug --features e2e`"
);

/// The redirected locations below the e2e root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub root: PathBuf,
    pub home: PathBuf,
    pub system_ssh_dir: PathBuf,
    pub app_config_dir: PathBuf,
    pub app_log_dir: PathBuf,
    pub webview_data_dir: PathBuf,
    pub keyring_file: PathBuf,
}

impl Layout {
    pub fn under(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            home: root.join("home"),
            system_ssh_dir: root.join("system-ssh"),
            app_config_dir: root.join("config"),
            app_log_dir: root.join("logs"),
            webview_data_dir: root.join("webview"),
            keyring_file: root.join("keyring.json"),
        }
    }
}

/// The layout requested by the environment (`lookup` = `std::env::var_os` in the app; injected
/// for tests). Without the `e2e` feature this is always `Ok(None)` and `lookup` is never called.
#[allow(clippy::needless_pass_by_value)]
pub fn layout_from(lookup: impl Fn(&str) -> Option<OsString>) -> Result<Option<Layout>, String> {
    #[cfg(feature = "e2e")]
    {
        let Some(raw) = lookup(imp::ROOT_ENV).filter(|v| !v.is_empty()) else {
            return Err(format!(
                "this is an e2e test build: set {} to an absolute directory",
                imp::ROOT_ENV
            ));
        };
        let root = PathBuf::from(raw);
        if !root.is_absolute() {
            return Err(format!("{} must be an absolute path", imp::ROOT_ENV));
        }
        Ok(Some(Layout::under(&root)))
    }
    #[cfg(not(feature = "e2e"))]
    {
        let _ = lookup;
        Ok(None)
    }
}

/// The active layout (set once at startup by [`activate`]); always `None` without the feature.
pub fn active() -> Option<&'static Layout> {
    #[cfg(feature = "e2e")]
    {
        imp::ACTIVE.get()
    }
    #[cfg(not(feature = "e2e"))]
    {
        None
    }
}

pub fn is_active() -> bool {
    active().is_some()
}

/// Create the directories and make `layout` the process-wide redirect (first call wins).
/// Without the feature this is a no-op (there is never a layout to activate).
pub fn activate(layout: Layout) -> std::io::Result<&'static Layout> {
    #[cfg(feature = "e2e")]
    {
        for dir in [
            &layout.root,
            &layout.home,
            &layout.app_config_dir,
            &layout.app_log_dir,
            &layout.webview_data_dir,
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(imp::ACTIVE.get_or_init(|| layout))
    }
    #[cfg(not(feature = "e2e"))]
    {
        let _ = layout;
        Err(std::io::Error::other("e2e isolation is not compiled in"))
    }
}

/// Environment lookup for the redirected app (`${VAR}` tokens, `SSH_AUTH_SOCK`): `HOME` /
/// `USERPROFILE` are the e2e home, `SSH_AUTH_SOCK` comes only from
/// `SHELLFIN_E2E_SSH_AUTH_SOCK`, everything else from the process environment.
pub fn env_lookup(layout: &Layout) -> crate::sshconfig::EnvLookup {
    #[cfg(feature = "e2e")]
    {
        let home = layout.home.to_string_lossy().into_owned();
        std::sync::Arc::new(move |name: &str| match name {
            "HOME" | "USERPROFILE" => Some(home.clone()),
            "SSH_AUTH_SOCK" => std::env::var(imp::AGENT_ENV).ok(),
            other => std::env::var(other).ok(),
        })
    }
    #[cfg(not(feature = "e2e"))]
    {
        let _ = layout;
        std::sync::Arc::new(|name: &str| std::env::var(name).ok())
    }
}

/// The file-backed keychain fake (`<root>/keyring.json`, `{account: password}`).
#[cfg(feature = "e2e")]
pub mod keyring {
    use std::collections::BTreeMap;
    use std::sync::{Mutex, PoisonError};

    use zeroize::Zeroizing;

    use crate::error::{AppError, ErrorCode};

    static LOCK: Mutex<()> = Mutex::new(());

    fn err(e: impl std::fmt::Display) -> AppError {
        AppError::new(ErrorCode::KeyringError).with_detail(format!("e2e keyring: {e}"))
    }

    fn load(path: &std::path::Path) -> Result<BTreeMap<String, String>, AppError> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(err),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(err(e)),
        }
    }

    fn store(path: &std::path::Path, map: &BTreeMap<String, String>) -> Result<(), AppError> {
        let json = Zeroizing::new(serde_json::to_vec_pretty(map).map_err(err)?);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &*json).map_err(err)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)).map_err(err)?;
        }
        std::fs::rename(&tmp, path).map_err(err)
    }

    pub fn get(
        path: &std::path::Path,
        account: &str,
    ) -> Result<Option<Zeroizing<String>>, AppError> {
        let _g = LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(load(path)?.remove(account).map(Zeroizing::new))
    }

    pub fn set(path: &std::path::Path, account: &str, password: &str) -> Result<(), AppError> {
        let _g = LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let mut map = load(path)?;
        map.insert(account.to_owned(), password.to_owned());
        store(path, &map)
    }

    pub fn delete(path: &std::path::Path, account: &str) -> Result<(), AppError> {
        let _g = LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let mut map = load(path)?;
        if map.remove(account).is_some() {
            store(path, &map)?;
        }
        Ok(())
    }
}

#[cfg(feature = "e2e")]
mod imp {
    use std::sync::OnceLock;

    pub const ROOT_ENV: &str = "SHELLFIN_E2E_ROOT";
    pub const AGENT_ENV: &str = "SHELLFIN_E2E_SSH_AUTH_SOCK";
    pub static ACTIVE: OnceLock<super::Layout> = OnceLock::new();
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn abs_root() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\e2e-root")
        } else {
            PathBuf::from("/e2e-root")
        }
    }

    #[test]
    fn layout_is_below_the_root() {
        let root = abs_root();
        let l = Layout::under(&root);
        for p in [
            &l.home,
            &l.system_ssh_dir,
            &l.app_config_dir,
            &l.app_log_dir,
            &l.webview_data_dir,
            &l.keyring_file,
        ] {
            assert!(p.starts_with(&root), "{}", p.display());
            assert_ne!(p, &root);
        }
    }

    /// Without the feature the variable has no effect: it is not even read.
    #[cfg(not(feature = "e2e"))]
    #[test]
    fn env_var_has_no_effect_without_the_feature() {
        let read = std::cell::Cell::new(false);
        let got = layout_from(|_| {
            read.set(true);
            Some(abs_root().into_os_string())
        });
        assert_eq!(got, Ok(None));
        assert!(!read.get(), "the environment must not be consulted");
        assert!(active().is_none());
        assert!(!is_active());
        assert!(activate(Layout::under(&abs_root())).is_err());
        assert!(active().is_none());
    }

    #[cfg(feature = "e2e")]
    #[test]
    fn e2e_build_requires_an_absolute_root() {
        let root = abs_root();
        let got =
            layout_from(|k| (k == "SHELLFIN_E2E_ROOT").then(|| root.clone().into_os_string()));
        assert_eq!(got, Ok(Some(Layout::under(&root))));
        assert!(
            layout_from(|_| None).is_err(),
            "fail closed without the variable"
        );
        assert!(layout_from(|_| Some(OsString::new())).is_err());
        assert!(layout_from(|_| Some(OsString::from("relative/dir"))).is_err());
    }

    #[cfg(feature = "e2e")]
    #[test]
    fn e2e_env_lookup_redirects_home_and_agent() {
        let l = Layout::under(&abs_root());
        let env = env_lookup(&l);
        assert_eq!(env("HOME").as_deref(), Some(&*l.home.to_string_lossy()));
        assert_eq!(
            env("USERPROFILE").as_deref(),
            Some(&*l.home.to_string_lossy())
        );
        // The real SSH_AUTH_SOCK is never used; only the e2e variable (unset here).
        if std::env::var_os("SHELLFIN_E2E_SSH_AUTH_SOCK").is_none() {
            assert_eq!(env("SSH_AUTH_SOCK"), None);
        }
    }

    #[cfg(feature = "e2e")]
    #[test]
    fn e2e_keyring_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("keyring.json");
        assert!(keyring::get(&f, "bookmark:a").unwrap().is_none());
        keyring::set(&f, "bookmark:a", "pw-1").unwrap();
        keyring::set(&f, "bookmark:b", "pw-2").unwrap();
        assert_eq!(
            keyring::get(&f, "bookmark:a")
                .unwrap()
                .as_deref()
                .map(String::as_str),
            Some("pw-1")
        );
        keyring::delete(&f, "bookmark:a").unwrap();
        keyring::delete(&f, "bookmark:a").unwrap();
        assert!(keyring::get(&f, "bookmark:a").unwrap().is_none());
        assert!(keyring::get(&f, "bookmark:b").unwrap().is_some());
    }
}
