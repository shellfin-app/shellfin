//! Recent connections (§7.3): at most 10, most recently connected first. Targets never
//! contain secrets (drafts are recorded as `quick{"user@host:port"}` by the caller).
//!
//! - [`RecentStore::upsert_connected`] — on the first `connected` of a session: insert or update
//!   (label, userAtHost, via, lastConnectedAtMs) and move to the front; keeps `lastDir` and
//!   `terminalCount` of an existing entry.
//! - [`RecentStore::record_close`] — on `session_close`: updates `lastDir` (only when the
//!   frontend sent one) and `terminalCount` of an existing entry; no-op if the entry was
//!   evicted meanwhile. Order is unchanged.
//!
//! Every mutation returns the full list for the `recents:changed` event. Invalid entries on load
//! are dropped like bookmarks (corrupt copy + `storageCorrupt`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{AppError, ErrorCode, IoOp};
use crate::ipc::dto::{RecentEntry, RecentTarget};
use crate::sshconfig::validate::{parse_host_spec, valid_hostname};
use crate::storage::file::{corrupt_warning, io_error, quarantine_copy, write_json};
use crate::storage::migrations::StoreFile;
use crate::storage::{LoadedDoc, load_document};

pub const MAX_RECENTS: usize = 10;
const MAX_TEXT_CHARS: usize = 300;
const MAX_PATH_CHARS: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredRecent {
    target: RecentTarget,
    label: String,
    user_at_host: String,
    via: Option<String>,
    last_connected_at_ms: i64,
    last_dir: Option<String>,
    terminal_count: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawFile {
    #[serde(default)]
    entries: Vec<Value>,
    /// Bookmark ids open at the last quit (no version bump: missing = none). Kept
    /// as a raw value so a malformed list never costs the entries.
    #[serde(default)]
    open_at_quit: Value,
    /// Updater: the version the user chose to skip (no version bump: missing = none).
    /// Raw for the same reason as `open_at_quit`.
    #[serde(default)]
    skipped_update_version: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileOut<'a> {
    version: u64,
    entries: &'a [StoredRecent],
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    open_at_quit: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    skipped_update_version: Option<&'a str>,
}

/// Most bookmark ids kept for startup restore.
pub const MAX_OPEN_AT_QUIT: usize = 64;
/// Longest version string accepted for "Skip this version" (SemVer text, hostile-file bound).
pub const MAX_VERSION_CHARS: usize = 64;

#[derive(Debug)]
pub struct RecentStore {
    path: PathBuf,
    read_only: bool,
    entries: Vec<StoredRecent>,
    /// Bookmark ids open at the last quit, in opening order.
    open_at_quit: Vec<String>,
    /// Updater: automatic update prompts for exactly this version are suppressed.
    skipped_update_version: Option<String>,
}

/// A plausible SemVer string (`1.2.3`, `0.2.0-experimental.1`, `1.0.0+build.5`): starts with a
/// digit, ASCII alphanumerics and `.-+` only, at most [`MAX_VERSION_CHARS`]. Exact SemVer parsing
/// is the updater's job; this only keeps hostile text out of the store.
pub fn valid_version_text(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= MAX_VERSION_CHARS
        && v.bytes().next().is_some_and(|b| b.is_ascii_digit())
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'))
}

fn valid_bookmark_id(id: &str) -> bool {
    valid_target(&RecentTarget::Bookmark { id: id.to_owned() })
}

/// Keep valid, distinct ids in order, at most [`MAX_OPEN_AT_QUIT`] (hostile file: anything
/// else is dropped silently).
fn clean_open_ids(ids: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in ids {
        if out.len() >= MAX_OPEN_AT_QUIT {
            break;
        }
        if valid_bookmark_id(&id) && !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

fn text_ok(s: &str) -> bool {
    s.chars().count() <= MAX_TEXT_CHARS && !s.chars().any(char::is_control)
}

/// A display text that passes [`text_ok`]: control characters dropped, cut to
/// [`MAX_TEXT_CHARS`] characters with a trailing `…`.
fn display_text(s: &str) -> String {
    let clean: Vec<char> = s.chars().filter(|c| !c.is_control()).collect();
    if clean.len() <= MAX_TEXT_CHARS {
        return clean.into_iter().collect();
    }
    let mut out: String = clean.into_iter().take(MAX_TEXT_CHARS - 1).collect();
    out.push('…');
    out
}

fn valid_target(t: &RecentTarget) -> bool {
    match t {
        RecentTarget::SshConfig { alias } => valid_hostname(alias),
        RecentTarget::Bookmark { id } => {
            !id.is_empty()
                && id.len() <= 64
                && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        }
        RecentTarget::Quick { spec } => parse_host_spec(spec).is_some(),
    }
}

fn valid_entry(e: &StoredRecent) -> bool {
    valid_target(&e.target)
        && text_ok(&e.label)
        && text_ok(&e.user_at_host)
        && e.via.as_deref().is_none_or(text_ok)
        && e.last_dir
            .as_deref()
            .is_none_or(|d| d.chars().count() <= MAX_PATH_CHARS && !d.contains('\0'))
}

fn to_dto(e: &StoredRecent) -> RecentEntry {
    RecentEntry {
        target: e.target.clone(),
        label: e.label.clone(),
        user_at_host: e.user_at_host.clone(),
        via: e.via.clone(),
        last_connected_at_ms: e.last_connected_at_ms,
        last_dir: e.last_dir.clone(),
        terminal_count: e.terminal_count,
    }
}

impl RecentStore {
    pub fn load(dir: &Path) -> (Self, Vec<AppError>) {
        let path = dir.join(StoreFile::Recents.file_name());
        let LoadedDoc {
            value,
            read_only,
            mut warnings,
        } = load_document(&path, StoreFile::Recents);
        let mut store = Self {
            path,
            read_only,
            entries: Vec::new(),
            open_at_quit: Vec::new(),
            skipped_update_version: None,
        };
        let Some(value) = value else {
            return (store, warnings);
        };
        let raw = serde_json::from_value::<RawFile>(value.clone()).unwrap_or(RawFile {
            entries: Vec::new(),
            open_at_quit: Value::Null,
            skipped_update_version: Value::Null,
        });
        store.skipped_update_version = raw
            .skipped_update_version
            .as_str()
            .filter(|v| valid_version_text(v))
            .map(str::to_owned);
        store.open_at_quit = clean_open_ids(
            raw.open_at_quit
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_owned)),
        );
        let total = raw.entries.len();
        let mut entries: Vec<StoredRecent> = Vec::new();
        for e in raw
            .entries
            .into_iter()
            .filter_map(|v| serde_json::from_value::<StoredRecent>(v).ok())
        {
            if valid_entry(&e) && !entries.iter().any(|k| k.target == e.target) {
                entries.push(e);
            }
        }
        entries.sort_by_key(|e| std::cmp::Reverse(e.last_connected_at_ms));
        let structurally_bad = value.get("entries").is_some_and(|e| !e.is_array());
        let dropped = total - entries.len().min(total) + usize::from(structurally_bad);
        entries.truncate(MAX_RECENTS);
        store.entries = entries;
        if dropped > 0 {
            let reason = format!("dropped {dropped} invalid entries");
            if store.read_only {
                warnings.push(AppError::new(ErrorCode::StorageReadOnly).with_detail(reason));
            } else {
                match quarantine_copy(
                    &store.path,
                    &serde_json::to_vec_pretty(&value).unwrap_or_default(),
                ) {
                    Ok(q) => warnings.push(corrupt_warning(&store.path, &q, reason)),
                    Err(e) => warnings.push(io_error(IoOp::WriteStorage, &store.path, &e)),
                }
                if let Err(e) = store.write(&store.entries) {
                    warnings.push(e);
                }
            }
        }
        (store, warnings)
    }

    fn write(&self, entries: &[StoredRecent]) -> Result<(), AppError> {
        self.write_all(entries, &self.open_at_quit)
    }

    fn write_all(&self, entries: &[StoredRecent], open_at_quit: &[String]) -> Result<(), AppError> {
        self.write_doc(
            entries,
            open_at_quit,
            self.skipped_update_version.as_deref(),
        )
    }

    fn write_doc(
        &self,
        entries: &[StoredRecent],
        open_at_quit: &[String],
        skipped_update_version: Option<&str>,
    ) -> Result<(), AppError> {
        let doc = FileOut {
            version: StoreFile::Recents.current_version(),
            entries,
            open_at_quit,
            skipped_update_version,
        };
        write_json(&self.path, &doc).map_err(|e| io_error(IoOp::WriteStorage, &self.path, &e))
    }

    fn commit(&mut self, entries: Vec<StoredRecent>) -> Result<Vec<RecentEntry>, AppError> {
        if self.read_only {
            return Err(AppError::new(ErrorCode::StorageReadOnly));
        }
        self.write(&entries)?;
        self.entries = entries;
        Ok(self.list())
    }

    /// The bookmark ids open at the last quit (opening order).
    pub fn open_at_quit(&self) -> &[String] {
        &self.open_at_quit
    }

    /// Record the open bookmark ids (cleaned like on load); one atomic write.
    pub fn set_open_at_quit(&mut self, ids: Vec<String>) -> Result<(), AppError> {
        if self.read_only {
            return Err(AppError::new(ErrorCode::StorageReadOnly));
        }
        let ids = clean_open_ids(ids);
        if ids == self.open_at_quit {
            return Ok(());
        }
        self.write_all(&self.entries, &ids)?;
        self.open_at_quit = ids;
        Ok(())
    }

    /// Updater: the version whose automatic prompt the user skipped.
    pub fn skipped_update_version(&self) -> Option<&str> {
        self.skipped_update_version.as_deref()
    }

    /// Updater ("Skip this version"): remember one version (replaces an earlier one); one
    /// atomic write. `invalidArgument{version}` for text that is not a plausible version.
    pub fn set_skipped_update_version(&mut self, version: &str) -> Result<(), AppError> {
        if !valid_version_text(version) {
            return Err(AppError::invalid_argument("version"));
        }
        if self.read_only {
            return Err(AppError::new(ErrorCode::StorageReadOnly));
        }
        if self.skipped_update_version.as_deref() == Some(version) {
            return Ok(());
        }
        self.write_doc(&self.entries, &self.open_at_quit, Some(version))?;
        self.skipped_update_version = Some(version.to_owned());
        Ok(())
    }

    pub fn list(&self) -> Vec<RecentEntry> {
        self.entries.iter().map(to_dto).collect()
    }

    pub fn read_only(&self) -> bool {
        self.read_only
    }

    /// First `connected` of a session.
    pub fn upsert_connected(
        &mut self,
        target: RecentTarget,
        label: String,
        user_at_host: String,
        via: Option<String>,
        now_ms: i64,
    ) -> Result<Vec<RecentEntry>, AppError> {
        let mut entries = self.entries.clone();
        let previous = entries
            .iter()
            .position(|e| e.target == target)
            .map(|i| entries.remove(i));
        // Display texts are shortened, never a reason to drop the connection's entry
        // (a long `via` chain or a long `user@host` is legitimate).
        let entry = StoredRecent {
            target,
            label: display_text(&label),
            user_at_host: display_text(&user_at_host),
            via: via.as_deref().map(display_text),
            last_connected_at_ms: now_ms,
            last_dir: previous.as_ref().and_then(|p| p.last_dir.clone()),
            terminal_count: previous.as_ref().map_or(0, |p| p.terminal_count),
        };
        if !valid_entry(&entry) {
            return Err(AppError::invalid_argument("target"));
        }
        entries.insert(0, entry);
        entries.truncate(MAX_RECENTS);
        self.commit(entries)
    }

    /// `session_close`: `last_dir` from the frontend (kept if `None`), `terminal_count` counted
    /// by the backend. An invalid `last_dir` (too long, NUL) is ignored and the stored one kept;
    /// the terminal count is still recorded (it used to reject the whole update).
    pub fn record_close(
        &mut self,
        target: &RecentTarget,
        last_dir: Option<String>,
        terminal_count: u32,
        _now_ms: i64,
    ) -> Result<Vec<RecentEntry>, AppError> {
        let mut entries = self.entries.clone();
        let Some(entry) = entries.iter_mut().find(|e| &e.target == target) else {
            return Ok(self.list());
        };
        match last_dir {
            Some(dir) if dir.chars().count() <= MAX_PATH_CHARS && !dir.contains('\0') => {
                entry.last_dir = Some(dir);
            }
            Some(_) => tracing::debug!("ignored an invalid lastDir on session close"),
            None => {}
        }
        entry.terminal_count = terminal_count;
        self.commit(entries)
    }

    /// `recents_clear`: forget every entry and the bookmarks open at the last quit
    /// (one atomic write). `storageReadOnly` for a newer-version file.
    pub fn clear(&mut self) -> Result<Vec<RecentEntry>, AppError> {
        if self.read_only {
            return Err(AppError::new(ErrorCode::StorageReadOnly));
        }
        self.write_all(&[], &[])?;
        self.entries.clear();
        self.open_at_quit.clear();
        Ok(self.list())
    }

    /// Remove the entry for `target` (e.g. after its bookmark was deleted).
    pub fn remove_target(&mut self, target: &RecentTarget) -> Result<Vec<RecentEntry>, AppError> {
        if !self.entries.iter().any(|e| &e.target == target) {
            return Ok(self.list());
        }
        let entries = self
            .entries
            .iter()
            .filter(|e| &e.target != target)
            .cloned()
            .collect();
        self.commit(entries)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::fs;

    fn quick(spec: &str) -> RecentTarget {
        RecentTarget::Quick { spec: spec.into() }
    }

    /// Long display texts are shortened (not rejected); IPv6 and UPN / `DOMAIN\user`
    /// specs (as recorded for drafts) are valid targets; the entry survives a reload.
    #[test]
    fn long_texts_ipv6_and_unusual_users_are_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = RecentStore::load(dir.path());
        let via = (0..40)
            .map(|i| format!("jump-host-{i}.example.com"))
            .collect::<Vec<_>>()
            .join(", ");
        let long_user = format!("{}@{}", "u".repeat(255), "h".repeat(253));
        for spec in [
            "alice@[2001:db8::1]",
            "alice@[2001:db8::1]:2222",
            r"CORP\alice@host.example.com",
            "alice@corp.example.com@host.example.com",
        ] {
            let l = s
                .upsert_connected(
                    quick(spec),
                    "x\u{7}y".into(),
                    long_user.clone(),
                    Some(via.clone()),
                    1,
                )
                .unwrap();
            assert_eq!(l[0].target, quick(spec), "{spec}");
            assert_eq!(l[0].label, "xy");
            assert_eq!(l[0].user_at_host.chars().count(), MAX_TEXT_CHARS);
            assert!(l[0].via.as_deref().unwrap().ends_with('…'));
        }
        let (s2, w2) = RecentStore::load(dir.path());
        assert!(w2.is_empty(), "{w2:?}");
        assert_eq!(s2.list().len(), 4);
    }

    #[test]
    fn upsert_orders_caps_and_keeps_close_data() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, w) = RecentStore::load(dir.path());
        assert!(w.is_empty());
        for i in 0..12 {
            s.upsert_connected(
                quick(&format!("h{i}")),
                format!("h{i}"),
                format!("alice@h{i}"),
                None,
                i,
            )
            .unwrap();
        }
        let l = s.list();
        assert_eq!(l.len(), MAX_RECENTS);
        assert_eq!(l[0].target, quick("h11"));
        assert_eq!(l[9].target, quick("h2"));

        s.record_close(&quick("h5"), Some("/srv/app".into()), 3, 100)
            .unwrap();
        s.record_close(&quick("h5"), None, 2, 101).unwrap();
        let l = s
            .upsert_connected(
                quick("h5"),
                "h5".into(),
                "alice@h5".into(),
                Some("bastion".into()),
                200,
            )
            .unwrap();
        assert_eq!(l[0].target, quick("h5"));
        assert_eq!(l[0].last_dir.as_deref(), Some("/srv/app"));
        assert_eq!(l[0].terminal_count, 2);
        assert_eq!(l[0].via.as_deref(), Some("bastion"));
        assert_eq!(l[0].last_connected_at_ms, 200);

        // An invalid lastDir (NUL, too long) is ignored, the count is still recorded.
        for bad in [
            "/srv/\0x".to_owned(),
            format!("/{}", "a".repeat(MAX_PATH_CHARS)),
        ] {
            let after = s.record_close(&quick("h5"), Some(bad), 4, 210).unwrap();
            assert_eq!(after[0].last_dir.as_deref(), Some("/srv/app"));
            assert_eq!(after[0].terminal_count, 4);
        }
        s.record_close(&quick("h5"), None, 2, 220).unwrap();

        // Close of an evicted target is a no-op.
        let json = |v: &Vec<RecentEntry>| serde_json::to_value(v).unwrap();
        assert_eq!(
            json(
                &s.record_close(&quick("h0"), Some("/x".into()), 1, 300)
                    .unwrap()
            ),
            json(&l)
        );

        let (s2, w2) = RecentStore::load(dir.path());
        assert!(w2.is_empty());
        assert_eq!(json(&s2.list()), json(&l));
        assert_eq!(s2.list().len(), MAX_RECENTS);
    }

    #[test]
    fn clear_empties_and_persists_and_respects_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = RecentStore::load(dir.path());
        s.upsert_connected(
            RecentTarget::SshConfig {
                alias: "web".into(),
            },
            "web".into(),
            "u@web".into(),
            None,
            1,
        )
        .unwrap();
        assert!(s.clear().unwrap().is_empty());
        assert!(RecentStore::load(dir.path()).0.list().is_empty());

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("recents.json"),
            br#"{"version":99,"entries":[]}"#,
        )
        .unwrap();
        let (mut s, _) = RecentStore::load(dir.path());
        assert!(s.read_only());
        assert_eq!(s.clear().unwrap_err().kind, ErrorCode::StorageReadOnly);
    }

    /// Open-at-quit ids persist next to the entries, are cleaned (hostile file),
    /// survive other writes and are cleared with the recents.
    #[test]
    fn open_at_quit_persists_and_is_cleaned() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = RecentStore::load(dir.path());
        assert!(s.open_at_quit().is_empty());
        s.set_open_at_quit(vec![
            "b-1".into(),
            "a-2".into(),
            "b-1".into(),
            "../etc".into(),
            String::new(),
            "x".repeat(65),
        ])
        .unwrap();
        assert_eq!(s.open_at_quit(), ["b-1", "a-2"]);
        s.upsert_connected(quick("h"), "h".into(), "u@h".into(), None, 1)
            .unwrap();
        let (s2, w) = RecentStore::load(dir.path());
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(s2.open_at_quit(), ["b-1", "a-2"]);
        assert_eq!(s2.list().len(), 1);

        // Hostile values in the file are dropped without failing the store.
        let p = dir.path().join("recents.json");
        let mut ids: Vec<Value> = (0..100).map(|i| json!(format!("id{i}"))).collect();
        ids.insert(0, json!(42));
        ids.insert(1, json!({"id": "x"}));
        ids.insert(2, json!("evil\u{1b}"));
        fs::write(
            &p,
            serde_json::to_vec(&json!({"version": 1, "entries": [], "openAtQuit": ids})).unwrap(),
        )
        .unwrap();
        let (mut s3, w) = RecentStore::load(dir.path());
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(s3.open_at_quit().len(), MAX_OPEN_AT_QUIT);
        assert_eq!(s3.open_at_quit()[0], "id0");
        // A malformed list never costs the entries.
        fs::write(
            &p,
            serde_json::to_vec(&json!({"version": 1, "openAtQuit": "nope", "entries": [
                {"target": {"kind": "quick", "spec": "alice@example.com"}, "label": "ok",
                 "userAtHost": "alice@example.com", "via": null, "lastConnectedAtMs": 5,
                 "lastDir": null, "terminalCount": 0}]}))
            .unwrap(),
        )
        .unwrap();
        let (l, w) = RecentStore::load(dir.path());
        assert!(w.is_empty(), "{w:?}");
        assert!(l.open_at_quit().is_empty());
        assert_eq!(l.list().len(), 1);

        s3.clear().unwrap();
        assert!(s3.open_at_quit().is_empty());
        assert!(RecentStore::load(dir.path()).0.open_at_quit().is_empty());

        // Read-only: refused.
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("recents.json"),
            br#"{"version": 9, "entries": []}"#,
        )
        .unwrap();
        let (mut ro, _) = RecentStore::load(dir.path());
        assert_eq!(
            ro.set_open_at_quit(vec!["a".into()]).unwrap_err().kind,
            ErrorCode::StorageReadOnly
        );
    }

    /// Updater: the skipped version persists, survives other writes and `recents_clear`,
    /// and hostile values are dropped on load without costing the entries.
    #[test]
    fn skipped_update_version_persists_and_is_validated() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = RecentStore::load(dir.path());
        assert_eq!(s.skipped_update_version(), None);
        s.set_skipped_update_version("0.2.0-experimental.1")
            .unwrap();
        s.set_open_at_quit(vec!["b-1".into()]).unwrap();
        s.upsert_connected(quick("h"), "h".into(), "u@h".into(), None, 1)
            .unwrap();
        let (mut s2, w) = RecentStore::load(dir.path());
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(s2.skipped_update_version(), Some("0.2.0-experimental.1"));
        s2.set_skipped_update_version("0.2.0").unwrap();
        s2.clear().unwrap();
        assert_eq!(
            RecentStore::load(dir.path()).0.skipped_update_version(),
            Some("0.2.0"),
            "clearing recent connections keeps the skipped version"
        );

        for bad in ["", "v1.0.0", "1.0.0 ", "1.0.0\u{1b}[2J", "../1", "１.0.0"] {
            assert_eq!(
                s2.set_skipped_update_version(bad).unwrap_err().kind,
                ErrorCode::InvalidArgument {
                    field: "version".into()
                },
                "{bad:?}"
            );
        }
        assert!(s2.set_skipped_update_version(&"1".repeat(65)).is_err());

        let p = dir.path().join("recents.json");
        for hostile in [
            json!(42),
            json!("evil\u{202e}"),
            json!("x1.0"),
            json!(["1.0.0"]),
        ] {
            fs::write(
                &p,
                serde_json::to_vec(&json!({"version": 1, "entries": [
                    {"target": {"kind": "quick", "spec": "alice@example.com"}, "label": "ok",
                     "userAtHost": "alice@example.com", "via": null, "lastConnectedAtMs": 5,
                     "lastDir": null, "terminalCount": 0}],
                    "skippedUpdateVersion": hostile}))
                .unwrap(),
            )
            .unwrap();
            let (l, w) = RecentStore::load(dir.path());
            assert!(w.is_empty(), "{w:?}");
            assert_eq!(l.skipped_update_version(), None);
            assert_eq!(l.list().len(), 1);
        }

        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("recents.json"),
            br#"{"version": 9, "entries": []}"#,
        )
        .unwrap();
        let (mut ro, _) = RecentStore::load(dir.path());
        assert_eq!(
            ro.set_skipped_update_version("1.0.0").unwrap_err().kind,
            ErrorCode::StorageReadOnly
        );
    }

    #[test]
    fn remove_target() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = RecentStore::load(dir.path());
        let b = RecentTarget::Bookmark { id: "b3f0".into() };
        s.upsert_connected(b.clone(), "nas".into(), "alice@nas".into(), None, 1)
            .unwrap();
        s.upsert_connected(
            RecentTarget::SshConfig {
                alias: "web".into(),
            },
            "web".into(),
            "u@web".into(),
            None,
            2,
        )
        .unwrap();
        assert_eq!(s.remove_target(&b).unwrap().len(), 1);
    }

    #[test]
    fn invalid_entries_dropped_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("recents.json");
        let doc = json!({"version": 1, "entries": [
            {"target": {"kind": "quick", "spec": "alice@example.com:2222"}, "label": "ok", "userAtHost": "alice@example.com",
             "via": null, "lastConnectedAtMs": 5, "lastDir": "/home/alice", "terminalCount": 1},
            {"target": {"kind": "quick", "spec": "bad host"}, "label": "x", "userAtHost": "x",
             "via": null, "lastConnectedAtMs": 6, "lastDir": null, "terminalCount": 0},
            {"target": {"kind": "draft", "password": "nope"}, "label": "x"},
            {"target": {"kind": "sshConfig", "alias": "web"}, "label": "evil\u{1b}[31m", "userAtHost": "x",
             "via": null, "lastConnectedAtMs": 7, "lastDir": null, "terminalCount": 0}
        ]});
        fs::write(&p, serde_json::to_vec(&doc).unwrap()).unwrap();
        let (s, w) = RecentStore::load(dir.path());
        assert_eq!(s.list().len(), 1);
        assert_eq!(
            serde_json::to_value(&w[0]).unwrap()["code"],
            "storageCorrupt"
        );
        let (_, w2) = RecentStore::load(dir.path());
        assert!(w2.is_empty());
    }

    #[test]
    fn read_only_refuses_writes() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("recents.json"),
            br#"{"version": 9, "entries": []}"#,
        )
        .unwrap();
        let (mut s, w) = RecentStore::load(dir.path());
        assert!(s.read_only());
        assert_eq!(
            serde_json::to_value(&w[0]).unwrap()["code"],
            "storageReadOnly"
        );
        let e = s
            .upsert_connected(quick("h"), "h".into(), "u@h".into(), None, 1)
            .unwrap_err();
        assert_eq!(e.kind, ErrorCode::StorageReadOnly);
    }
}
