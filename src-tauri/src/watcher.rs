//! ssh config watcher (§7.5): `notify_debouncer_mini` (500 ms) watches, non-recursively, the
//! parent directory of every config file read, the directory part of every `Include` pattern
//! (even without matches), the user config directory and `~` (to notice `~/.ssh` being created).
//! Relevant events → re-parse → `on_change(snapshot)` (the app emits `ssh-config:changed`) if
//! the parsed config differs. The watched set is recomputed after each parse (≤ 64 dirs).
//! A start failure is logged; manual reload (`ssh_config_reload`) still works.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{DebounceEventResult, Debouncer, new_debouncer};

use crate::ipc::dto::SshConfigSnapshot;
use crate::sshconfig::service::SshConfigService;

pub const DEBOUNCE: Duration = Duration::from_millis(500);

/// [`relevant_any`] with a single spelling of `~` and `~/.ssh`.
#[cfg(test)]
fn relevant(
    path: &Path,
    dirs: &BTreeSet<PathBuf>,
    home: &Path,
    home_is_config_dir: bool,
    ssh_dir: &Path,
) -> bool {
    relevant_any(
        path,
        dirs,
        &[home.to_path_buf()],
        home_is_config_dir,
        &[ssh_dir.to_path_buf()],
    )
}

/// `path` plus its canonical form (if it exists and differs). Backends may report events with
/// canonical paths — macOS FSEvents resolves `/var` → `/private/var`.
fn with_canonical(path: &Path) -> Vec<PathBuf> {
    let mut out = vec![path.to_path_buf()];
    if let Ok(c) = std::fs::canonicalize(path)
        && c != path
    {
        out.push(c);
    }
    out
}

/// Is an event at `path` relevant? Everything inside a watched directory counts, except in `~`
/// (only `~/.ssh` itself matters there, unless `~` is also an Include/config directory), so
/// busy home directories (shell history, registry hives) do not trigger re-parses.
/// `dirs`, `homes` and `ssh_dirs` hold every spelling (raw and canonical) of those paths: the
/// `~` exclusion must hold for each spelling of `~`.
fn relevant_any(
    path: &Path,
    dirs: &BTreeSet<PathBuf>,
    homes: &[PathBuf],
    home_is_config_dir: bool,
    ssh_dirs: &[PathBuf],
) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    if !dirs.contains(parent) {
        return dirs.contains(path); // the watched directory itself was created/removed
    }
    if !home_is_config_dir && homes.iter().any(|h| h == parent) {
        return ssh_dirs.iter().any(|s| s == path);
    }
    true
}

/// What a re-parse depends on, cheaply: existence, size, mtime (and inode on Unix) of every
/// file read, every `Include` directory, every watched directory and both main config paths.
type Fingerprint = Vec<(PathBuf, Option<(u64, Option<std::time::SystemTime>, u64)>)>;

/// Notify 8 adds `IN_OPEN` to every inotify watch and the mini debouncer forwards
/// `Access(Open)` events, so the re-parse (which opens `~/.ssh/config`) triggered itself about
/// twice a second, forever. Opening a file changes none of the values compared here, so the
/// watcher re-parses only when this fingerprint differs.
fn fingerprint(
    parsed: &crate::sshconfig::ParsedConfig,
    wanted: &[PathBuf],
    mains: &[&Path],
) -> Fingerprint {
    let paths = parsed
        .files_read()
        .iter()
        .chain(parsed.include_dirs.iter())
        .chain(wanted.iter())
        .map(PathBuf::as_path)
        .chain(mains.iter().copied());
    paths
        .map(|p| {
            let meta = std::fs::metadata(p).ok().map(|m| {
                #[cfg(unix)]
                let ino = std::os::unix::fs::MetadataExt::ino(&m);
                #[cfg(not(unix))]
                let ino = 0;
                (m.len(), m.modified().ok(), ino)
            });
            (p.to_path_buf(), meta)
        })
        .collect()
}

struct WatchState {
    debouncer: Debouncer<RecommendedWatcher>,
    watched: BTreeSet<PathBuf>,
}

impl WatchState {
    fn update(&mut self, wanted: &[PathBuf]) {
        let wanted: BTreeSet<PathBuf> = wanted.iter().filter(|d| d.is_dir()).cloned().collect();
        for gone in self
            .watched
            .difference(&wanted)
            .cloned()
            .collect::<Vec<_>>()
        {
            let _ = self.debouncer.watcher().unwatch(&gone);
            self.watched.remove(&gone);
        }
        for new in wanted
            .difference(&self.watched)
            .cloned()
            .collect::<Vec<_>>()
        {
            match self
                .debouncer
                .watcher()
                .watch(&new, RecursiveMode::NonRecursive)
            {
                Ok(()) => {
                    self.watched.insert(new);
                }
                Err(e) => {
                    tracing::debug!(dir = %new.display(), error = %e, "cannot watch directory")
                }
            }
        }
    }
}

/// Start the watcher thread. It lives for the rest of the process.
pub fn start(
    service: Arc<SshConfigService>,
    on_change: impl Fn(SshConfigSnapshot) + Send + 'static,
) -> Result<(), String> {
    start_with(service, on_change, || {})
}

/// [`start`] with a hook called on every re-parse (tests count them).
fn start_with(
    service: Arc<SshConfigService>,
    on_change: impl Fn(SshConfigSnapshot) + Send + 'static,
    on_reload: impl Fn() + Send + 'static,
) -> Result<(), String> {
    let (tx, rx) = mpsc::channel::<DebounceEventResult>();
    let debouncer = new_debouncer(DEBOUNCE, tx).map_err(|e| e.to_string())?;
    let home = service.env().home.clone();
    let ssh_dir = service.env().user_ssh_dir.clone();
    let user_config = service.env().user_config.clone();
    let system_config = service.env().system_config.clone();
    std::thread::Builder::new()
        .name("ssh-config-watcher".into())
        .spawn(move || {
            // `~` is a real config directory only if a config file or an Include pattern lives there.
            let home_is_config_dir = |p: &crate::sshconfig::ParsedConfig| {
                p.files_read()
                    .iter()
                    .any(|f| f.parent() == Some(home.as_path()))
                    || p.include_dirs.contains(&home)
            };
            let mut state = WatchState {
                debouncer,
                watched: BTreeSet::new(),
            };
            let mains = [user_config.as_path(), system_config.as_path()];
            let current = service.current();
            let mut wanted = current.watch_dirs();
            let mut home_cfg = home_is_config_dir(&current);
            let mut last_fp = fingerprint(&current, &wanted, &mains);
            let mut last_parsed = current;
            if wanted.len() >= crate::sshconfig::MAX_WATCH_DIRS {
                tracing::warn!(
                    "too many ssh config directories; only manual reload covers the rest"
                );
            }
            state.update(&wanted);
            while let Ok(result) = rx.recv() {
                let events = match result {
                    Ok(events) => events,
                    Err(e) => {
                        tracing::warn!(error = %e, "ssh config watcher error");
                        continue;
                    }
                };
                let dirs: BTreeSet<PathBuf> =
                    wanted.iter().flat_map(|d| with_canonical(d)).collect();
                let homes = with_canonical(&home);
                let ssh_dirs = with_canonical(&ssh_dir);
                if !events
                    .iter()
                    .any(|e| relevant_any(&e.path, &dirs, &homes, home_cfg, &ssh_dirs))
                {
                    continue;
                }
                // Only re-parse if something the parse depends on changed (an `open` of
                // a config file, including our own re-parse, changes nothing).
                let before = fingerprint(&last_parsed, &wanted, &mains);
                if before == last_fp {
                    continue;
                }
                on_reload();
                let (parsed, changed) = service.reload();
                if changed {
                    tracing::info!("ssh config changed on disk");
                    on_change(service.snapshot());
                }
                wanted = parsed.watch_dirs();
                home_cfg = home_is_config_dir(&parsed);
                state.update(&wanted);
                let after = fingerprint(&parsed, &wanted, &mains);
                // A file that changed while it was being re-parsed: keep no fingerprint, so the
                // next event (its own, or the open by this re-parse) parses again.
                let raced = after
                    .iter()
                    .any(|(p, m)| before.iter().any(|(q, n)| p == q && m != n));
                last_fp = if raced { Vec::new() } else { after };
                last_parsed = parsed;
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::platform::test_support::private_tempdir;
    use crate::sshconfig::ConfigEnv;

    #[test]
    fn relevance_filter() {
        let home = PathBuf::from("/home/alice");
        let ssh = home.join(".ssh");
        let confd = ssh.join("conf.d");
        let dirs: BTreeSet<PathBuf> = [home.clone(), ssh.clone(), confd.clone()]
            .into_iter()
            .collect();
        assert!(relevant(&ssh.join("config"), &dirs, &home, false, &ssh));
        assert!(relevant(&confd.join("x.conf"), &dirs, &home, false, &ssh));
        assert!(relevant(&ssh, &dirs, &home, false, &ssh)); // ~/.ssh created
        assert!(!relevant(
            &home.join(".bash_history"),
            &dirs,
            &home,
            false,
            &ssh
        ));
        assert!(relevant(&home.join("extra.conf"), &dirs, &home, true, &ssh));
        assert!(!relevant(Path::new("/tmp/x"), &dirs, &home, false, &ssh));
    }

    /// Events reported with the canonical path (macOS: `/var` → `/private/var`) are
    /// matched against watched directories configured via a non-canonical path.
    #[cfg(unix)]
    #[test]
    fn canonical_event_paths_are_relevant() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real-home");
        std::fs::create_dir_all(real.join(".ssh")).unwrap();
        let alias = tmp.path().join("alias-home");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        let home = alias.clone();
        let ssh = alias.join(".ssh");
        let dirs: BTreeSet<PathBuf> = [home.clone(), ssh.clone()]
            .iter()
            .flat_map(|d| with_canonical(d))
            .collect();
        let homes = with_canonical(&home);
        let ssh_dirs = with_canonical(&ssh);
        let canonical_event = std::fs::canonicalize(&real).unwrap().join(".ssh/config");
        assert!(!relevant(
            &canonical_event,
            &[home.clone(), ssh.clone()].into_iter().collect(),
            &home,
            false,
            &ssh
        ));
        assert!(relevant_any(
            &canonical_event,
            &dirs,
            &homes,
            false,
            &ssh_dirs
        ));
        let noise = std::fs::canonicalize(&real).unwrap().join(".bash_history");
        assert!(!relevant_any(&noise, &dirs, &homes, false, &ssh_dirs));
        assert!(!relevant_any(
            &alias.join(".bash_history"),
            &dirs,
            &homes,
            false,
            &ssh_dirs
        ));
        let canonical_ssh = std::fs::canonicalize(&real).unwrap().join(".ssh");
        assert!(relevant_any(
            &canonical_ssh,
            &dirs,
            &homes,
            false,
            &ssh_dirs
        ));
    }

    /// Reading the config files (what every re-parse and every connect does) never causes a
    /// re-parse, so the watcher is idle while nothing changes (Linux: inotify `IN_OPEN` events);
    /// a real edit still re-parses.
    #[test]
    fn opening_config_files_does_not_reparse() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let dir = private_tempdir();
        let home = dir.path().to_path_buf();
        let ssh = home.join(".ssh");
        std::fs::create_dir(&ssh).unwrap();
        let cfg = ssh.join("config");
        let write = |text: &str| {
            std::fs::write(&cfg, text).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o600)).unwrap();
            }
        };
        write("Host one\n");
        let env = ConfigEnv::new(
            home.clone(),
            "alice".into(),
            home.join("absent"),
            home.join("absent/ssh_config"),
        );
        let service = Arc::new(SshConfigService::new(env));
        service.reload();
        let reloads = Arc::new(AtomicUsize::new(0));
        let counter = reloads.clone();
        let (tx, rx) = mpsc::channel();
        start_with(
            service.clone(),
            move |snap| {
                let _ = tx.send(snap);
            },
            move || {
                counter.fetch_add(1, Ordering::SeqCst);
            },
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(300)); // let the watches settle
        // Reads like a connect / manual reload does them.
        for _ in 0..3 {
            let _ = std::fs::read(&cfg).unwrap();
            service.reload();
        }
        std::thread::sleep(Duration::from_secs(3));
        assert_eq!(reloads.load(Ordering::SeqCst), 0, "idle watcher re-parsed");

        write("Host one\nHost two\n");
        let snap = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("no ssh-config change reported");
        assert_eq!(snap.hosts.len(), 2);
        // Settles: its own re-parse does not start another one.
        std::thread::sleep(Duration::from_secs(2));
        let n = reloads.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_secs(2));
        assert_eq!(reloads.load(Ordering::SeqCst), n, "re-parse loop");
    }

    /// Edits emit `ssh-config:changed` within 1 s (debounce 500 ms).
    #[test]
    fn edit_is_reported_within_a_second() {
        let dir = private_tempdir();
        let home = dir.path().to_path_buf();
        let ssh = home.join(".ssh");
        std::fs::create_dir(&ssh).unwrap();
        let cfg = ssh.join("config");
        let write = |text: &str| {
            std::fs::write(&cfg, text).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o600)).unwrap();
            }
        };
        write("Host one\n");
        let env = ConfigEnv {
            system_config: home.join("absent/ssh_config"),
            ..ConfigEnv::new(
                home.clone(),
                "alice".into(),
                home.join("absent"),
                home.join("absent/ssh_config"),
            )
        };
        let service = Arc::new(SshConfigService::new(env));
        service.reload();
        let (tx, rx) = mpsc::channel();
        start(service, move |snap| {
            let _ = tx.send(snap);
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(300)); // let the watches settle
        let t0 = std::time::Instant::now();
        write("Host one\nHost two\n");
        let snap = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("no ssh-config change reported");
        let elapsed = t0.elapsed();
        assert_eq!(snap.hosts.len(), 2);
        assert!(elapsed < Duration::from_millis(1000), "took {elapsed:?}");
    }
}
