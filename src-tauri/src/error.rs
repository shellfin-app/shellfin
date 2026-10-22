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
    /// This build never checks for updates (dev, debug or `e2e` build, or no HTTPS endpoint for
    /// the selected channel).
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

    /// Placeholder for commands whose service lands later. Uses the
    /// existing `internal` code so the contract stays unchanged.
    pub fn not_implemented(command: &str) -> Self {
        Self::internal(format!("not implemented yet: {command}"))
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

    #[test]
    fn not_implemented_uses_internal_code() {
        let e = AppError::not_implemented("session_open");
        assert_eq!(e.kind, ErrorCode::Internal);
        assert_eq!(
            to_json(&e),
            json!({ "code": "internal", "detail": "not implemented yet: session_open" })
        );
    }
}
