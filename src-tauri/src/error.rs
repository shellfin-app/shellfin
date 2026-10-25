//! Error model (§4.3). The backend never returns prose for the UI: only a code plus params.
//! `detail` is an English developer note for logs and "Copy details".
//!
//! Wire shape: `{"code":"hostKeyChanged","params":{"host":"h","port":22,…},"detail":"…"}`;
//! unit variants serialize as `{"code":"cancelled"}`.

use serde::Serialize;
use ts_rs::TS;

/// Every command returns `Result<T, AppError>`. Outbound only (Serialize).
#[derive(Debug, Clone, thiserror::Error, Serialize, TS)]
#[error("{kind:?}")]
#[ts(export)]
pub struct AppError {
    #[serde(flatten)]
    pub kind: ErrorCode,
    /// English developer detail for logs and "Copy details"; never primary UI text; server text allowed here.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "code", content = "params", rename_all = "camelCase")]
#[ts(export)]
pub enum ErrorCode {
    // generic
    Internal,
    /// A command panicked: a bug in Shellfin, not a problem with the user's setup. `command` is
    /// the command name (`snake_case`). The panic location is in `detail` and in the log; the
    /// panic message is never sent or logged (it may contain server data).
    #[serde(rename_all = "camelCase")]
    Bug {
        command: String,
    },
    Cancelled,
    PromptTimeout,
    StorageReadOnly,
    KeyringUnavailable,
    KeyringError,
    PromptNotFound,
    /// `field` = DTO field path, e.g. `"port"` or `"terminal.fontSize"`.
    #[serde(rename_all = "camelCase")]
    InvalidArgument {
        field: String,
    },
    #[serde(rename_all = "camelCase")]
    NotFound {
        what: EntityKind,
        id: String,
    },
    #[serde(rename_all = "camelCase")]
    Io {
        op: IoOp,
        path: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    StorageCorrupt {
        file: String,
        quarantined_as: String,
    },
    // resolution
    #[serde(rename_all = "camelCase")]
    InvalidHostSpec {
        spec: String,
    },
    #[serde(rename_all = "camelCase")]
    InvalidHostname {
        value: String,
    },
    #[serde(rename_all = "camelCase")]
    ProxyJumpCycle {
        chain: Vec<String>,
    },
    ProxyJumpTooDeep,
    #[serde(rename_all = "camelCase")]
    ProxyCommandUnsupported {
        alias: String,
    },
    // network
    #[serde(rename_all = "camelCase")]
    DnsFailed {
        host: String,
    },
    #[serde(rename_all = "camelCase")]
    ConnectRefused {
        host: String,
        port: u16,
    },
    #[serde(rename_all = "camelCase")]
    ConnectTimeout {
        host: String,
        port: u16,
        seconds: u32,
    },
    #[serde(rename_all = "camelCase")]
    NetworkUnreachable {
        host: String,
    },
    #[serde(rename_all = "camelCase")]
    JumpChannelFailed {
        via: String,
        host: String,
        port: u16,
    },
    KexFailed,
    ProtocolError,
    // host keys
    #[serde(rename_all = "camelCase")]
    HostKeyUnknownStrict {
        host: String,
        port: u16,
        key_type: String,
        fingerprint: String,
    },
    #[serde(rename_all = "camelCase")]
    HostKeyRejected {
        host: String,
        port: u16,
    },
    #[serde(rename_all = "camelCase")]
    HostKeyChanged {
        host: String,
        port: u16,
        key_type: String,
        fingerprint: String,
        known_hosts_file: String,
        line: u32,
        /// Backend-built, quoted removal command for the local shell, using the real known_hosts
        /// entry name (`HostKeyAlias` / `[host]:port`): `ssh-keygen -R '<lookup>' -f '<file>'`.
        /// `null` when a value cannot be quoted safely (control, bidi, invisible characters;
        /// a backslash for POSIX shells). The UI shows it verbatim and never builds its own.
        remove_hint: Option<String>,
        /// Which shell `removeHint` is quoted for (label it in the UI).
        remove_hint_shell: HintShell,
    },
    #[serde(rename_all = "camelCase")]
    HostKeyRevoked {
        host: String,
        port: u16,
        fingerprint: String,
    },
    #[serde(rename_all = "camelCase")]
    HostKeyCaManaged {
        host: String,
        port: u16,
        fingerprint: String,
        known_hosts_file: String,
        line: u32,
    },
    #[serde(rename_all = "camelCase")]
    HostKeyMismatchOnReconnect {
        host: String,
        port: u16,
        expected: String,
        actual: String,
    },
    // auth
    #[serde(rename_all = "camelCase")]
    AuthFailed {
        user: String,
        host: String,
        tried: Vec<AuthMethodKind>,
        server_methods: Vec<AuthMethodKind>,
    },
    #[serde(rename_all = "camelCase")]
    KeyLoadFailed {
        path: String,
        reason: KeyLoadReason,
    },
    #[serde(rename_all = "camelCase")]
    PassphraseIncorrect {
        path: String,
    },
    /// Session warning (§5.3): a known_hosts file exists but was ignored (unsafe owner or
    /// permissions, too large, unreadable). Hosts recorded only there count as unknown, and
    /// `accept-new`/`no` ask instead for that hop.
    #[serde(rename_all = "camelCase")]
    KnownHostsIgnored {
        file: String,
    },
    AgentUnavailable,
    AgentUntrusted,
    AgentNoIdentities,
    AgentConstraintUnsupported,
    TooManyAuthFailures,
    // session
    #[serde(rename_all = "camelCase")]
    Disconnected {
        reason: DisconnectKind,
        message: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    SessionNotFound {
        session_id: String,
    },
    #[serde(rename_all = "camelCase")]
    NotConnected {
        session_id: String,
    },
    #[serde(rename_all = "camelCase")]
    TerminalNotFound {
        term_id: String,
    },
    #[serde(rename_all = "camelCase")]
    ChannelOpenFailed {
        reason: String,
    },
    // sftp
    SftpUnavailable,
    #[serde(rename_all = "camelCase")]
    RemoteNotFound {
        path: String,
    },
    #[serde(rename_all = "camelCase")]
    PermissionDenied {
        path: String,
    },
    #[serde(rename_all = "camelCase")]
    RemoteExists {
        path: String,
    },
    #[serde(rename_all = "camelCase")]
    NotADirectory {
        path: String,
    },
    #[serde(rename_all = "camelCase")]
    IsADirectory {
        path: String,
    },
    /// The remote path (after links) is a FIFO, socket or device, not a regular file: reading
    /// it could block the SFTP server or never end.
    #[serde(rename_all = "camelCase")]
    NotARegularFile {
        path: String,
    },
    #[serde(rename_all = "camelCase")]
    DirectoryNotEmpty {
        path: String,
    },
    #[serde(rename_all = "camelCase")]
    FileIsBinary {
        path: String,
    },
    #[serde(rename_all = "camelCase")]
    RemoteChanged {
        path: String,
        size: u64,
        mtime: i64,
        /// The file a forced save would write when it differs from `path` (the path resolves
        /// through a symlink). Show it in the conflict dialog.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        target: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    FileTooLarge {
        path: String,
        size: u64,
        limit: u64,
    },
    #[serde(rename_all = "camelCase")]
    DecodeFailed {
        path: String,
        encoding: String,
    },
    #[serde(rename_all = "camelCase")]
    EncodingUnmappable {
        encoding: String,
    },
    #[serde(rename_all = "camelCase")]
    UnsupportedEncoding {
        encoding: String,
    },
    #[serde(rename_all = "camelCase")]
    SftpFailure {
        path: Option<String>,
        status: String,
    },
    #[serde(rename_all = "camelCase")]
    Timeout {
        op: TimeoutOp,
    },
    // transfers
    #[serde(rename_all = "camelCase")]
    LocalNotFound {
        path: String,
    },
    #[serde(rename_all = "camelCase")]
    LocalPermissionDenied {
        path: String,
    },
    #[serde(rename_all = "camelCase")]
    LocalIsADirectory {
        path: String,
    },
    #[serde(rename_all = "camelCase")]
    DropExpired {
        drop_id: String,
    },
    /// `bookmarks_import_open`: the chosen file cannot be imported at all (nothing was changed).
    #[serde(rename_all = "camelCase")]
    ImportInvalidFile {
        reason: ImportInvalidReason,
    },
    /// `connection.rememberPasswords` is off, so no new secret is stored in the OS
    /// keychain (`bookmark_set_password` with a password).
    SecretStorageDisabled,
    // updater (§9.8)
    /// This build never checks for updates (dev, debug or `e2e` build, or no HTTPS `stable.json`
    /// endpoint).
    UpdatesDisabled,
    /// The update manifest could not be fetched or understood (network, TLS, HTTP status, JSON,
    /// no entry for this platform).
    UpdateCheckFailed,
    /// `update_install` without a current update from `update_check` / `update:available`.
    UpdateNotFound,
    /// Another download or install is already running.
    UpdateInProgress,
    /// This installation cannot replace itself (Linux .deb/.rpm): download it from the release page.
    UpdateManualOnly,
    /// The download failed (network, HTTP status).
    UpdateDownloadFailed,
    /// The downloaded file does not carry a valid signature from the release key, or was signed
    /// for another version. Nothing was installed.
    UpdateVerifyFailed,
    /// The verified update could not be installed; the current version keeps running.
    UpdateInstallFailed,
}

/// Why a bookmark import file was refused as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ImportInvalidReason {
    NotJson,
    UnknownFormat,
    UnsupportedVersion,
    TooLarge,
    TooMany,
    BadStructure,
}

/// The local shell a copyable command is quoted for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum HintShell {
    /// Windows: PowerShell single quotes (`'` and the typographic quotes doubled).
    Powershell,
    /// Unix: POSIX `sh` single quotes.
    Posix,
}

impl HintShell {
    pub const fn local() -> Self {
        if cfg!(windows) {
            Self::Powershell
        } else {
            Self::Posix
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum EntityKind {
    Bookmark,
    Group,
    SshConfigHost,
    Transfer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum IoOp {
    ReadConfig,
    ReadKnownHosts,
    WriteKnownHosts,
    ReadKey,
    ReadStorage,
    WriteStorage,
    ReadLocalFile,
    WriteLocalFile,
    RevealPath,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TimeoutOp {
    Connect,
    SftpRequest,
    Listing,
    Prompt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum KeyLoadReason {
    NotFound,
    PermissionDenied,
    Unsupported,
    Corrupt,
    InsecurePermissions,
    NotAFile,
    TooLarge,
    UntrustedPath,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum DisconnectKind {
    KeepaliveTimeout,
    ConnectionReset,
    ServerClosed,
    NetworkError,
    JumpHostLost,
    /// The user chose Disconnect (`session_disconnect`).
    UserRequested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum AuthMethodKind {
    None,
    Publickey,
    Agent,
    Password,
    KeyboardInteractive,
}

impl AppError {
    pub fn new(kind: ErrorCode) -> Self {
        Self { kind, detail: None }
    }

    /// Attach an English developer detail (logs / "Copy details" only).
    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn internal(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal).with_detail(detail)
    }

    /// A caught panic in `command` (§4.3, `commands::guarded`).
    pub fn bug(command: impl Into<String>) -> Self {
        Self::new(ErrorCode::Bug {
            command: command.into(),
        })
    }

    /// `bug{context}` for a panic just caught: the detail is the location only, never the
    /// payload (it may contain server data or secrets). `context` is bounded and escaped.
    pub fn panicked(context: &str, location: Option<&crate::logging::PanicLocation>) -> Self {
        let error = Self::bug(crate::logging::sanitize_for_log(context, 64));
        match location {
            Some(location) => error.with_detail(format!("panicked at {location}")),
            None => error.with_detail("panicked"),
        }
    }

    pub fn invalid_argument(field: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidArgument {
            field: field.into(),
        })
    }

    /// Map a local I/O error. The OS error text goes to `detail` only.
    pub fn io(op: IoOp, path: Option<String>, err: &std::io::Error) -> Self {
        Self::new(ErrorCode::Io { op, path }).with_detail(err.to_string())
    }

    pub fn code(&self) -> &ErrorCode {
        &self.kind
    }
}

impl From<ErrorCode> for AppError {
    fn from(kind: ErrorCode) -> Self {
        Self::new(kind)
    }
}

/// Run `f` and turn a panic into `bug{context}` (location from the panic hook, which ran on this
/// thread). Use it inside `spawn_blocking` closures: tokio's `JoinError` Display carries the
/// panic payload, so a panic must never reach a `JoinError` that is formatted.
pub fn catch_panic<T>(context: &str, f: impl FnOnce() -> T) -> Result<T, AppError> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(value) => Ok(value),
        Err(_) => {
            let location = crate::logging::take_panic_location();
            tracing::error!(
                context = %crate::logging::sanitize_for_log(context, 64),
                "a background task panicked"
            );
            Err(AppError::panicked(context, location.as_ref()))
        }
    }
}

/// A background task that did not finish, without the `JoinError` text (its Display carries the
/// panic payload): `bug{context}` for a panic, else `internal`.
pub fn join_failed(panicked: bool, context: &str) -> AppError {
    if panicked {
        AppError::panicked(context, None)
    } else if context.is_empty() {
        AppError::internal("background task cancelled")
    } else {
        AppError::internal(format!(
            "{}: background task cancelled",
            crate::logging::sanitize_for_log(context, 64)
        ))
    }
}

/// Whether a Tauri `JoinHandle` failed because its task panicked.
pub fn tauri_task_panicked(e: &tauri::Error) -> bool {
    matches!(e, tauri::Error::JoinError(j) if j.is_panic())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn to_json(e: &AppError) -> serde_json::Value {
        serde_json::to_value(e).unwrap()
    }

    #[test]
    fn unit_variant_has_no_params() {
        assert_eq!(
            to_json(&AppError::new(ErrorCode::Cancelled)),
            json!({ "code": "cancelled" })
        );
        assert_eq!(
            to_json(&AppError::new(ErrorCode::AgentConstraintUnsupported)),
            json!({ "code": "agentConstraintUnsupported" })
        );
    }

    #[test]
    fn bug_carries_the_command_name() {
        assert_eq!(
            to_json(&AppError::bug("sftp_list").with_detail("panicked at src/x.rs:1:2")),
            json!({
                "code": "bug",
                "params": { "command": "sftp_list" },
                "detail": "panicked at src/x.rs:1:2"
            })
        );
    }

    /// Logs written on this thread while `f` runs.
    fn captured_logs(f: impl FnOnce()) -> String {
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::layer::SubscriberExt;
        #[derive(Clone, Default)]
        struct Buf(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Buf {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let buf = Buf::default();
        let sink = buf.clone();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(move || sink.clone()),
        );
        tracing::subscriber::with_default(subscriber, f);
        String::from_utf8(buf.0.lock().unwrap().clone()).unwrap()
    }

    #[test]
    fn caught_panics_keep_their_payload_out_of_errors_and_logs() {
        crate::logging::install_test_panic_hook();
        let mut result = None;
        let logs = captured_logs(|| {
            result = Some(catch_panic("known_hosts", || -> u8 {
                panic!("test panic, payload marker PAYLOAD-44d1")
            }));
        });
        let err = result.unwrap().unwrap_err();
        assert_eq!(
            err.kind,
            ErrorCode::Bug {
                command: "known_hosts".into()
            }
        );
        let detail = err.detail.clone().unwrap();
        assert!(
            detail.starts_with("panicked at ") && detail.contains("error.rs"),
            "{detail}"
        );
        assert!(
            !serde_json::to_string(&err)
                .unwrap()
                .contains("PAYLOAD-44d1")
        );
        assert!(logs.contains("a background task panicked"), "{logs}");
        assert!(!logs.contains("PAYLOAD-44d1"), "{logs}");
        assert_eq!(catch_panic("x", || 3).unwrap(), 3);
    }

    #[tokio::test]
    async fn join_failures_never_use_the_join_error_text() {
        crate::logging::install_test_panic_hook();
        let joined =
            tokio::spawn(async { panic!("test panic, payload marker PAYLOAD-5b70") }).await;
        let e = joined.unwrap_err();
        // tokio's text carries the payload: exactly why it is never formatted.
        assert!(e.to_string().contains("PAYLOAD-5b70"));
        let err = join_failed(e.is_panic(), "transfer");
        assert_eq!(
            err.kind,
            ErrorCode::Bug {
                command: "transfer".into()
            }
        );
        assert!(
            !serde_json::to_string(&err)
                .unwrap()
                .contains("PAYLOAD-5b70")
        );
        let cancelled = join_failed(false, "transfer");
        assert_eq!(cancelled.kind, ErrorCode::Internal);
        assert_eq!(
            cancelled.detail.as_deref(),
            Some("transfer: background task cancelled")
        );
        assert_eq!(
            join_failed(false, "").detail.as_deref(),
            Some("background task cancelled")
        );
        let tauri_err = tauri::Error::JoinError(e);
        assert!(tauri_task_panicked(&tauri_err));
    }

    #[test]
    fn struct_variant_camel_case_params_and_detail() {
        let e = AppError::new(ErrorCode::HostKeyChanged {
            host: "h".into(),
            port: 22,
            key_type: "ssh-ed25519".into(),
            fingerprint: "SHA256:abc".into(),
            known_hosts_file: "/home/u/.ssh/known_hosts".into(),
            line: 7,
            remove_hint: Some("ssh-keygen -R 'h' -f '/home/u/.ssh/known_hosts'".into()),
            remove_hint_shell: HintShell::Posix,
        })
        .with_detail("dev note");
        assert_eq!(
            to_json(&e),
            json!({
                "code": "hostKeyChanged",
                "params": {
                    "host": "h", "port": 22, "keyType": "ssh-ed25519", "fingerprint": "SHA256:abc",
                    "knownHostsFile": "/home/u/.ssh/known_hosts", "line": 7,
                    "removeHint": "ssh-keygen -R 'h' -f '/home/u/.ssh/known_hosts'",
                    "removeHintShell": "posix"
                },
                "detail": "dev note"
            })
        );
    }

    #[test]
    fn nullable_fields_serialize_as_null_and_enums_camel_case() {
        let e = AppError::new(ErrorCode::Io {
            op: IoOp::RevealPath,
            path: None,
        });
        assert_eq!(
            to_json(&e),
            json!({ "code": "io", "params": { "op": "revealPath", "path": null } })
        );

        let e = AppError::new(ErrorCode::Disconnected {
            reason: DisconnectKind::KeepaliveTimeout,
            message: None,
        });
        assert_eq!(
            to_json(&e),
            json!({ "code": "disconnected", "params": { "reason": "keepaliveTimeout", "message": null } })
        );

        let e = AppError::new(ErrorCode::AuthFailed {
            user: "u".into(),
            host: "h".into(),
            tried: vec![
                AuthMethodKind::Publickey,
                AuthMethodKind::KeyboardInteractive,
            ],
            server_methods: vec![AuthMethodKind::Password],
        });
        assert_eq!(
            to_json(&e),
            json!({ "code": "authFailed", "params": {
                "user": "u", "host": "h", "tried": ["publickey", "keyboardInteractive"], "serverMethods": ["password"]
            } })
        );

        let e = AppError::new(ErrorCode::StorageCorrupt {
            file: "a.json".into(),
            quarantined_as: "b.json".into(),
        });
        assert_eq!(
            to_json(&e),
            json!({ "code": "storageCorrupt", "params": { "file": "a.json", "quarantinedAs": "b.json" } })
        );

        let e = AppError::new(ErrorCode::NotFound {
            what: EntityKind::SshConfigHost,
            id: "x".into(),
        });
        assert_eq!(
            to_json(&e),
            json!({ "code": "notFound", "params": { "what": "sshConfigHost", "id": "x" } })
        );

        let e = AppError::new(ErrorCode::Timeout {
            op: TimeoutOp::SftpRequest,
        });
        assert_eq!(
            to_json(&e),
            json!({ "code": "timeout", "params": { "op": "sftpRequest" } })
        );

        let e = AppError::new(ErrorCode::KeyLoadFailed {
            path: "k".into(),
            reason: KeyLoadReason::NotAFile,
        });
        assert_eq!(
            to_json(&e),
            json!({ "code": "keyLoadFailed", "params": { "path": "k", "reason": "notAFile" } })
        );
    }
}
