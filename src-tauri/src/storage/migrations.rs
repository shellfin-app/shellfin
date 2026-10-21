//! Storage schema migrations (§7.2). Each file has a top-level integer `"version"`.
//!
//! - `version == CURRENT` → unchanged.
//! - `version > CURRENT` (written by a newer app) → **read-only**: data is shown, every write is
//!   refused with `storageReadOnly`.
//! - `version < CURRENT` → steps `vN → vN+1` over `serde_json::Value`, writing a
//!   `<stem>.v<N>.bak.json` backup of the vN document before each step. The caller persists
//!   the migrated document.
//! - Missing/non-integer version, or no step for an old version → [`MigrateError`]; the
//!   caller treats the file as corrupt (quarantine + defaults).
//!
//! Real steps: `bookmarks.json` v1 → v2 (explicit order and color, see [`bookmarks_v1_to_v2`]).
//! The mechanism itself is also tested with a synthetic step table.

use std::path::Path;

use serde_json::Value;

use crate::storage::file::write_json;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreFile {
    Bookmarks,
    Settings,
    Recents,
}

impl StoreFile {
    pub const fn file_name(self) -> &'static str {
        match self {
            Self::Bookmarks => "bookmarks.json",
            Self::Settings => "settings.json",
            Self::Recents => "recents.json",
        }
    }

    /// Current schema version written by this build.
    pub const fn current_version(self) -> u64 {
        match self {
            Self::Bookmarks => 2,
            Self::Settings | Self::Recents => 1,
        }
    }

    /// Migration steps: `(from_version, step)` migrates `from_version → from_version + 1`.
    fn steps(self) -> &'static [(u64, Step)] {
        match self {
            Self::Bookmarks => &[(1, bookmarks_v1_to_v2)],
            Self::Settings | Self::Recents => &[],
        }
    }
}

/// `bookmarks.json` v1 → v2: every bookmark gets a dense `order` within its group (the order the
/// UI showed: case-insensitive name), and the groups get dense `order` values 0..n in their old
/// order (ties keep file order). Malformed entries are left untouched (load validation drops
/// them). `color` is optional and needs no default.
fn bookmarks_v1_to_v2(mut doc: Value) -> Result<Value, String> {
    let obj = doc.as_object_mut().ok_or("document is not an object")?;
    if let Some(Value::Array(groups)) = obj.get_mut("groups") {
        // Stable: ties and entries without a numeric order keep their file order.
        groups.sort_by_key(|g| g.get("order").and_then(Value::as_i64).unwrap_or(i64::MAX));
        for (rank, g) in groups.iter_mut().enumerate() {
            if let Some(g) = g.as_object_mut() {
                g.insert("order".to_owned(), Value::from(rank));
            }
        }
    }
    if let Some(Value::Array(bookmarks)) = obj.get_mut("bookmarks") {
        let key = |b: &Value| {
            (
                b.get("groupId").and_then(Value::as_str).map(str::to_owned),
                b.get("name")
                    .and_then(Value::as_str)
                    .map(str::to_lowercase)
                    .unwrap_or_default(),
            )
        };
        let mut ranked: Vec<_> = bookmarks.iter_mut().collect();
        ranked.sort_by_cached_key(|b| key(b));
        let mut next: std::collections::HashMap<Option<String>, usize> =
            std::collections::HashMap::new();
        for b in ranked {
            let group = key(b).0;
            if let Some(obj) = b.as_object_mut() {
                let n = next.entry(group).or_insert(0);
                obj.insert("order".to_owned(), Value::from(*n));
                *n += 1;
            }
        }
    }
    Ok(doc)
}

/// One `vN → vN+1` migration over the raw JSON document.
pub type Step = fn(Value) -> Result<Value, String>;

#[derive(Debug, Clone, PartialEq)]
pub struct Migrated {
    pub value: Value,
    /// The file was written by a newer version; never write it.
    pub read_only: bool,
    /// `Some(old)` if steps ran; the caller must persist `value`.
    pub migrated_from: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum MigrateError {
    #[error("missing or invalid top-level \"version\"")]
    NoVersion,
    #[error("no migration step from version {0}")]
    NoStep(u64),
    #[error("migration step from version {from} failed: {reason}")]
    StepFailed { from: u64, reason: String },
    #[error("could not write the migration backup: {0}")]
    Backup(#[from] std::io::Error),
}

pub fn version_of(value: &Value) -> Option<u64> {
    value.get("version").and_then(Value::as_u64)
}

/// Migrate `value` (the content of `path`) to the current version of `file`.
pub fn migrate(value: Value, file: StoreFile, path: &Path) -> Result<Migrated, MigrateError> {
    migrate_with(value, file.current_version(), file.steps(), path)
}

fn migrate_with(
    value: Value,
    current: u64,
    steps: &[(u64, Step)],
    path: &Path,
) -> Result<Migrated, MigrateError> {
    let from = version_of(&value).ok_or(MigrateError::NoVersion)?;
    if from > current {
        return Ok(Migrated {
            value,
            read_only: true,
            migrated_from: None,
        });
    }
    if from == current {
        return Ok(Migrated {
            value,
            read_only: false,
            migrated_from: None,
        });
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut doc = value;
    for version in from..current {
        let step = steps
            .iter()
            .find(|(v, _)| *v == version)
            .map(|(_, s)| *s)
            .ok_or(MigrateError::NoStep(version))?;
        write_json(
            &path.with_file_name(format!("{stem}.v{version}.bak.json")),
            &doc,
        )?;
        doc = step(doc).map_err(|reason| MigrateError::StepFailed {
            from: version,
            reason,
        })?;
        if let Some(obj) = doc.as_object_mut() {
            obj.insert("version".to_owned(), Value::from(version + 1));
        }
    }
    Ok(Migrated {
        value: doc,
        read_only: false,
        migrated_from: Some(from),
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use serde_json::json;

    fn v1_to_v2(mut v: Value) -> Result<Value, String> {
        let name = v["title"].take();
        v["name"] = name;
        v.as_object_mut().ok_or("not an object")?.remove("title");
        Ok(v)
    }

    fn v2_to_v3(mut v: Value) -> Result<Value, String> {
        v["extra"] = json!(true);
        Ok(v)
    }

    fn failing(_: Value) -> Result<Value, String> {
        Err("boom".into())
    }

    #[test]
    fn bookmarks_v1_to_v2_assigns_dense_orders_and_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bookmarks.json");
        let doc = json!({"version": 1,
            "groups": [{"id":"g2","name":"B","order":7},{"id":"g1","name":"A","order":3}],
            "bookmarks": [
              {"id":"b1","name":"zeta","groupId":"g1","host":"h"},
              {"id":"b2","name":"Alpha","groupId":null,"host":"h"},
              {"id":"b3","name":"alpha2","groupId":"g1","host":"h"},
              {"id":"b4","name":"beta","groupId":null,"host":"h"},
              "junk"]});
        let m = migrate(doc.clone(), StoreFile::Bookmarks, &p).unwrap();
        assert_eq!(m.migrated_from, Some(1));
        assert!(!m.read_only);
        let v = &m.value;
        assert_eq!(v["version"], 2);
        assert_eq!(v["groups"][0]["id"], "g1");
        assert_eq!(v["groups"][0]["order"], 0);
        assert_eq!(v["groups"][1]["id"], "g2");
        assert_eq!(v["groups"][1]["order"], 1);
        let order = |i: usize| v["bookmarks"][i]["order"].clone();
        // ungrouped: Alpha(0) beta(1); g1: alpha2(0) zeta(1)
        assert_eq!((order(1), order(3)), (json!(0), json!(1)));
        assert_eq!((order(2), order(0)), (json!(0), json!(1)));
        assert_eq!(v["bookmarks"][4], "junk");
        assert_eq!(v["bookmarks"][0]["host"], "h");
        // The v1 document was backed up unchanged.
        let bak: Value = serde_json::from_slice(
            &std::fs::read(dir.path().join("bookmarks.v1.bak.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(bak, doc);
    }

    #[test]
    fn current_version_is_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let m = migrate(
            json!({"version": 1, "a": 1}),
            StoreFile::Settings,
            &dir.path().join("settings.json"),
        )
        .unwrap();
        assert_eq!(
            m,
            Migrated {
                value: json!({"version": 1, "a": 1}),
                read_only: false,
                migrated_from: None
            }
        );
    }

    #[test]
    fn newer_version_is_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let m = migrate(
            json!({"version": 7}),
            StoreFile::Bookmarks,
            &dir.path().join("bookmarks.json"),
        )
        .unwrap();
        assert!(m.read_only);
        assert_eq!(m.value, json!({"version": 7}));
    }

    #[test]
    fn missing_version_or_missing_step_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("recents.json");
        assert!(matches!(
            migrate(json!({"entries": []}), StoreFile::Recents, &p),
            Err(MigrateError::NoVersion)
        ));
        assert!(matches!(
            migrate(json!({"version": "1"}), StoreFile::Recents, &p),
            Err(MigrateError::NoVersion)
        ));
        assert!(matches!(
            migrate(json!({"version": 0}), StoreFile::Recents, &p),
            Err(MigrateError::NoStep(0))
        ));
    }

    #[test]
    fn chain_runs_with_backups_per_step() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bookmarks.json");
        let steps: &[(u64, Step)] = &[(1, v1_to_v2), (2, v2_to_v3)];
        let m = migrate_with(json!({"version": 1, "title": "x"}), 3, steps, &p).unwrap();
        assert_eq!(m.value, json!({"version": 3, "name": "x", "extra": true}));
        assert_eq!(m.migrated_from, Some(1));
        let b1: Value = serde_json::from_slice(
            &std::fs::read(dir.path().join("bookmarks.v1.bak.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(b1, json!({"version": 1, "title": "x"}));
        let b2: Value = serde_json::from_slice(
            &std::fs::read(dir.path().join("bookmarks.v2.bak.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(b2, json!({"version": 2, "name": "x"}));
    }

    #[test]
    fn failing_step_reports_version() {
        let dir = tempfile::tempdir().unwrap();
        let steps: &[(u64, Step)] = &[(1, failing)];
        let err =
            migrate_with(json!({"version": 1}), 2, steps, &dir.path().join("s.json")).unwrap_err();
        assert!(matches!(err, MigrateError::StepFailed { from: 1, .. }));
    }
}
