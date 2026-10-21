//! OS drag & drop (§6.6, §4.2): `WindowEvent::DragDrop` → `drop:hover` / `drop:leave` /
//! `drop:files`. Local paths stay in Rust: the WebView only sees names, `isDir` and logical
//! positions, and redeems a `dropId` once with `transfer_upload_dropped` (60 s TTL).
//!
//! Drop effect: Tauri/wry accept every OS file drag over the whole window (Windows: wry's
//! `IDropTarget` always answers `DROPEFFECT_COPY`; macOS: `NSDragOperationCopy`) and the handler's
//! answer is ignored, so the OS cursor cannot be switched per position (see `docs/ARCHITECTURE.md` §6.6). The UI
//! shows its own drop highlight from `drop:hover`; a drop outside a folder is simply not redeemed.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::error::{AppError, ErrorCode};
use crate::ipc::dto::DropEntry;

/// Lifetime of an unredeemed drop (§6.6).
pub const DROP_TTL: Duration = Duration::from_secs(60);
/// At most this many unredeemed drops are kept (oldest evicted).
pub const MAX_DROPS: usize = 32;

struct Pending {
    at: Instant,
    paths: Vec<PathBuf>,
}

#[derive(Default)]
pub struct DropRegistry {
    inner: Mutex<HashMap<String, Pending>>,
}

impl std::fmt::Debug for DropRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DropRegistry").finish_non_exhaustive()
    }
}

impl DropRegistry {
    /// Store the dropped paths; returns the `dropId`.
    pub fn register(&self, paths: Vec<PathBuf>) -> String {
        self.register_at(paths, Instant::now())
    }

    fn register_at(&self, paths: Vec<PathBuf>, at: Instant) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let mut map = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        map.retain(|_, p| p.at.elapsed() < DROP_TTL);
        while map.len() >= MAX_DROPS {
            let oldest = map.iter().min_by_key(|(_, p)| p.at).map(|(k, _)| k.clone());
            match oldest {
                Some(k) => {
                    map.remove(&k);
                }
                None => break,
            }
        }
        map.insert(id.clone(), Pending { at, paths });
        id
    }

    /// Redeem a drop (consumed by the first call): `dropExpired{dropId}` if unknown, used or older
    /// than 60 s.
    pub fn take(&self, drop_id: &str) -> Result<Vec<PathBuf>, AppError> {
        let mut map = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        match map.remove(drop_id) {
            Some(p) if p.at.elapsed() < DROP_TTL => Ok(p.paths),
            _ => Err(AppError::new(ErrorCode::DropExpired {
                drop_id: drop_id.to_owned(),
            })),
        }
    }
}

/// `DropEntry` for one dropped path (blocking: stats the path).
pub fn entry_of(path: &std::path::Path) -> DropEntry {
    DropEntry {
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string()),
        is_dir: std::fs::metadata(path).is_ok_and(|m| m.is_dir()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn drops_are_consumed_once_and_expire() {
        let reg = DropRegistry::default();
        let id = reg.register(vec![PathBuf::from("a")]);
        assert_eq!(reg.take(&id).unwrap(), vec![PathBuf::from("a")]);
        let e = reg.take(&id).unwrap_err();
        assert!(matches!(e.kind, ErrorCode::DropExpired { .. }));
        let old = reg.register_at(vec![PathBuf::from("b")], Instant::now() - DROP_TTL);
        assert!(reg.take(&old).is_err());
        assert!(reg.take("unknown").is_err());
    }

    #[test]
    fn registry_is_bounded() {
        let reg = DropRegistry::default();
        let first = reg.register(vec![]);
        for _ in 0..MAX_DROPS {
            reg.register(vec![]);
        }
        assert!(reg.take(&first).is_err(), "oldest drop evicted");
        assert!(reg.inner.lock().unwrap().len() <= MAX_DROPS);
    }

    #[test]
    fn entries_report_directories() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.txt");
        std::fs::write(&f, b"x").unwrap();
        assert_eq!(
            entry_of(&f),
            DropEntry {
                name: "x.txt".into(),
                is_dir: false
            }
        );
        assert!(entry_of(dir.path()).is_dir);
    }
}
