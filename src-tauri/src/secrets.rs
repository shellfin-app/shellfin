//! OS keychain wrapper (§5.6, §9.2): passwords only, never a plaintext fallback.
//!
//! - Service name: [`SERVICE`] = `app.shellfin.desktop` (equal to the bundle identifier).
//! - Accounts: `bookmark:<uuid>` ([`bookmark_account`]) and `sshconfig:<user>@<hostname>:<port>`
//!   ([`ssh_config_account`]).
//! - All functions are **blocking** (Credential Manager / Keychain / Secret Service over D-Bus):
//!   call them from `spawn_blocking`.
//! - Store availability is evaluated **once per process** ([`availability`]); a keyring daemon
//!   started later requires an app restart.
//! - Errors map to `keyringUnavailable` (no store / store locked) or `keyringError`; `detail`
//!   carries the platform error text, never the secret.

use std::sync::OnceLock;

use zeroize::Zeroizing;

use crate::error::{AppError, ErrorCode};

/// Keychain service name — the bundle identifier.
pub const SERVICE: &str = "app.shellfin.desktop";

pub fn bookmark_account(bookmark_id: &str) -> String {
    format!("bookmark:{bookmark_id}")
}

pub fn ssh_config_account(user: &str, hostname: &str, port: u16) -> String {
    format!("sshconfig:{user}@{hostname}:{port}")
}

fn map_err(err: &keyring::Error) -> AppError {
    let code = match err {
        keyring::Error::NoDefaultStore | keyring::Error::NoStorageAccess(_) => {
            ErrorCode::KeyringUnavailable
        }
        _ => ErrorCode::KeyringError,
    };
    let detail = match err {
        // Fixed texts: these variants carry raw secret bytes.
        keyring::Error::BadEncoding(_) => "stored password is not valid UTF-8".to_owned(),
        keyring::Error::BadDataFormat(..) => "stored secret is malformed".to_owned(),
        other => other.to_string(),
    };
    AppError::new(code).with_detail(detail)
}

/// Whether the platform credential store is usable. Evaluated once per process; the UI disables
/// "Save in keychain" when this is an error (`keyringUnavailable`).
pub fn availability() -> Result<(), AppError> {
    // `e2e` test builds: the file-backed fake below the e2e root (src/e2e.rs).
    if crate::e2e::is_active() {
        return Ok(());
    }
    static STATUS: OnceLock<Result<(), String>> = OnceLock::new();
    match STATUS.get_or_init(|| {
        keyring::Entry::store_status()
            .as_ref()
            .copied()
            .map_err(ToString::to_string)
    }) {
        Ok(()) => Ok(()),
        Err(detail) => {
            Err(AppError::new(ErrorCode::KeyringUnavailable).with_detail(detail.clone()))
        }
    }
}

fn entry(service: &str, account: &str) -> Result<keyring::Entry, AppError> {
    availability()?;
    keyring::Entry::new(service, account).map_err(|e| map_err(&e))
}

fn get_in(service: &str, account: &str) -> Result<Option<Zeroizing<String>>, AppError> {
    #[cfg(feature = "e2e")]
    if let Some(l) = crate::e2e::active() {
        return crate::e2e::keyring::get(&l.keyring_file, account);
    }
    match entry(service, account)?.get_password() {
        Ok(p) => Ok(Some(Zeroizing::new(p))),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(map_err(&e)),
    }
}

fn set_in(service: &str, account: &str, password: &str) -> Result<(), AppError> {
    #[cfg(feature = "e2e")]
    if let Some(l) = crate::e2e::active() {
        return crate::e2e::keyring::set(&l.keyring_file, account, password);
    }
    entry(service, account)?
        .set_password(password)
        .map_err(|e| map_err(&e))
}

fn delete_in(service: &str, account: &str) -> Result<(), AppError> {
    #[cfg(feature = "e2e")]
    if let Some(l) = crate::e2e::active() {
        return crate::e2e::keyring::delete(&l.keyring_file, account);
    }
    match entry(service, account)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(map_err(&e)),
    }
}

/// Read a saved password (`None` if there is no entry).
pub fn get_password(account: &str) -> Result<Option<Zeroizing<String>>, AppError> {
    get_in(SERVICE, account)
}

/// Save (create or replace) a password.
pub fn set_password(account: &str, password: &str) -> Result<(), AppError> {
    set_in(SERVICE, account, password)
}

/// Delete a saved password; a missing entry is not an error.
pub fn delete_password(account: &str) -> Result<(), AppError> {
    delete_in(SERVICE, account)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn account_names() {
        assert_eq!(bookmark_account("b3f0"), "bookmark:b3f0");
        assert_eq!(
            ssh_config_account("alice", "nas.example.com", 2222),
            "sshconfig:alice@nas.example.com:2222"
        );
    }

    #[test]
    fn error_mapping_never_includes_secret_bytes() {
        let e = map_err(&keyring::Error::BadEncoding(b"hunter2".to_vec()));
        assert_eq!(e.kind, ErrorCode::KeyringError);
        assert!(!format!("{e:?}").contains("hunter2"));
        assert_eq!(
            map_err(&keyring::Error::NoDefaultStore).kind,
            ErrorCode::KeyringUnavailable
        );
    }

    /// Touches the real OS keychain with a dedicated test service; run manually with
    /// `cargo test -- --ignored keychain_roundtrip`.
    #[test]
    #[ignore = "uses the real OS keychain"]
    fn keychain_roundtrip() {
        const TEST_SERVICE: &str = "app.shellfin.desktop.test";
        let account = format!("bookmark:test-{}", uuid::Uuid::new_v4());
        availability().unwrap();
        assert!(get_in(TEST_SERVICE, &account).unwrap().is_none());
        set_in(TEST_SERVICE, &account, "s3cret-ünïcode").unwrap();
        assert_eq!(
            get_in(TEST_SERVICE, &account)
                .unwrap()
                .as_deref()
                .map(String::as_str),
            Some("s3cret-ünïcode")
        );
        set_in(TEST_SERVICE, &account, "second").unwrap();
        assert_eq!(
            get_in(TEST_SERVICE, &account)
                .unwrap()
                .as_deref()
                .map(String::as_str),
            Some("second")
        );
        delete_in(TEST_SERVICE, &account).unwrap();
        delete_in(TEST_SERVICE, &account).unwrap();
        assert!(get_in(TEST_SERVICE, &account).unwrap().is_none());
    }
}
