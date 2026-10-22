//! The one parsed import waiting for the user's decision (`bookmarks_import_open` →
//! `bookmarks_import_apply` / `bookmarks_import_cancel`, §7.7).
//!
//! The parsed file (not its path) is kept under a fresh random token, so the apply step uses
//! exactly what the preview showed and never reads the file again. There is at most one: a new
//! open replaces the previous one. Apply consumes it on every attempt (success, error, or a
//! wrong token), so a token is good for one apply at most; cancel drops it only for the
//! matching token.

use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::error::AppError;
use crate::storage::bookmarks::ParsedImport;

struct PendingImport {
    token: String,
    parsed: ParsedImport,
}

/// Held in `AppState`. The lock is only held for a swap (never across I/O), so it may be taken
/// on the async runtime.
#[derive(Default)]
pub struct PendingImports(Mutex<Option<PendingImport>>);

impl std::fmt::Debug for PendingImports {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PendingImports")
    }
}

impl PendingImports {
    fn slot(&self) -> MutexGuard<'_, Option<PendingImport>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Keep `parsed` as the pending import (replacing any previous one); returns its token.
    pub fn put(&self, parsed: ParsedImport) -> String {
        let token = uuid::Uuid::new_v4().to_string();
        *self.slot() = Some(PendingImport {
            token: token.clone(),
            parsed,
        });
        token
    }

    /// Drop the pending import, if any (a new open starts, or its dialog was cancelled).
    pub fn clear(&self) {
        self.slot().take();
    }

    /// Take the pending import out for `token`. The slot is emptied whatever the outcome; no
    /// pending import or another token is `invalidArgument{field:"token"}`.
    pub fn take(&self, token: &str) -> Result<ParsedImport, AppError> {
        match self.slot().take() {
            Some(p) if p.token == token => Ok(p.parsed),
            _ => Err(AppError::invalid_argument("token")),
        }
    }

    /// Drop the pending import when `token` names it; otherwise nothing happens (idempotent).
    pub fn cancel(&self, token: &str) {
        let mut slot = self.slot();
        if slot.as_ref().is_some_and(|p| p.token == token) {
            slot.take();
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::error::ErrorCode;
    use crate::storage::bookmarks::parse_import;

    /// A parsed file with `n` valid bookmarks, so tests can tell two pending imports apart.
    fn parsed(n: usize) -> ParsedImport {
        let bookmarks: Vec<serde_json::Value> = (0..n)
            .map(|i| serde_json::json!({"id": format!("b{i}"), "name": format!("n{i}"), "host": "h"}))
            .collect();
        let doc = serde_json::json!({
            "format": "shellfin-bookmarks", "version": 1, "bookmarks": bookmarks,
        });
        parse_import(&serde_json::to_vec(&doc).unwrap()).unwrap()
    }

    fn is_token_error(e: &AppError) -> bool {
        e.kind
            == ErrorCode::InvalidArgument {
                field: "token".to_owned(),
            }
    }

    #[test]
    fn apply_without_open_is_refused() {
        let p = PendingImports::default();
        assert!(is_token_error(&p.take("anything").unwrap_err()));
        assert!(is_token_error(&p.take("").unwrap_err()));
    }

    #[test]
    fn token_is_single_use() {
        let p = PendingImports::default();
        let t = p.put(parsed(2));
        assert_eq!(uuid::Uuid::parse_str(&t).unwrap().get_version_num(), 4);
        assert_eq!(p.take(&t).unwrap().valid_bookmarks(), 2);
        assert!(is_token_error(&p.take(&t).unwrap_err()), "second apply");
    }

    #[test]
    fn wrong_token_is_refused_and_consumes_the_pending_import() {
        let p = PendingImports::default();
        let t = p.put(parsed(1));
        assert!(is_token_error(&p.take("not-the-token").unwrap_err()));
        assert!(
            is_token_error(&p.take(&t).unwrap_err()),
            "an apply attempt consumes the pending import"
        );
    }

    #[test]
    fn apply_after_cancel_is_refused_and_cancel_is_idempotent() {
        let p = PendingImports::default();
        let t = p.put(parsed(1));
        p.cancel(&t);
        p.cancel(&t);
        assert!(is_token_error(&p.take(&t).unwrap_err()));
        // Cancel without anything pending is a no-op too.
        p.cancel("nothing");
    }

    #[test]
    fn cancel_with_another_token_keeps_the_pending_import() {
        let p = PendingImports::default();
        let t = p.put(parsed(3));
        p.cancel("not-the-token");
        p.cancel("");
        assert_eq!(p.take(&t).unwrap().valid_bookmarks(), 3);
    }

    #[test]
    fn a_second_open_replaces_the_first() {
        let p = PendingImports::default();
        let first = p.put(parsed(1));
        let second = p.put(parsed(2));
        assert_ne!(first, second);
        p.cancel(&first);
        assert_eq!(
            p.take(&second).unwrap().valid_bookmarks(),
            2,
            "the old token neither cancels nor applies the new import"
        );
        let first = p.put(parsed(1));
        let _second = p.put(parsed(2));
        assert!(is_token_error(&p.take(&first).unwrap_err()));
    }

    #[test]
    fn clear_drops_the_pending_import() {
        let p = PendingImports::default();
        let t = p.put(parsed(1));
        p.clear();
        assert!(is_token_error(&p.take(&t).unwrap_err()));
    }
}
