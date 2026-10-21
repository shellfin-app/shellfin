//! Local storage (§7): `bookmarks.json`, `settings.json`, `recents.json` in the private app
//! config directory. All functions are synchronous (blocking file I/O); commands call them inside
//! `spawn_blocking`.
//!
//! Loading never fails: every problem becomes a warning (`storageCorrupt`, `storageReadOnly`,
//! `io{readStorage|writeStorage}`) shown by the frontend from `AppBootstrap.warnings`, and the
//! store falls back to defaults. A file that cannot be *read* (I/O error other than "not found")
//! is treated as read-only so it is never overwritten.

pub mod bookmarks;
pub mod file;
pub mod migrations;
pub mod recents;
pub mod settings;

use std::cell::Cell;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde_json::Value;

use crate::error::{AppError, ErrorCode, IoOp};
use crate::ipc::dto::Settings;
use crate::storage::bookmarks::BookmarkStore;
use crate::storage::file::{
    LoadOutcome, corrupt_warning, ensure_dir, io_error, load_json, quarantine, read_json,
    write_json,
};
use crate::storage::migrations::{MigrateError, StoreFile, migrate};
use crate::storage::recents::RecentStore;

/// A storage document after reading, quarantine and migration.
#[derive(Debug)]
pub(crate) struct LoadedDoc {
    /// `None` → use defaults.
    pub value: Option<Value>,
    pub read_only: bool,
    pub warnings: Vec<AppError>,
}

thread_local! {
    /// Set while [`Stores::open`] loads the stores of a second app instance.
    static SECONDARY_INSTANCE: Cell<bool> = const { Cell::new(false) };
}

/// While alive, [`load_document`] on this thread changes nothing on disk and reports every
/// store read-only.
struct SecondaryScope;

impl SecondaryScope {
    fn enter() -> Self {
        SECONDARY_INSTANCE.set(true);
        Self
    }
}

impl Drop for SecondaryScope {
    fn drop(&mut self) {
        SECONDARY_INSTANCE.set(false);
    }
}

/// [`load_document`] for a second instance: read only, never quarantine or migrate. A file this
/// version would have to migrate or repair is not used (defaults are shown).
fn load_document_read_only(path: &Path, file: StoreFile) -> LoadedDoc {
    let value = match read_json(path) {
        Ok(Some(Ok(v))) => {
            let current = file.current_version();
            migrations::version_of(&v)
                .is_some_and(|ver| ver >= current)
                .then_some(v)
        }
        Ok(None | Some(Err(_))) => None,
        Err(e) => {
            return LoadedDoc {
                value: None,
                read_only: true,
                warnings: vec![io_error(IoOp::ReadStorage, path, &e)],
            };
        }
    };
    LoadedDoc {
        value,
        read_only: true,
        warnings: Vec::new(),
    }
}

pub(crate) fn load_document(path: &Path, file: StoreFile) -> LoadedDoc {
    if SECONDARY_INSTANCE.get() {
        return load_document_read_only(path, file);
    }
    let mut warnings = Vec::new();
    let loaded = match load_json(path) {
        Ok(LoadOutcome::Missing) => {
            return LoadedDoc {
                value: None,
                read_only: false,
                warnings,
            };
        }
        Ok(LoadOutcome::Corrupt {
            quarantined_as,
            reason,
        }) => {
            warnings.push(corrupt_warning(path, &quarantined_as, reason));
            return LoadedDoc {
                value: None,
                read_only: false,
                warnings,
            };
        }
        Ok(LoadOutcome::Loaded(v)) => v,
        Err(e) => {
            warnings.push(io_error(IoOp::ReadStorage, path, &e));
            return LoadedDoc {
                value: None,
                read_only: true,
                warnings,
            };
        }
    };
    match migrate(loaded, file, path) {
        Ok(m) => {
            if m.read_only {
                warnings.push(
                    AppError::new(ErrorCode::StorageReadOnly).with_detail(format!(
                        "{} was written by a newer version",
                        file.file_name()
                    )),
                );
            } else if m.migrated_from.is_some()
                && let Err(e) = write_json(path, &m.value)
            {
                warnings.push(io_error(IoOp::WriteStorage, path, &e));
            }
            LoadedDoc {
                value: Some(m.value),
                read_only: m.read_only,
                warnings,
            }
        }
        Err(MigrateError::Backup(e)) => {
            // Cannot back up before migrating: leave the file untouched.
            warnings.push(io_error(IoOp::WriteStorage, path, &e));
            LoadedDoc {
                value: None,
                read_only: true,
                warnings,
            }
        }
        Err(e) => {
            match quarantine(path) {
                Ok(q) => warnings.push(corrupt_warning(path, &q, e.to_string())),
                Err(io) => warnings.push(io_error(IoOp::WriteStorage, path, &io)),
            }
            LoadedDoc {
                value: None,
                read_only: false,
                warnings,
            }
        }
    }
}

/// All local stores of the app (`AppState.stores`).
#[derive(Debug)]
pub struct Stores {
    dir: PathBuf,
    pub bookmarks: Mutex<BookmarkStore>,
    pub recents: Mutex<RecentStore>,
    pub settings: Mutex<Settings>,
    /// Serializes settings writers: the file is written under this lock, so the
    /// `settings` lock is only held for a clone or a swap and readers on the async runtime never
    /// wait for disk I/O.
    settings_write: Mutex<()>,
    settings_read_only: bool,
    /// The single-instance lock (held for the lifetime of the stores), `None` in a second
    /// instance or where the file system cannot lock.
    _instance_lock: Option<File>,
    secondary: bool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Lock file in the config directory that marks the instance owning the stores.
pub const INSTANCE_LOCK_FILE: &str = "instance.lock";

enum InstanceLock {
    /// This process owns the stores.
    Primary(File),
    /// Another process (another app instance) holds the lock.
    Secondary,
    /// Locking is not possible here (file system without locks, I/O error): carry on as
    /// before, unguarded.
    Unavailable,
}

/// Take the exclusive single-instance lock (`<config dir>/instance.lock`, never truncated, Unix
/// 0600 without following links). Released by the OS when the process ends, also on a crash.
fn take_instance_lock(dir: &Path) -> InstanceLock {
    let path = dir.join(INSTANCE_LOCK_FILE);
    let mut opts = OpenOptions::new();
    opts.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = match opts.open(&path) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(error = %e, "cannot open the instance lock file; continuing unguarded");
            return InstanceLock::Unavailable;
        }
    };
    match file.try_lock() {
        Ok(()) => InstanceLock::Primary(file),
        Err(TryLockError::WouldBlock) => InstanceLock::Secondary,
        Err(TryLockError::Error(e)) => {
            tracing::warn!(error = %e, "cannot lock the instance lock file; continuing unguarded");
            InstanceLock::Unavailable
        }
    }
}

impl Stores {
    /// Open (or initialise) the stores in `config_dir`. Never fails; see the module docs.
    ///
    /// Single instance: the first process takes an exclusive lock on
    /// [`INSTANCE_LOCK_FILE`]. A second instance (the lock is held) opens every store
    /// **read-only** and changes nothing on disk (no migration, quarantine or repair), so the two
    /// can never overwrite each other's files; it reports one `storageReadOnly` warning.
    pub fn open(config_dir: &Path) -> (Self, Vec<AppError>) {
        let mut warnings = Vec::new();
        if let Err(e) = ensure_dir(config_dir) {
            warnings.push(io_error(IoOp::WriteStorage, config_dir, &e));
        }
        let (instance_lock, secondary) = match take_instance_lock(config_dir) {
            InstanceLock::Primary(f) => (Some(f), false),
            InstanceLock::Secondary => {
                tracing::warn!("another instance owns the app data; opening it read-only");
                warnings.push(
                    AppError::new(ErrorCode::StorageReadOnly)
                        .with_detail("another instance of the app is running"),
                );
                (None, true)
            }
            InstanceLock::Unavailable => (None, false),
        };
        let scope = secondary.then(SecondaryScope::enter);
        let (bookmarks, w) = BookmarkStore::load(config_dir);
        warnings.extend(w);
        let (recents, w) = RecentStore::load(config_dir);
        warnings.extend(w);
        let (loaded, w) = settings::load_full(config_dir);
        warnings.extend(w);
        drop(scope);
        let stores = Self {
            dir: config_dir.to_path_buf(),
            bookmarks: Mutex::new(bookmarks),
            recents: Mutex::new(recents),
            settings: Mutex::new(loaded.settings),
            settings_write: Mutex::new(()),
            settings_read_only: loaded.read_only || secondary,
            _instance_lock: instance_lock,
            secondary,
        };
        (stores, warnings)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Another app instance owns the stores; everything here is read-only.
    pub fn is_secondary_instance(&self) -> bool {
        self.secondary
    }

    pub fn lock_bookmarks(&self) -> MutexGuard<'_, BookmarkStore> {
        lock(&self.bookmarks)
    }

    pub fn lock_recents(&self) -> MutexGuard<'_, RecentStore> {
        lock(&self.recents)
    }

    pub fn settings(&self) -> Settings {
        lock(&self.settings).clone()
    }

    pub fn settings_read_only(&self) -> bool {
        self.settings_read_only
    }
    /// `bookmark_delete`: delete the bookmark and prune its recents entry, so the Welcome card
    /// does not point at a bookmark that no longer exists. Returns the delete outcome
    /// and the new recents list when it changed (for `recents:changed`).
    ///
    /// Once the bookmark is gone the delete has happened: the recents prune is best effort (a
    /// read-only or unwritable `recents.json` is logged), so the caller always gets the outcome
    /// and removes the keychain entries (§9.2).
    pub fn delete_bookmark(
        &self,
        id: &str,
        now_ms: i64,
    ) -> Result<
        (
            bookmarks::DeleteOutcome,
            Option<Vec<crate::ipc::dto::RecentEntry>>,
        ),
        AppError,
    > {
        let outcome = self.lock_bookmarks().delete(id, now_ms)?;
        let target = crate::ipc::dto::RecentTarget::Bookmark { id: id.to_owned() };
        let mut recents = self.lock_recents();
        let listed = recents.list().iter().any(|e| e.target == target);
        let list = if listed {
            match recents.remove_target(&target) {
                Ok(list) => Some(list),
                Err(e) => {
                    tracing::warn!(code = ?e.kind, "could not prune the recents entry of a deleted bookmark");
                    None
                }
            }
        } else {
            None
        };
        Ok((outcome, list))
    }

    /// `settings_update`: validate (§4.4 ranges), refuse when read-only, persist, then apply.
    pub fn update_settings(&self, new: Settings) -> Result<Settings, AppError> {
        let _writer = lock(&self.settings_write);
        settings::update(&self.dir, self.settings_read_only, &new)?;
        *lock(&self.settings) = new.clone();
        Ok(new)
    }

    /// `settings_reset`: write the defaults (same file version) and return them.
    pub fn reset_settings(&self) -> Result<Settings, AppError> {
        self.update_settings(settings::default_settings())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use crate::ipc::dto::RecentTarget;

    /// `settings()` (called on the async runtime) never waits for a settings write.
    #[test]
    fn settings_reads_do_not_wait_for_a_writer() {
        let tmp = tempfile::tempdir().unwrap();
        let (stores, _) = Stores::open(tmp.path());
        let writer = lock(&stores.settings_write); // a writer in the middle of its file write
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::scope(|s| {
            s.spawn(|| tx.send(stores.settings()).unwrap());
            assert!(
                rx.recv_timeout(std::time::Duration::from_secs(5)).is_ok(),
                "a reader was blocked by the writer"
            );
        });
        drop(writer);
        let mut s = stores.settings();
        s.files.show_hidden = true;
        assert!(stores.update_settings(s).unwrap().files.show_hidden);
        assert!(stores.settings().files.show_hidden);
    }

    #[test]
    fn reset_settings_writes_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let (stores, _) = Stores::open(tmp.path());
        let mut s = stores.settings();
        s.files.show_hidden = true;
        s.terminal.font_family = Some("Fira Code".into());
        stores.update_settings(s).unwrap();
        let reset = stores.reset_settings().unwrap();
        assert_eq!(reset, settings::default_settings());
        assert_eq!(stores.settings(), reset);
        assert_eq!(settings::load(tmp.path()).0, reset);
    }

    #[test]
    fn open_fresh_dir_and_roundtrip_all_stores() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("app.config");
        let (stores, w) = Stores::open(&dir);
        assert!(w.is_empty(), "{w:?}");
        assert!(dir.is_dir());
        stores
            .lock_bookmarks()
            .copy_from_ssh_config("web", 1)
            .unwrap();
        stores
            .lock_recents()
            .upsert_connected(
                RecentTarget::SshConfig {
                    alias: "web".into(),
                },
                "web".into(),
                "alice@web".into(),
                None,
                2,
            )
            .unwrap();
        let mut s = stores.settings();
        s.files.show_hidden = true;
        stores.update_settings(s.clone()).unwrap();
        let mut bad = s.clone();
        bad.connection.prompt_timeout_secs = 500;
        assert!(stores.update_settings(bad).is_err());
        assert_eq!(stores.settings(), s);
        drop(stores); // releases the single-instance lock

        let (again, w) = Stores::open(&dir);
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(again.lock_bookmarks().dto().bookmarks.len(), 1);
        assert_eq!(again.lock_recents().list().len(), 1);
        assert_eq!(again.settings(), s);
    }

    #[cfg(unix)]
    #[test]
    fn files_and_dir_are_private_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("cfg");
        let (stores, _) = Stores::open(&dir);
        stores
            .lock_bookmarks()
            .copy_from_ssh_config("web", 1)
            .unwrap();
        stores.update_settings(stores.settings()).unwrap();
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for f in ["bookmarks.json", "settings.json"] {
            assert_eq!(
                std::fs::metadata(dir.join(f)).unwrap().permissions().mode() & 0o777,
                0o600,
                "{f}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_file_is_never_overwritten() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return; // root can read anything; the scenario does not apply
        }
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("bookmarks.json");
        std::fs::write(&p, br#"{"version":1,"groups":[],"bookmarks":[]}"#).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();
        let (stores, w) = Stores::open(tmp.path());
        assert!(stores.lock_bookmarks().read_only());
        assert!(w.iter().any(|e| matches!(
            e.kind,
            ErrorCode::Io {
                op: IoOp::ReadStorage,
                ..
            }
        )));
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn version_without_step_is_quarantined() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("recents.json"),
            br#"{"version":0,"entries":[]}"#,
        )
        .unwrap();
        let (stores, w) = Stores::open(tmp.path());
        assert!(stores.lock_recents().list().is_empty());
        assert_eq!(
            serde_json::to_value(&w[0]).unwrap()["code"],
            "storageCorrupt"
        );
        assert!(!tmp.path().join("recents.json").exists());
    }

    /// Deleting a bookmark prunes its recents entry (and only then reports a list).
    #[test]
    fn delete_bookmark_prunes_recents() {
        use crate::ipc::dto::RecentTarget;
        let tmp = tempfile::tempdir().unwrap();
        let (stores, _) = Stores::open(tmp.path());
        let b = stores
            .lock_bookmarks()
            .copy_from_ssh_config("web", 1)
            .unwrap();
        let other = stores
            .lock_bookmarks()
            .copy_from_ssh_config("db", 1)
            .unwrap();
        let mut recents = stores.lock_recents();
        recents
            .upsert_connected(
                RecentTarget::Bookmark { id: b.id.clone() },
                "web".into(),
                "alice@web".into(),
                None,
                2,
            )
            .unwrap();
        recents
            .upsert_connected(
                RecentTarget::SshConfig { alias: "x".into() },
                "x".into(),
                "alice@x".into(),
                None,
                3,
            )
            .unwrap();
        drop(recents);
        let (_, list) = stores.delete_bookmark(&b.id, 4).unwrap();
        let list = list.expect("recents changed");
        assert_eq!(list.len(), 1);
        assert_eq!(
            list[0].target,
            RecentTarget::SshConfig { alias: "x".into() }
        );
        // Persisted across a reload.
        let (reopened, _) = Stores::open(tmp.path());
        assert_eq!(reopened.lock_recents().list().len(), 1);
        // A bookmark without a recents entry reports no change.
        let (_, list) = stores.delete_bookmark(&other.id, 5).unwrap();
        assert!(list.is_none());
    }

    /// While one instance owns the stores, a second one opens them read-only, refuses
    /// every write and changes nothing on disk (not even a corrupt file is quarantined). Once
    /// the first instance is gone, the next one owns them again.
    #[test]
    fn second_instance_is_read_only_and_touches_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("cfg");
        let (first, w) = Stores::open(&dir);
        assert!(w.is_empty(), "{w:?}");
        assert!(!first.is_secondary_instance());
        first
            .lock_bookmarks()
            .copy_from_ssh_config("web", 1)
            .unwrap();
        let mut s = first.settings();
        s.files.show_hidden = true;
        first.update_settings(s.clone()).unwrap();
        std::fs::write(dir.join("recents.json"), b"{broken").unwrap();
        let snapshot = |d: &Path| -> Vec<(String, Vec<u8>)> {
            let mut v: Vec<_> = std::fs::read_dir(d)
                .unwrap()
                .map(|e| {
                    let e = e.unwrap();
                    (
                        e.file_name().to_string_lossy().into_owned(),
                        std::fs::read(e.path()).unwrap_or_default(),
                    )
                })
                .collect();
            v.sort();
            v
        };
        let before = snapshot(&dir);

        let (second, w) = Stores::open(&dir);
        assert!(second.is_secondary_instance());
        assert!(
            w.iter().any(|e| e.kind == ErrorCode::StorageReadOnly),
            "{w:?}"
        );
        assert!(second.lock_bookmarks().read_only());
        assert!(second.lock_recents().read_only());
        assert!(second.settings_read_only());
        // It sees the data, but cannot change it.
        assert_eq!(second.lock_bookmarks().dto().bookmarks.len(), 1);
        assert_eq!(second.settings(), s);
        assert!(
            second
                .lock_bookmarks()
                .copy_from_ssh_config("db", 2)
                .is_err()
        );
        assert!(second.update_settings(s.clone()).is_err());
        assert!(
            second
                .lock_bookmarks()
                .take_orphaned_keychain_ids()
                .is_empty()
        );
        assert_eq!(
            snapshot(&dir),
            before,
            "the second instance wrote something"
        );

        // The first instance keeps working; after it is gone, the next one owns the stores.
        first
            .lock_bookmarks()
            .copy_from_ssh_config("db", 3)
            .unwrap();
        drop(second);
        drop(first);
        let (third, _) = Stores::open(&dir);
        assert!(!third.is_secondary_instance());
        assert_eq!(third.lock_bookmarks().dto().bookmarks.len(), 2);
        assert!(!third.lock_bookmarks().read_only());
    }

    /// A recents file that cannot be written does not stop a bookmark delete; the delete
    /// outcome (which drives the keychain cleanup) is still returned.
    #[test]
    fn delete_bookmark_survives_an_unwritable_recents_file() {
        let tmp = tempfile::tempdir().unwrap();
        let (stores, _) = Stores::open(tmp.path());
        let b = stores
            .lock_bookmarks()
            .copy_from_ssh_config("web", 1)
            .unwrap();
        stores
            .lock_recents()
            .upsert_connected(
                RecentTarget::Bookmark { id: b.id.clone() },
                "web".into(),
                "alice@web".into(),
                None,
                2,
            )
            .unwrap();
        drop(stores);
        // A newer version wrote recents.json: read-only here, every write fails.
        let path = tmp.path().join("recents.json");
        let mut doc: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        doc["version"] = Value::from(99);
        std::fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
        let (stores, _) = Stores::open(tmp.path());
        assert!(stores.lock_recents().read_only());
        assert_eq!(stores.lock_recents().list().len(), 1);
        let (outcome, list) = stores.delete_bookmark(&b.id, 3).unwrap();
        assert!(!outcome.had_password);
        assert!(list.is_none());
        assert!(stores.lock_bookmarks().get(&b.id).is_none());
    }

    /// Keychain entries of bookmarks dropped on load, and `passwordSaved`
    /// without a binding, are reported for deletion; the latter is normalised to "not saved".
    #[test]
    fn orphaned_keychain_ids_are_reported_on_load() {
        let tmp = tempfile::tempdir().unwrap();
        let doc = serde_json::json!({
            "version": 1,
            "groups": [],
            "bookmarks": [
                { "id": "ok1", "name": "ok", "host": "ok.example.com", "port": null, "user": null,
                  "auth": { "method": "password", "passwordSaved": true,
                            "passwordBinding": { "hostname": "ok.example.com", "port": 22, "user": "alice" } },
                  "proxyJump": null, "groupId": null, "tags": [], "startDir": null, "useSshConfig": false,
                  "createdAtMs": 1, "updatedAtMs": 1 },
                { "id": "nobind", "name": "nobind", "host": "nb.example.com", "port": null, "user": null,
                  "auth": { "method": "password", "passwordSaved": true },
                  "proxyJump": null, "groupId": null, "tags": [], "startDir": null, "useSshConfig": false,
                  "createdAtMs": 1, "updatedAtMs": 1 },
                { "id": "bad1", "name": "bad", "host": "bad host", "port": null, "user": null,
                  "auth": { "method": "password", "passwordSaved": true,
                            "passwordBinding": { "hostname": "x", "port": 22, "user": "alice" } },
                  "proxyJump": null, "groupId": null, "tags": [], "startDir": null, "useSshConfig": false,
                  "createdAtMs": 1, "updatedAtMs": 1 }
            ]
        });
        std::fs::write(
            tmp.path().join("bookmarks.json"),
            serde_json::to_vec(&doc).unwrap(),
        )
        .unwrap();
        let (stores, _) = Stores::open(tmp.path());
        let mut ids = stores.lock_bookmarks().take_orphaned_keychain_ids();
        ids.sort();
        assert_eq!(ids, vec!["bad1".to_owned(), "nobind".to_owned()]);
        let dto = stores.lock_bookmarks().dto();
        let nobind = dto.bookmarks.iter().find(|b| b.id == "nobind").unwrap();
        assert_eq!(
            nobind.auth,
            crate::ipc::dto::AuthConfig::Password {
                password_saved: false
            }
        );
        assert!(
            stores
                .lock_bookmarks()
                .take_orphaned_keychain_ids()
                .is_empty()
        );
    }

    /// A store written by a NEWER version is read-only; entries this version cannot
    /// read (or that look unbound to it) keep their keychain entries, and the file is untouched.
    #[test]
    fn read_only_store_never_reports_keychain_orphans() {
        let tmp = tempfile::tempdir().unwrap();
        let doc = serde_json::json!({
            "version": 3,
            "groups": [],
            "bookmarks": [
                { "id": "future1", "name": "f", "host": "f.example.com", "port": null, "user": null,
                  "auth": { "method": "passkey", "passwordSaved": true },
                  "proxyJump": null, "groupId": null, "tags": [], "startDir": null, "useSshConfig": false,
                  "createdAtMs": 1, "updatedAtMs": 1 },
                { "id": "nobind", "name": "nobind", "host": "nb.example.com", "port": null, "user": null,
                  "auth": { "method": "password", "passwordSaved": true },
                  "proxyJump": null, "groupId": null, "tags": [], "startDir": null, "useSshConfig": false,
                  "createdAtMs": 1, "updatedAtMs": 1 },
                { "id": "bad1", "name": "bad", "host": "bad host", "port": null, "user": null,
                  "auth": { "method": "password", "passwordSaved": true,
                            "passwordBinding": { "hostname": "x", "port": 22, "user": "alice" } },
                  "proxyJump": null, "groupId": null, "tags": [], "startDir": null, "useSshConfig": false,
                  "createdAtMs": 1, "updatedAtMs": 1 }
            ]
        });
        let path = tmp.path().join("bookmarks.json");
        std::fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
        let before = std::fs::read(&path).unwrap();
        let (stores, _) = Stores::open(tmp.path());
        assert!(stores.lock_bookmarks().read_only());
        assert!(
            stores
                .lock_bookmarks()
                .take_orphaned_keychain_ids()
                .is_empty()
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}
