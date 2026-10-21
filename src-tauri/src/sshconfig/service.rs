//! Production wiring of the ssh_config parser: the trusted [`ConfigSource`] (§9.4 allowlist +
//! §7.4.1 trust check on the opened handle + 4 MiB cap via `platform::local_path`) and the
//! [`SshConfigService`] that owns the cache (`AppState.ssh_config`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use super::{ConfigEnv, ConfigScope, ConfigSource, ParsedConfig, ReadOutcome, SshConfigCache};
use crate::ipc::dto::SshConfigSnapshot;
use crate::platform::file_trust::FileKind;
use crate::platform::local_path::{LocalPathError, config_path_allowed, read_local};
use crate::storage::file::now_ms;

/// Reads config files through `platform::local_path` (the only way to open local files).
#[derive(Debug, Default, Clone, Copy)]
pub struct TrustedSource;

impl ConfigSource for TrustedSource {
    fn read(&self, path: &Path, scope: ConfigScope) -> ReadOutcome {
        let kind = match scope {
            ConfigScope::User => FileKind::UserConfig,
            ConfigScope::System => FileKind::SystemConfig,
        };
        match read_local(path, kind) {
            Ok(bytes) => ReadOutcome::Ok(bytes),
            Err(LocalPathError::NotFound) => ReadOutcome::NotFound,
            Err(LocalPathError::TooLarge(_)) => ReadOutcome::TooLarge,
            Err(
                e @ (LocalPathError::UntrustedPath(_)
                | LocalPathError::Insecure(_)
                | LocalPathError::NotAFile),
            ) => {
                tracing::warn!(file = %path.display(), reason = %e, "ignoring untrusted ssh config file");
                ReadOutcome::Untrusted
            }
            Err(e) => ReadOutcome::Error(e.to_string()),
        }
    }

    fn path_allowed(&self, path: &Path) -> bool {
        config_path_allowed(path)
    }
}

/// The system ssh directory: `/etc/ssh` (Unix) or `%ProgramData%\ssh` (Windows).
pub fn system_ssh_dir() -> PathBuf {
    if cfg!(windows) {
        std::env::var_os("ProgramData")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
            .join("ssh")
    } else {
        PathBuf::from("/etc/ssh")
    }
}

/// The production [`ConfigEnv`] for `home`. In an `e2e` test build the system ssh dir and the
/// environment lookup are redirected as well (src/e2e.rs).
pub fn production_env(home: PathBuf) -> ConfigEnv {
    let e2e = crate::e2e::active();
    let system = e2e.map_or_else(system_ssh_dir, |l| l.system_ssh_dir.clone());
    let system_config = system.join("ssh_config");
    let mut env = ConfigEnv::new(
        home,
        crate::platform::local_user_name(),
        system,
        system_config,
    );
    if let Some(l) = e2e {
        env.env = crate::e2e::env_lookup(l);
    }
    env
}

/// A source that opens nothing. Every file is
/// "not found", so the parsed config is empty and no file is read.
#[derive(Debug, Default, Clone, Copy)]
pub struct DisabledSource;

impl ConfigSource for DisabledSource {
    fn read(&self, _path: &Path, _scope: ConfigScope) -> ReadOutcome {
        ReadOutcome::NotFound
    }

    fn path_allowed(&self, _path: &Path) -> bool {
        false
    }
}

/// Loads and caches the parsed config; one instance per process.
pub struct SshConfigService {
    env: ConfigEnv,
    source: Arc<dyn ConfigSource>,
    cache: SshConfigCache,
    loaded_at_ms: AtomicI64,
    /// `connection.readSshConfig`.
    enabled: AtomicBool,
    /// Serialises [`Self::reload`]: a watcher reload that read the files before
    /// `readSshConfig` was turned off must not replace the empty config cached afterwards.
    reload_lock: Mutex<()>,
}

impl std::fmt::Debug for SshConfigService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshConfigService")
            .field("env", &self.env)
            .finish_non_exhaustive()
    }
}

impl SshConfigService {
    pub fn new(env: ConfigEnv) -> Self {
        Self::with_source(env, Arc::new(TrustedSource))
    }

    pub fn with_source(env: ConfigEnv, source: Arc<dyn ConfigSource>) -> Self {
        Self {
            env,
            source,
            cache: SshConfigCache::default(),
            loaded_at_ms: AtomicI64::new(0),
            enabled: AtomicBool::new(true),
            reload_lock: Mutex::new(()),
        }
    }

    pub fn env(&self) -> &ConfigEnv {
        &self.env
    }

    /// `false` = never read any config file (the parsed config is empty). Returns
    /// whether the flag changed; the caller then reloads and announces the new snapshot.
    pub fn set_enabled(&self, enabled: bool) -> bool {
        self.enabled.swap(enabled, Ordering::Relaxed) != enabled
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    fn active_source(&self) -> &dyn ConfigSource {
        if self.enabled() {
            self.source.as_ref()
        } else {
            &DisabledSource
        }
    }

    /// Re-read every config file (blocking). Returns the config and whether it changed.
    pub fn reload(&self) -> (Arc<ParsedConfig>, bool) {
        // A poisoned lock only means another reload panicked; the cache itself is consistent.
        let _serial = self
            .reload_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let parsed = super::load(self.active_source(), &self.env);
        let changed = self.cache.replace(parsed);
        if changed || self.loaded_at_ms.load(Ordering::Relaxed) == 0 {
            self.loaded_at_ms.store(now_ms(), Ordering::Relaxed);
        }
        let current = self
            .cache
            .get()
            .unwrap_or_else(|| Arc::new(super::load(self.active_source(), &self.env)));
        (current, changed)
    }

    /// The cached config, loading it on first use (blocking).
    pub fn current(&self) -> Arc<ParsedConfig> {
        match self.cache.get() {
            Some(c) => c,
            None => self.reload().0,
        }
    }

    /// Whether the snapshot lists `alias` (without building the snapshot).
    pub fn lists_host(&self, alias: &str) -> bool {
        self.current().lists_host(alias)
    }

    /// Snapshot of the cached config (`loadedAtMs` = time of the last content change).
    pub fn snapshot(&self) -> SshConfigSnapshot {
        let current = self.current();
        current.snapshot(self.loaded_at_ms.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use crate::platform::test_support::private_tempdir;

    #[test]
    fn trusted_source_reads_and_rejects() {
        let dir = private_tempdir();
        let cfg = dir.path().join("config");
        std::fs::write(&cfg, "Host web\n  HostName web.example.com\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        assert!(matches!(
            TrustedSource.read(&cfg, ConfigScope::User),
            ReadOutcome::Ok(_)
        ));
        assert_eq!(
            TrustedSource.read(&dir.path().join("missing"), ConfigScope::User),
            ReadOutcome::NotFound
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o664)).unwrap();
            assert_eq!(
                TrustedSource.read(&cfg, ConfigScope::User),
                ReadOutcome::Untrusted
            );
        }
        #[cfg(windows)]
        {
            crate::platform::test_support::set_dacl(&cfg, "D:P(A;;FA;;;WD)");
            assert_eq!(
                TrustedSource.read(&cfg, ConfigScope::User),
                ReadOutcome::Untrusted
            );
            assert!(!TrustedSource.path_allowed(Path::new(r"\\server\share\config")));
        }
    }

    #[test]
    fn service_reload_detects_changes() {
        let dir = private_tempdir();
        let home = dir.path().to_path_buf();
        std::fs::create_dir(home.join(".ssh")).unwrap();
        let env = ConfigEnv {
            system_config: home.join("no-system-config"),
            ..ConfigEnv::new(
                home.clone(),
                "alice".into(),
                home.join("etc"),
                home.join("etc/ssh_config"),
            )
        };
        let svc = SshConfigService::new(env);
        assert!(svc.snapshot().hosts.is_empty());
        let cfg = home.join(".ssh/config");
        std::fs::write(&cfg, "Host web\n  HostName web.example.com\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let (_, changed) = svc.reload();
        assert!(changed);
        let snap = svc.snapshot();
        assert_eq!(snap.hosts.len(), 1);
        assert!(snap.loaded_at_ms > 0);
        let (_, changed) = svc.reload();
        assert!(!changed);
    }

    /// `readSshConfig = false` reads no file at all (counted by the source) and
    /// yields an empty snapshot; turning it on again reads the files.
    #[test]
    fn disabled_service_reads_nothing() {
        use std::sync::atomic::AtomicUsize;
        struct Counting(Arc<AtomicUsize>);
        impl ConfigSource for Counting {
            fn read(&self, _path: &Path, _scope: ConfigScope) -> ReadOutcome {
                self.0.fetch_add(1, Ordering::SeqCst);
                ReadOutcome::Ok(b"Host web\n  HostName web.example.com\n".to_vec())
            }
            fn path_allowed(&self, _path: &Path) -> bool {
                true
            }
        }
        let reads = Arc::new(AtomicUsize::new(0));
        let dir = private_tempdir();
        let home = dir.path().to_path_buf();
        let env = ConfigEnv::new(
            home.clone(),
            "alice".into(),
            home.join("etc"),
            home.join("etc/ssh_config"),
        );
        let svc = SshConfigService::with_source(env, Arc::new(Counting(reads.clone())));
        assert!(svc.set_enabled(false));
        assert!(!svc.set_enabled(false), "no change");
        assert!(svc.snapshot().hosts.is_empty());
        svc.reload();
        assert_eq!(reads.load(Ordering::SeqCst), 0, "nothing was read");
        assert!(
            svc.current()
                .query("web", svc.env())
                .is_ok_and(|c| c.hostname.is_none())
        );
        assert!(svc.set_enabled(true));
        let (_, changed) = svc.reload();
        assert!(changed);
        assert!(reads.load(Ordering::SeqCst) > 0);
        assert_eq!(svc.snapshot().hosts.len(), 1);
    }

    /// Manual check ("real ~/.ssh/config loads"): read-only parse of the real
    /// user and system config through the production source. Prints counts and warning codes
    /// only — never host names or paths. Run with `cargo test real_config -- --ignored --nocapture`.
    #[test]
    #[ignore = "reads the developer's real ssh config (read-only)"]
    fn real_config_loads() {
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .map(std::path::PathBuf::from)
            .unwrap();
        let svc = SshConfigService::new(production_env(home));
        let parsed = svc.current();
        let snap = svc.snapshot();
        let codes: Vec<String> = snap
            .warnings
            .iter()
            .map(|w| serde_json::to_value(&w.kind).unwrap()["code"].to_string())
            .collect();
        println!(
            "files read: {}, includes: {}, hosts: {}, warnings: {codes:?}",
            parsed.files_read().len(),
            parsed.include_count(),
            snap.hosts.len()
        );
        println!(
            "system config read: {}",
            parsed
                .files_read()
                .iter()
                .any(|f| f == &svc.env().system_config)
        );
    }
}
