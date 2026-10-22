//! Own `ssh_config` parser and resolver (§2.4, §7.4).
//!
//! - [`load`] reads the user config and then the system config (and their `Include`s) through a
//!   [`ConfigSource`]. The production source is `platform::local_path` + `file_trust` (strict path
//!   allowlist, trust check on the opened handle, 4 MiB cap); tests use [`StdFsSource`].
//! - [`ParsedConfig::query`] resolves the options for one alias with OpenSSH semantics:
//!   first obtained value wins (user file before system file), `IdentityFile`/`CertificateFile`
//!   accumulate, `Host` patterns and a `Match` subset select blocks, `Include` is conditional.
//! - [`ParsedConfig::snapshot`] enumerates the concrete aliases for the UI (§7.3).
//!
//! Nothing is ever executed (`Match exec` and friends make the block not match, with a warning).

#![deny(clippy::indexing_slicing)]

mod hosts;
mod lexer;
mod parser;
mod pattern;
mod resolve;
pub mod service;
mod tokens;
pub mod validate;

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};

use crate::ipc::dto::{ConfigWarning, SourceLocation};

pub use resolve::{
    AgentSetting, HostConfig, IdentityFileOpt, KnownHostsFileOpt, ProxyJumpOpt, QueryError,
    QueryOverrides, StrictMode,
};

/// Maximum `Include` nesting depth (OpenSSH `READCONF_MAX_DEPTH`): depth 16 is allowed, 17 is not.
pub const MAX_INCLUDE_DEPTH: usize = 16;
/// Maximum number of config files read in total (main files plus every `Include`d file).
pub const MAX_CONFIG_FILES: usize = 256;
/// Maximum number of directories handed to the watcher (§7.5).
pub const MAX_WATCH_DIRS: usize = 64;
/// Size cap for one config file (§9.4); enforced by the production [`ConfigSource`].
pub const MAX_CONFIG_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// Which config a file belongs to. Included files inherit the scope of the including file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConfigScope {
    /// `~/.ssh/config` and its includes (relative includes resolve against `~/.ssh`).
    User,
    /// `/etc/ssh/ssh_config` / `%ProgramData%\ssh\ssh_config` (relative includes resolve
    /// against the system ssh directory).
    System,
}

/// Result of reading one config file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadOutcome {
    Ok(Vec<u8>),
    /// Silently ignored (a missing main file yields an empty snapshot).
    NotFound,
    /// Failed the path allowlist or the §7.4.1 trust check ⇒ `untrustedFile` warning.
    Untrusted,
    /// Larger than [`MAX_CONFIG_FILE_BYTES`] ⇒ `fileTooLarge` warning.
    TooLarge,
    /// Any other I/O error ⇒ `untrustedFile` warning (the file is ignored).
    Error(String),
}

/// Where config files come from. Implemented by `platform` in production.
pub trait ConfigSource: Send + Sync {
    /// Read one config file (after the §9.4 allowlist and the §7.4.1 trust check for `scope`).
    fn read(&self, path: &Path, scope: ConfigScope) -> ReadOutcome;
    /// §9.4 drive-letter allowlist for `Config`-source paths (Windows; `true` elsewhere).
    /// Applied to every `Include` pattern and every glob match **before** globbing/stat.
    fn path_allowed(&self, path: &Path) -> bool;
}

/// Plain `std::fs` source without trust checks (tests, tools). Enforces the size cap.
#[derive(Debug, Clone, Default)]
pub struct StdFsSource;

impl ConfigSource for StdFsSource {
    fn read(&self, path: &Path, _scope: ConfigScope) -> ReadOutcome {
        match std::fs::metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ReadOutcome::NotFound,
            Err(e) => return ReadOutcome::Error(e.to_string()),
            Ok(m) if !m.is_file() => return ReadOutcome::Untrusted,
            Ok(m) if m.len() > MAX_CONFIG_FILE_BYTES => return ReadOutcome::TooLarge,
            Ok(_) => {}
        }
        match std::fs::read(path) {
            Ok(bytes) => ReadOutcome::Ok(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => ReadOutcome::NotFound,
            Err(e) => ReadOutcome::Error(e.to_string()),
        }
    }

    fn path_allowed(&self, _path: &Path) -> bool {
        true
    }
}

/// Environment variable lookup (`${VAR}` tokens). Injected so tests are hermetic.
pub type EnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Local facts needed for parsing and token expansion.
#[derive(Clone)]
pub struct ConfigEnv {
    /// `~` and `%d`.
    pub home: PathBuf,
    /// `%u`, the default remote user, and `Match localuser`.
    pub local_user: String,
    /// `~/.ssh` — base for relative `Include`s in user files.
    pub user_ssh_dir: PathBuf,
    /// `/etc/ssh` or `%ProgramData%\ssh` — base for relative `Include`s in system files.
    pub system_ssh_dir: PathBuf,
    /// `~/.ssh/config`.
    pub user_config: PathBuf,
    /// `/etc/ssh/ssh_config` or `%ProgramData%\ssh\ssh_config`.
    pub system_config: PathBuf,
    /// `${VAR}` lookup.
    pub env: EnvLookup,
}

impl ConfigEnv {
    /// Conventional layout below `home` with the process environment.
    pub fn new(
        home: PathBuf,
        local_user: String,
        system_ssh_dir: PathBuf,
        system_config: PathBuf,
    ) -> Self {
        let user_ssh_dir = home.join(".ssh");
        Self {
            user_config: user_ssh_dir.join("config"),
            user_ssh_dir,
            home,
            local_user,
            system_ssh_dir,
            system_config,
            env: Arc::new(|name| std::env::var(name).ok()),
        }
    }
}

impl fmt::Debug for ConfigEnv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConfigEnv")
            .field("home", &self.home)
            .field("local_user", &self.local_user)
            .field("user_ssh_dir", &self.user_ssh_dir)
            .field("system_ssh_dir", &self.system_ssh_dir)
            .field("user_config", &self.user_config)
            .field("system_config", &self.system_config)
            .finish_non_exhaustive()
    }
}

/// One parsed config file (main or included).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParsedFile {
    pub path: PathBuf,
    pub scope: ConfigScope,
    pub items: Vec<parser::Item>,
}

/// The result of [`load`]: every file that was read, as a tree of items, plus load warnings.
/// Resolution ([`ParsedConfig::query`]) is done on demand per alias.
#[derive(Debug, Clone)]
pub struct ParsedConfig {
    pub(crate) user: Option<ParsedFile>,
    pub(crate) system: Option<ParsedFile>,
    /// Warnings found while loading (bad lines, include problems, untrusted files, Match).
    pub(crate) warnings: Vec<ConfigWarning>,
    /// `badLine` warnings left out of `warnings` by the per-file cap (all files together).
    pub(crate) dropped_warnings: u32,
    /// Every file that was read successfully, in read order.
    pub(crate) files_read: Vec<PathBuf>,
    /// Directory part of every `Include` pattern (watched even without matches).
    pub(crate) include_dirs: Vec<PathBuf>,
    pub(crate) env: ConfigEnv,
}

/// Equality ignores the environment (used to decide whether a reload changed anything).
impl PartialEq for ParsedConfig {
    fn eq(&self, other: &Self) -> bool {
        self.user == other.user
            && self.system == other.system
            && self.warnings == other.warnings
            && self.dropped_warnings == other.dropped_warnings
            && self.files_read == other.files_read
            && self.include_dirs == other.include_dirs
    }
}

/// Read and parse the user config, then the system config, with all `Include`s.
pub fn load(source: &dyn ConfigSource, env: &ConfigEnv) -> ParsedConfig {
    let mut loader = parser::Loader::new(source, env);
    let user = loader.load_main(&env.user_config, ConfigScope::User);
    let system = loader.load_main(&env.system_config, ConfigScope::System);
    let loaded = loader.finish();
    ParsedConfig {
        user,
        system,
        warnings: loaded.warnings,
        dropped_warnings: loaded.dropped_warnings,
        files_read: loaded.files_read,
        include_dirs: loaded.include_dirs,
        env: env.clone(),
    }
}

impl ParsedConfig {
    /// Load-time warnings (resolution warnings are attached to snapshot hosts / query results).
    pub fn load_warnings(&self) -> &[ConfigWarning] {
        &self.warnings
    }

    /// Every config file that was read, in read order.
    pub fn files_read(&self) -> &[PathBuf] {
        &self.files_read
    }

    /// Resolve the options for `alias` (the host as typed). See [`HostConfig`].
    pub fn query(&self, alias: &str, env: &ConfigEnv) -> Result<HostConfig, QueryError> {
        self.query_with(alias, env, QueryOverrides::default())
    }

    /// [`Self::query`] with the connection's own user/port applied first: they feed
    /// `Match user`, `%r` and `%p` like OpenSSH's command-line values.
    pub fn query_with(
        &self,
        alias: &str,
        env: &ConfigEnv,
        overrides: QueryOverrides<'_>,
    ) -> Result<HostConfig, QueryError> {
        resolve::resolve(self, alias, env, overrides)
            .map(|r| r.config)
            .map_err(|e| e.error)
    }

    /// Directories the watcher should observe (§7.5): the parent directory of every file read,
    /// the directory part of every `Include` pattern (even without matches), the directory of
    /// the user config (even if it does not exist yet) and `~`. Deduplicated, at most
    /// [`MAX_WATCH_DIRS`].
    pub fn watch_dirs(&self) -> Vec<PathBuf> {
        let mut seen = HashSet::new();
        let mut dirs = Vec::new();
        let mut push = |p: Option<&Path>| {
            if let Some(p) = p
                && !p.as_os_str().is_empty()
                && seen.insert(p.to_path_buf())
            {
                dirs.push(p.to_path_buf());
            }
        };
        push(self.env.user_config.parent());
        for f in &self.files_read {
            push(f.parent());
        }
        for d in &self.include_dirs {
            push(Some(d));
        }
        push(Some(&self.env.home));
        dirs.truncate(MAX_WATCH_DIRS);
        dirs
    }

    /// Number of `Include`d files that were read (main files not counted).
    pub fn include_count(&self) -> u32 {
        let mains = [&self.user, &self.system]
            .iter()
            .filter(|f| f.is_some())
            .count();
        u32::try_from(self.files_read.len().saturating_sub(mains)).unwrap_or(u32::MAX)
    }

    /// Number of hosts listed in the snapshot.
    pub fn host_count(&self) -> u32 {
        u32::try_from(self.snapshot(0).hosts.len()).unwrap_or(u32::MAX)
    }
}

pub(crate) fn location(path: &Path, line: usize) -> SourceLocation {
    SourceLocation {
        file: path.display().to_string(),
        line: u32::try_from(line).unwrap_or(u32::MAX),
    }
}

/// Holds the most recently loaded config (`AppState.ssh_config`).
#[derive(Debug, Default)]
pub struct SshConfigCache {
    current: RwLock<Option<Arc<ParsedConfig>>>,
}

impl SshConfigCache {
    pub fn get(&self) -> Option<Arc<ParsedConfig>> {
        self.current
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Store a freshly loaded config. Returns `true` if it differs from the previous one.
    pub fn replace(&self, parsed: ParsedConfig) -> bool {
        let mut guard = self.current.write().unwrap_or_else(PoisonError::into_inner);
        let changed = guard.as_deref() != Some(&parsed);
        *guard = Some(Arc::new(parsed));
        changed
    }
}

/// Shared test fixtures: a fake home inside a tempdir and helpers to write config files.
#[cfg(test)]
pub(crate) mod testutil {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use super::{ConfigEnv, ConfigScope, ConfigSource, ParsedConfig, ReadOutcome, StdFsSource};

    pub struct Fixture {
        /// Keeps the temporary directory alive for the test's duration.
        _tmp: tempfile::TempDir,
        pub env: ConfigEnv,
    }

    impl Fixture {
        pub fn new() -> Self {
            Self::with_env(&[])
        }

        pub fn with_env(vars: &[(&str, &str)]) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let home = dir.path().join("home").join("alice");
            let system_dir = dir.path().join("etc").join("ssh");
            std::fs::create_dir_all(home.join(".ssh")).unwrap();
            std::fs::create_dir_all(&system_dir).unwrap();
            let vars: HashMap<String, String> = vars
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect();
            let mut env = ConfigEnv::new(
                home,
                "alice".to_owned(),
                system_dir.clone(),
                system_dir.join("ssh_config"),
            );
            env.env = Arc::new(move |name| vars.get(name).cloned());
            Self { _tmp: dir, env }
        }

        pub fn home(&self) -> &Path {
            &self.env.home
        }

        pub fn ssh_dir(&self) -> PathBuf {
            self.env.user_ssh_dir.clone()
        }

        /// Write `~/.ssh/<rel>` (creating directories).
        pub fn user_file(&self, rel: &str, text: &str) -> PathBuf {
            // Native separators, so paths compare equal to glob results on Windows.
            let p = rel
                .split('/')
                .fold(self.env.user_ssh_dir.clone(), |acc, part| acc.join(part));
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, text).unwrap();
            p
        }

        pub fn user_config(&self, text: &str) -> PathBuf {
            self.user_file("config", text)
        }

        pub fn system_config(&self, text: &str) -> PathBuf {
            std::fs::write(&self.env.system_config, text).unwrap();
            self.env.system_config.clone()
        }

        pub fn load(&self) -> ParsedConfig {
            super::load(&StdFsSource, &self.env)
        }

        pub fn load_with(&self, source: &dyn ConfigSource) -> ParsedConfig {
            super::load(source, &self.env)
        }
    }

    /// A source that reports configured paths as untrusted / too large and denies paths
    /// containing a marker in `path_allowed`.
    pub struct PickySource {
        pub untrusted: Vec<PathBuf>,
        pub too_large: Vec<PathBuf>,
        pub disallow_marker: Option<String>,
    }

    impl ConfigSource for PickySource {
        fn read(&self, path: &Path, scope: ConfigScope) -> ReadOutcome {
            if self.untrusted.iter().any(|p| p == path) {
                return ReadOutcome::Untrusted;
            }
            if self.too_large.iter().any(|p| p == path) {
                return ReadOutcome::TooLarge;
            }
            StdFsSource.read(path, scope)
        }

        fn path_allowed(&self, path: &Path) -> bool {
            self.disallow_marker
                .as_deref()
                .is_none_or(|m| !path.to_string_lossy().contains(m))
        }
    }
}
