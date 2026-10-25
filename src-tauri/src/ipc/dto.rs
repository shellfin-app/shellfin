//! ALL IPC DTOs (§4.4). The TypeScript in §4.3–4.4 is the expected ts-rs output; the generated
//! files in `src/ipc/bindings/` are the contract (`cargo test` regenerates them; CI fails on drift).
//!
//! Direction rule (§4.1): outbound types derive `Serialize`, inbound types derive `Deserialize`,
//! types used in both directions derive both. Types containing [`Secret`] are inbound-only —
//! `Secret` has no `Serialize` impl, so the compiler rejects any outbound use.
//!
//! Inline TS object types of §4.4 (e.g. `Settings.terminal`, `EnvInfo.sshConfig`,
//! `SessionInfo.auth`, kbd-int `fields`) are named Rust structs here; they are exported as
//! named TS types that are structurally identical to the inline shapes.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::AppError;
use crate::ipc::secret::{Secret, SensitiveText};

// ---------------------------------------------------------------------------------------------
// app
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Platform {
    Windows,
    Macos,
    Linux,
}

impl Platform {
    pub const fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Macos
        } else {
            Self::Linux
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum RevealKind {
    SshConfig,
    KnownHosts,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppBootstrap {
    pub app_version: String,
    pub platform: Platform,
    pub settings: Settings,
    pub ssh_config: SshConfigSnapshot,
    pub bookmarks: BookmarksDto,
    pub recents: Vec<RecentEntry>,
    pub warnings: Vec<AppError>,
    /// The window's state at load time (later changes arrive as `window:state`).
    pub window: WindowState,
    /// Bookmarks to reopen (`general.onStartup = "restore"`; otherwise empty). The
    /// bookmark sessions open at the last quit, in opening order, that still exist.
    pub restore_bookmark_ids: Vec<String>,
}

/// Window state for the custom title bar. Payload of the `window:state` event and
/// `AppBootstrap.window`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WindowState {
    pub maximized: bool,
    pub fullscreen: bool,
    pub focused: bool,
}

/// Payload of the `menu:action` event (macOS native menu). `id` is one of the ids
/// listed in `app_menu::MENUS` (and mapped in the frontend's `shellEvents.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MenuActionPayload {
    pub id: String,
}

/// Input of `app_error_report` (§4.2, §9.6). Everything is optional except `includeLog`; the
/// backend bounds, redacts and escapes every text before it goes into the report. With no
/// error fields at all the report holds the environment only (Help → Report a problem).
#[derive(Debug, Clone, Default, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ErrorReportInput {
    /// `AppError.code` as received (`[A-Za-z][A-Za-z0-9]{0,63}`, else shown as `unknown`).
    #[serde(default)]
    #[ts(optional = nullable)]
    pub code: Option<String>,
    /// `AppError.params` as received (any JSON).
    #[serde(default)]
    #[ts(optional = nullable, type = "unknown")]
    pub params: Option<serde_json::Value>,
    /// `AppError.detail` as received.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub detail: Option<String>,
    /// The translated message the dialog shows.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub message: Option<String>,
    /// Where it happened: an action id or command name (`[A-Za-z0-9_.:-]{1,80}`, else dropped).
    #[serde(default)]
    #[ts(optional = nullable)]
    pub context: Option<String>,
    /// A frontend stack trace (at most 100 lines are used).
    #[serde(default)]
    #[ts(optional = nullable)]
    pub stack: Option<String>,
    /// When the error happened (Unix ms).
    #[serde(default)]
    #[ts(optional = nullable)]
    pub occurred_at_ms: Option<i64>,
    /// The UI language (`i18n.language`), `[A-Za-z0-9-]{1,35}`.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub ui_language: Option<String>,
    /// Add the tail of the current log file (≤ 200 lines / 64 KiB; the dialog's checkbox,
    /// off by default).
    pub include_log: bool,
}

/// Result of `app_error_report`: the redacted report and the pre-filled issue form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ErrorReport {
    /// The full report (Markdown) for the preview and the clipboard: exactly what is copied.
    pub markdown: String,
    /// `https://github.com/shellfin-app/shellfin/issues/new?template=bug_report.yml&…` with the
    /// short fields, the error summary and as much of the log tail as fits; ≤ 2048 characters,
    /// accepted by `app_open_external`.
    pub issue_url: String,
    pub environment: ReportEnvironment,
    /// `includeLog` was set and the current log file had lines to include.
    pub log_included: bool,
    /// Log lines in `markdown` (0 without the log).
    pub log_lines: u32,
    /// The newest of those lines that also fit into the issue form's `logs` field.
    pub log_lines_in_url: u32,
}

/// The environment part of an error report (also in `markdown`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReportEnvironment {
    pub app_version: String,
    /// e.g. `Windows 11 24H2 (build 26100.2033)`, `macOS 15.1`, `Ubuntu 24.04.1 LTS (Linux 6.8.0)`.
    pub os: String,
    /// The CPU architecture (`x86_64`, `aarch64`).
    pub arch: String,
    /// How Shellfin was installed, as the bug form's option text; `null` when it cannot be told
    /// (macOS DMG vs Homebrew, an MSI built by hand).
    pub install: Option<String>,
    pub ui_language: Option<String>,
}

/// Result of `app_save_text_file`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SaveTextOutcome {
    /// `false` when the user cancelled the dialog (nothing written).
    pub saved: bool,
    /// Base name of the written file (for a toast); `null` when cancelled.
    pub file_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EnvInfo {
    pub ssh_config: EnvSshConfigInfo,
    pub known_hosts: EnvKnownHostsInfo,
    pub agent: AgentStatus,
    pub log_dir: String,
}

/// `EnvInfo.sshConfig`.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EnvSshConfigInfo {
    pub path: String,
    /// The file exists (a `0 hosts` card can say "no ~/.ssh/config yet").
    pub exists: bool,
    pub host_count: u32,
    pub include_count: u32,
    pub loaded_at_ms: i64,
}

/// `EnvInfo.knownHosts`.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EnvKnownHostsInfo {
    pub path: String,
    pub entry_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum AgentKind {
    OpensshPipe,
    NamedPipe,
    UnixSocket,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentStatus {
    pub available: bool,
    pub kind: Option<AgentKind>,
    pub endpoint: Option<String>,
    pub identities: Vec<AgentKey>,
    /// e.g. `agentUntrusted`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub warning: Option<AppError>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentKey {
    pub key_type: String,
    pub fingerprint: String,
    pub comment: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub bits: Option<u32>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LocalKeyInfo {
    pub path: String,
    pub key_type: Option<String>,
    pub fingerprint: Option<String>,
    pub encrypted: bool,
    pub in_agent: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<AppError>,
}

// ---------------------------------------------------------------------------------------------
// ssh config
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SourceLocation {
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SshConfigSnapshot {
    pub main_file: String,
    pub files: Vec<String>,
    pub hosts: Vec<SshConfigHost>,
    pub warnings: Vec<ConfigWarning>,
    /// `badLine` warnings not in `warnings` because their file reached the cap of 50
    /// (all files together).
    pub dropped_warnings: u32,
    pub loaded_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SshConfigHost {
    pub alias: String,
    pub hostname: String,
    pub user: Option<String>,
    pub port: u16,
    pub proxy_jump: Option<String>,
    pub identity_files: Vec<String>,
    pub origin: SourceLocation,
    pub has_warnings: bool,
}

/// `{ location: SourceLocation } & ConfigWarningCode`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConfigWarning {
    pub location: SourceLocation,
    #[serde(flatten)]
    pub kind: ConfigWarningCode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "code", content = "params", rename_all = "camelCase")]
#[ts(export)]
pub enum ConfigWarningCode {
    #[serde(rename_all = "camelCase")]
    UnsupportedMatchCriteria { criteria: String },
    #[serde(rename_all = "camelCase")]
    ProxyCommandUnsupported { alias: String },
    #[serde(rename_all = "camelCase")]
    IncludeDepthExceeded { pattern: String },
    #[serde(rename_all = "camelCase")]
    IncludeRecursiveGlob { pattern: String },
    #[serde(rename_all = "camelCase")]
    IncludeTooManyFiles { pattern: String },
    #[serde(rename_all = "camelCase")]
    IncludeCycle { file: String },
    #[serde(rename_all = "camelCase")]
    UntrustedFile { file: String },
    #[serde(rename_all = "camelCase")]
    FileTooLarge { file: String },
    #[serde(rename_all = "camelCase")]
    BadLine { text: String },
    #[serde(rename_all = "camelCase")]
    UnknownToken { token: String },
    #[serde(rename_all = "camelCase")]
    InvalidHostname { value: String },
    /// `User` with `%` tokens or `${ENV}` could not be expanded, or the result is not a safe
    /// user name. `value` is the config text as written (never the expanded value: a
    /// variable may hold a secret), shortened to 255 characters.
    #[serde(rename_all = "camelCase")]
    InvalidUser { value: String },
    /// A known token whose value Shellfin cannot compute exactly like OpenSSH here (`%C`,
    /// `%l`/`%L` without a usable local host name, `%C` with `CanonicalizeHostname`, `%h`/`%C`
    /// of a non-canonical numeric IPv4, `%u`/`%r`/`%C` when the local account name is not
    /// OpenSSH's, …): the option is ignored (known_hosts options fall back and the hop asks).
    #[serde(rename_all = "camelCase")]
    UnavailableToken { token: String },
    /// A file option expanded to more than 4096 bytes and was ignored. `option` = the
    /// keyword as OpenSSH spells it (`IdentityFile`, `UserKnownHostsFile`, …).
    #[serde(rename_all = "camelCase")]
    ExpansionTooLong { option: String },
}

// ---------------------------------------------------------------------------------------------
// bookmarks
// ---------------------------------------------------------------------------------------------

/// Outbound auth of a stored bookmark. `passwordSaved` is backend-owned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "method", rename_all = "camelCase")]
#[ts(export)]
pub enum AuthConfig {
    Auto,
    #[serde(rename_all = "camelCase")]
    Key {
        identity_file: String,
    },
    #[serde(rename_all = "camelCase")]
    Password {
        password_saved: bool,
    },
    Agent,
}

/// Inbound auth from the bookmark dialog.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
#[serde(tag = "method", rename_all = "camelCase")]
#[ts(export)]
pub enum AuthInput {
    Auto,
    #[serde(rename_all = "camelCase")]
    Key {
        identity_file: String,
    },
    Password,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum JumpRef {
    #[serde(rename_all = "camelCase")]
    SshConfig { alias: String },
    #[serde(rename_all = "camelCase")]
    Bookmark { id: String },
    #[serde(rename_all = "camelCase")]
    Spec { spec: String },
}

/// Bookmark/session accent color: a fixed palette of names; the UI maps them to tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum BookmarkColor {
    Red,
    Orange,
    Yellow,
    Green,
    Teal,
    Blue,
    Purple,
    Gray,
}

impl BookmarkColor {
    pub const ALL: [Self; 8] = [
        Self::Red,
        Self::Orange,
        Self::Yellow,
        Self::Green,
        Self::Teal,
        Self::Blue,
        Self::Purple,
        Self::Gray,
    ];
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Bookmark {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub auth: AuthConfig,
    pub proxy_jump: Option<JumpRef>,
    pub group_id: Option<String>,
    pub tags: Vec<String>,
    pub start_dir: Option<String>,
    pub use_ssh_config: bool,
    /// Accent color (`null` = none).
    pub color: Option<BookmarkColor>,
    /// 0-based position among the bookmarks with the same `groupId` (dense).
    pub order: i32,
    /// Per-bookmark bandwidth override (see `BookmarkInput`); `null` = global.
    #[serde(rename = "uploadLimitKiBps")]
    pub upload_limit_kibps: Option<u32>,
    #[serde(rename = "downloadLimitKiBps")]
    pub download_limit_kibps: Option<u32>,
    /// Ask before every user-initiated connect (UI only).
    pub confirm_connect: bool,
    /// One line (≤ 1024 chars) the frontend sends to the first terminal; never
    /// logged, imported only when the user opts in per import (§7.7).
    pub startup_command: Option<SensitiveText>,
    /// Plain text, ≤ 4000 chars.
    pub notes: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BookmarkInput {
    pub id: Option<String>,
    pub name: String,
    pub host: String,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub auth: AuthInput,
    pub proxy_jump: Option<JumpRef>,
    pub group_id: Option<String>,
    pub tags: Vec<String>,
    pub start_dir: Option<String>,
    pub use_ssh_config: bool,
    /// Accent color; `null` or omitted = none.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub color: Option<BookmarkColor>,
    /// Per-bookmark bandwidth override in KiB/s. `null` = use the global setting,
    /// `0` = unlimited for this bookmark, 1–1048576 = limit.
    #[serde(default, rename = "uploadLimitKiBps")]
    #[ts(optional = nullable)]
    pub upload_limit_kibps: Option<u32>,
    #[serde(default, rename = "downloadLimitKiBps")]
    #[ts(optional = nullable)]
    pub download_limit_kibps: Option<u32>,
    /// Omitted or `null` = `false`.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub confirm_connect: Option<bool>,
    /// ≤ 1024 chars, no control characters; blank or omitted = none.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub startup_command: Option<SensitiveText>,
    /// ≤ 4000 chars (LF/CR/tab allowed); blank or omitted = none.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Group {
    pub id: String,
    pub name: String,
    /// 0-based position among the groups (dense).
    pub order: i32,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GroupInput {
    pub id: Option<String>,
    pub name: String,
    /// Ignored since schema v2 (new groups are appended, updates keep their place); use `group_move`.
    pub order: i32,
}

#[derive(Debug, Clone, Default, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BookmarksDto {
    pub bookmarks: Vec<Bookmark>,
    pub groups: Vec<Group>,
    pub read_only: bool,
}

/// Secret-free target, stored in recents and in the frontend store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum RecentTarget {
    #[serde(rename_all = "camelCase")]
    SshConfig { alias: String },
    #[serde(rename_all = "camelCase")]
    Bookmark { id: String },
    #[serde(rename_all = "camelCase")]
    Quick { spec: String },
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RecentEntry {
    pub target: RecentTarget,
    pub label: String,
    pub user_at_host: String,
    pub via: Option<String>,
    pub last_connected_at_ms: i64,
    pub last_dir: Option<String>,
    pub terminal_count: u32,
}

// ---------------------------------------------------------------------------------------------
// settings (ranges enforced on update, clamped on load — `storage::settings`)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Settings {
    /// Schema version (1).
    pub version: u32,
    /// `"system"` or a BCP-47 tag; the app ships `"en"` and `"uk"`.
    pub locale: String,
    pub terminal: TerminalSettings,
    pub editor: EditorSettings,
    pub connection: ConnectionSettings,
    pub files: FilesSettings,
    pub ui: UiSettings,
    /// SFTP bandwidth limits. Missing in older files ⇒ defaults (unlimited).
    pub transfers: TransferSettings,
    /// General behaviour. Missing ⇒ defaults.
    #[serde(default)]
    pub general: GeneralSettings,
    /// Advanced. Missing ⇒ defaults.
    #[serde(default)]
    pub advanced: AdvancedSettings,
    /// Appearance. Missing ⇒ defaults.
    #[serde(default)]
    pub appearance: AppearanceSettings,
}

fn default_true() -> bool {
    true
}

fn default_ui_font_size() -> u32 {
    13
}

fn default_minimum_contrast_ratio() -> f64 {
    1.0
}

/// `terminal.wordSeparators` default (space first).
pub const DEFAULT_WORD_SEPARATORS: &str = " ()[]{}'\"";

fn default_word_separators() -> String {
    DEFAULT_WORD_SEPARATORS.to_owned()
}

fn default_scroll_sensitivity() -> u32 {
    1
}

/// `general.confirmQuit`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ConfirmQuit {
    Always,
    #[default]
    WithConnections,
    Never,
}

/// `general.onStartup`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum StartupAction {
    #[default]
    Welcome,
    /// Reopen the bookmarks that were open at the last quit (`AppBootstrap.restoreBookmarkIds`).
    Restore,
}

/// `appearance.theme`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
    System,
}

/// `appearance.accent`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum AccentColor {
    #[default]
    Teal,
    Blue,
    Purple,
    Orange,
    Green,
    Pink,
}

/// `appearance.reduceMotion`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ReduceMotion {
    #[default]
    System,
    On,
    Off,
}

/// UI only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppearanceSettings {
    #[serde(default)]
    pub theme: ThemeMode,
    #[serde(default)]
    pub accent: AccentColor,
    /// 12–16 px (UI only, not terminal/editor).
    #[serde(default = "default_ui_font_size")]
    pub ui_font_size: u32,
    #[serde(default)]
    pub reduce_motion: ReduceMotion,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: ThemeMode::Dark,
            accent: AccentColor::Teal,
            ui_font_size: 13,
            reduce_motion: ReduceMotion::System,
        }
    }
}

/// `terminal.onExit`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TerminalExitAction {
    #[default]
    Keep,
    CloseOnSuccess,
    Close,
}

/// `terminal.newTerminalCwd`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum NewTerminalCwd {
    #[default]
    Home,
    CurrentFolder,
}

/// `terminal.termType`, the `TERM` of the PTY request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum TermType {
    #[default]
    #[serde(rename = "xterm-256color")]
    Xterm256Color,
    #[serde(rename = "xterm")]
    Xterm,
    #[serde(rename = "vt100")]
    Vt100,
}

impl TermType {
    /// The `TERM` value sent with the PTY request.
    pub const fn as_term(self) -> &'static str {
        match self {
            Self::Xterm256Color => "xterm-256color",
            Self::Xterm => "xterm",
            Self::Vt100 => "vt100",
        }
    }
}

/// `editor.newFileEol`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum NewFileEol {
    #[default]
    Lf,
    Crlf,
}

/// `editor.showWhitespace`: which spaces and tabs the editor marks. Earlier builds stored a
/// boolean; it still loads (`true` = all, `false` = none) and is written back as a string, so the
/// file version stays 1 (§7.2). Any other value is a wrong type (load repair resets the field).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ShowWhitespace {
    #[default]
    None,
    /// Only the lines with a cursor.
    ActiveLine,
    All,
}

impl<'de> Deserialize<'de> for ShowWhitespace {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ModeVisitor;
        impl serde::de::Visitor<'_> for ModeVisitor {
            type Value = ShowWhitespace;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("\"none\", \"activeLine\", \"all\" or a boolean")
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<ShowWhitespace, E> {
                Ok(if v {
                    ShowWhitespace::All
                } else {
                    ShowWhitespace::None
                })
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<ShowWhitespace, E> {
                match v {
                    "none" => Ok(ShowWhitespace::None),
                    "activeLine" => Ok(ShowWhitespace::ActiveLine),
                    "all" => Ok(ShowWhitespace::All),
                    other => Err(E::unknown_variant(other, &["none", "activeLine", "all"])),
                }
            }
        }
        deserializer.deserialize_any(ModeVisitor)
    }
}

/// `files.doubleClick`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum DoubleClickAction {
    #[default]
    Open,
    Download,
}

/// `files.sizeUnits` (KiB/MiB vs KB/MB).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SizeUnits {
    #[default]
    Binary,
    Decimal,
}

/// `files.dateFormat`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum DateFormat {
    #[default]
    Relative,
    Absolute,
}

fn default_line_height() -> f64 {
    1.2
}

fn default_max_concurrent() -> u32 {
    3
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GeneralSettings {
    /// Ask before closing the window while terminals are open.
    #[serde(default = "default_true")]
    pub confirm_close_with_terminals: bool,
    /// When quitting the app asks (unsaved editor changes always ask).
    #[serde(default)]
    pub confirm_quit: ConfirmQuit,
    #[serde(default)]
    pub on_startup: StartupAction,
    /// OS notification when a connection drops while the window is not focused.
    #[serde(default)]
    pub notify_on_disconnect: bool,
    /// Updater: check for updates 15 s after startup and then every 24 h. Manual checks
    /// ("Check for updates…") work either way. Missing ⇒ `true`.
    #[serde(default = "default_true")]
    pub auto_update: bool,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            confirm_close_with_terminals: true,
            confirm_quit: ConfirmQuit::WithConnections,
            on_startup: StartupAction::Welcome,
            notify_on_disconnect: false,
            auto_update: true,
        }
    }
}

/// Log verbosity (applies live). `debug` never adds secrets, terminal data or file contents.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum LogLevel {
    #[default]
    Info,
    Debug,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AdvancedSettings {
    #[serde(default)]
    pub log_level: LogLevel,
}

/// `0` = unlimited. Applies to file transfers (upload/download), not to editor
/// reads/saves or the tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TransferSettings {
    /// 0–1048576 KiB/s
    #[serde(rename = "uploadLimitKiBps")]
    pub upload_limit_kibps: u32,
    /// 0–1048576 KiB/s
    #[serde(rename = "downloadLimitKiBps")]
    pub download_limit_kibps: u32,
    /// Transfers that run at once per session, 1–64 (applies to new transfers).
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: u32,
    /// What to do when an upload's destination exists (`overwrite: true` always
    /// wins). Not applied to downloads through the Save dialog.
    #[serde(default)]
    pub on_conflict: ConflictPolicy,
    /// Upload sets remote mtime/atime, download sets local mtime/atime.
    #[serde(default = "default_true")]
    pub preserve_timestamps: bool,
    /// OS notification when transfers finish while the window is not focused.
    #[serde(default)]
    pub notify_on_complete: bool,
}

/// `transfers.onConflict`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ConflictPolicy {
    /// The UI asks (upload: `remoteExists`; download: the native Save dialog).
    #[default]
    Ask,
    Overwrite,
    Skip,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CursorStyle {
    #[default]
    Block,
    Bar,
    Underline,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TerminalBell {
    #[default]
    Off,
    Visual,
    /// A short synthesized beep.
    Sound,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum RightClickAction {
    #[default]
    Menu,
    Paste,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TerminalSettings {
    /// 8–32
    pub font_size: u32,
    /// 10–1 000 000 (was 1000–100000)
    pub scrollback: u32,
    pub cursor_blink: bool,
    pub copy_on_select: bool,
    /// CSS font-family list (names only, ≤ 200 chars); `null` = the bundled JetBrains Mono.
    pub font_family: Option<String>,
    #[serde(default)]
    pub cursor_style: CursorStyle,
    /// 1.0–2.0.
    #[serde(default = "default_line_height")]
    pub line_height: f64,
    #[serde(default)]
    pub bell: TerminalBell,
    #[serde(default)]
    pub right_click: RightClickAction,
    /// 0–5 px.
    #[serde(default)]
    pub letter_spacing: u32,
    #[serde(default = "default_true")]
    pub mouse_reporting: bool,
    #[serde(default)]
    pub hide_mouse_pointer_while_typing: bool,
    /// 1–21 (1 = off).
    #[serde(default = "default_minimum_contrast_ratio")]
    pub minimum_contrast_ratio: f64,
    /// ≤ 64 printable ASCII characters.
    #[serde(default = "default_word_separators")]
    pub word_separators: String,
    #[serde(default = "default_true")]
    pub multiline_paste_warning: bool,
    #[serde(default)]
    pub on_exit: TerminalExitAction,
    #[serde(default)]
    pub new_terminal_cwd: NewTerminalCwd,
    #[serde(default = "default_true")]
    pub open_terminal_on_connect: bool,
    #[serde(default)]
    pub term_type: TermType,
    #[serde(default)]
    pub alt_is_meta: bool,
    /// 1–10.
    #[serde(default = "default_scroll_sensitivity")]
    pub scroll_sensitivity: u32,
    /// `null` = follow the theme, else `#RRGGBB`.
    #[serde(default)]
    pub background: Option<String>,
    /// xterm `screenReaderMode`: mirrors the output into an accessible DOM region for screen readers.
    #[serde(default)]
    pub screen_reader_mode: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EditorSettings {
    /// 8–32
    pub font_size: u32,
    /// 1–8
    pub tab_size: u32,
    pub word_wrap: bool,
    /// 1–256 (was 1–50). The backend read and save cap.
    #[serde(rename = "maxFileSizeMiB")]
    pub max_file_size_mib: u32,
    /// Same rules as `terminal.fontFamily`; `null` = the bundled font.
    #[serde(default)]
    pub font_family: Option<String>,
    #[serde(default = "default_true")]
    pub line_numbers: bool,
    #[serde(default = "default_true")]
    pub highlight_active_line: bool,
    #[serde(default)]
    pub indent_with_tabs: bool,
    #[serde(default)]
    pub trim_trailing_whitespace: bool,
    #[serde(default)]
    pub insert_final_newline: bool,
    #[serde(default = "default_true")]
    pub detect_indentation: bool,
    #[serde(default)]
    pub new_file_eol: NewFileEol,
    #[serde(default)]
    pub auto_close_brackets: bool,
    #[serde(default = "default_true")]
    pub bracket_matching: bool,
    /// `none | activeLine | all`; a stored boolean of earlier builds loads as `all` / `none`.
    #[serde(default)]
    pub show_whitespace: ShowWhitespace,
    #[serde(default = "default_true")]
    pub code_folding: bool,
    /// Programming ligatures in the editor font (CSS only). Default off. There is no terminal counterpart.
    #[serde(default)]
    pub font_ligatures: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConnectionSettings {
    /// 5–300
    pub keepalive_interval_secs: u32,
    /// 1–10
    pub keepalive_count_max: u32,
    /// 3–120
    pub connect_timeout_secs: u32,
    /// 30–110
    pub prompt_timeout_secs: u32,
    pub reconnect: ReconnectSettings,
    /// `""` or a valid bookmark user (UI prefill only).
    #[serde(default)]
    pub default_user: String,
    /// `false` = never read `~/.ssh/config` or the system `ssh_config`.
    #[serde(default = "default_true")]
    pub read_ssh_config: bool,
    /// `false` = never store a new secret in the OS keychain.
    #[serde(default = "default_true")]
    pub remember_passwords: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReconnectSettings {
    pub enabled: bool,
    /// 0–10
    pub max_attempts: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FilesSettings {
    pub show_hidden: bool,
    #[serde(default = "default_true")]
    pub folders_first: bool,
    /// Ask before deleting remote files.
    #[serde(default = "default_true")]
    pub confirm_delete: bool,
    #[serde(default)]
    pub double_click: DoubleClickAction,
    #[serde(default)]
    pub size_units: SizeUnits,
    #[serde(default)]
    pub date_format: DateFormat,
    #[serde(default)]
    pub show_permissions: bool,
    /// Seconds: 0 (off), 5, 10, 30 or 60.
    #[serde(default)]
    pub auto_refresh: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UiSettings {
    pub sidebar_width: f64,
    pub terminal_panel_height: f64,
    pub terminal_panel_open: bool,
}

// ---------------------------------------------------------------------------------------------
// sessions (inbound types may contain secrets; never stored in the frontend store)
// ---------------------------------------------------------------------------------------------

/// `RecentTarget | { kind: "draft"; bookmark: BookmarkInput; password: string | null }` — inbound only.
#[derive(Debug, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Target {
    #[serde(rename_all = "camelCase")]
    SshConfig { alias: String },
    #[serde(rename_all = "camelCase")]
    Bookmark { id: String },
    #[serde(rename_all = "camelCase")]
    Quick { spec: String },
    #[serde(rename_all = "camelCase")]
    Draft {
        // Boxed only to keep the enum small; transparent on the wire and in TS.
        bookmark: Box<BookmarkInput>,
        password: Option<Secret<String>>,
    },
}

#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct OpenSessionRequest {
    pub target: Target,
    pub probe_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HopRef {
    pub index: u32,
    pub label: String,
    pub user: String,
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum HostKeyVerification {
    KnownHosts,
    UserConfirmed,
    AutoAccepted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HostKeyInfo {
    pub key_type: String,
    pub fingerprint: String,
    pub verification: HostKeyVerification,
}

/// `HopInfo extends HopRef { hostKey }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HopInfo {
    #[serde(flatten)]
    pub hop: HopRef,
    pub host_key: HostKeyInfo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum AuthMethodUsed {
    /// The server accepted `none` (no credentials at all).
    None,
    Publickey,
    Agent,
    Password,
    KeyboardInteractive,
}

/// From `homeDir`: `"/…"` and not `/^\/[A-Za-z]:/` ⇒ `posix`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum RemotePathStyle {
    Posix,
    Windows,
    Unknown,
}

/// `SessionInfo.auth`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SessionAuthInfo {
    pub method: AuthMethodUsed,
    pub key_type: Option<String>,
    pub key_source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SessionInfo {
    pub session_id: String,
    pub label: String,
    /// Last = destination.
    pub hops: Vec<HopInfo>,
    pub auth: SessionAuthInfo,
    pub sftp_available: bool,
    pub home_dir: Option<String>,
    pub start_dir: Option<String>,
    /// "Open terminal here" only when `posix`.
    pub remote_path_style: RemotePathStyle,
    /// The remote account's numeric ids, from a fixed `id -u; id -G` exec at
    /// connect/reconnect. `null` when unknown (non-POSIX server such as Windows OpenSSH, exec
    /// disabled, timeout, unparsable output). Used for "probably not writable" hints only.
    pub remote_identity: Option<RemoteIdentity>,
    /// The bookmark bandwidth overrides captured when the session was opened (a
    /// later bookmark edit does not change a running session). The effective limit of a
    /// direction is `override ?? settings.transfers.<dir>LimitKibps` (the global part is live),
    /// where `0` = unlimited.
    pub limit_overrides: LimitOverridesInfo,
    /// Accent color of the bookmark the session was opened from (`null` otherwise).
    pub color: Option<BookmarkColor>,
}

/// `SessionInfo.limitOverrides`: `null` = no override (follows the global setting).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LimitOverridesInfo {
    pub upload_kibps: Option<u32>,
    pub download_kibps: Option<u32>,
}

/// Numeric identity of the remote user. `gids` are the primary and supplementary
/// groups as reported by `id -G`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoteIdentity {
    pub uid: u32,
    pub gids: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ReconnectStage {
    Waiting,
    Connecting,
    Authenticating,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(tag = "phase", rename_all = "camelCase")]
#[ts(export)]
pub enum SessionState {
    Resolving,
    #[serde(rename_all = "camelCase")]
    Connecting {
        hop: HopRef,
    },
    #[serde(rename_all = "camelCase")]
    Authenticating {
        hop: HopRef,
    },
    /// Every hop is authenticated; the SFTP subsystem and the remote identity are being
    /// set up (up to 10 s). Emitted between `authenticating` and `connected` of a first connect
    /// only (a reconnect stays in `reconnecting{stage:"authenticating"}` until `connected`).
    #[serde(rename_all = "camelCase")]
    OpeningChannels {
        hop: HopRef,
    },
    #[serde(rename_all = "camelCase")]
    Connected {
        info: SessionInfo,
    },
    #[serde(rename_all = "camelCase")]
    Reconnecting {
        stage: ReconnectStage,
        hop: Option<HopRef>,
        attempt: u32,
        max_attempts: u32,
        next_attempt_at_ms: Option<i64>,
        cause: AppError,
    },
    #[serde(rename_all = "camelCase")]
    Disconnected {
        error: Option<AppError>,
        can_retry: bool,
    },
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PromptLastError {
    PassphraseIncorrect,
    PasswordIncorrect,
}

/// One keyboard-interactive prompt field (`Prompt.keyboardInteractive.fields[]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct KbdInteractiveField {
    pub label: String,
    pub echo: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Prompt {
    #[serde(rename_all = "camelCase")]
    HostKeyUnknown {
        id: String,
        hop: HopRef,
        known_hosts_name: String,
        key_type: String,
        fingerprint: String,
        known_hosts_file: Option<String>,
        other_key_types_known: Vec<String>,
        can_remember: bool,
    },
    #[serde(rename_all = "camelCase")]
    Passphrase {
        id: String,
        hop: HopRef,
        key_path: String,
        attempts_left: u32,
        last_error: Option<PromptLastError>,
        agent_available: bool,
        /// false for the Windows OpenSSH pipe (keeps keys permanently, §5.5).
        add_to_agent_supported: bool,
        add_to_agent_default: bool,
    },
    #[serde(rename_all = "camelCase")]
    Password {
        id: String,
        hop: HopRef,
        user: String,
        host: String,
        attempts_left: u32,
        last_error: Option<PromptLastError>,
        can_remember: bool,
        saved_password_available: bool,
        server_label: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    KeyboardInteractive {
        id: String,
        hop: HopRef,
        name: String,
        instructions: String,
        fields: Vec<KbdInteractiveField>,
    },
}

/// Inbound only — contains secrets.
#[derive(Debug, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum PromptAnswer {
    #[serde(rename_all = "camelCase")]
    HostKey {
        accept: bool,
        remember: bool,
    },
    #[serde(rename_all = "camelCase")]
    Passphrase {
        passphrase: Secret<String>,
        add_to_agent: bool,
    },
    #[serde(rename_all = "camelCase")]
    Password {
        password: Secret<String>,
        remember: bool,
    },
    UseSavedPassword,
    #[serde(rename_all = "camelCase")]
    KeyboardInteractive {
        responses: Vec<Secret<String>>,
    },
    /// Skip this key / method, continue auth.
    Skip,
    /// Abort the connection.
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PromptClosedReason {
    Answered,
    Timeout,
    ConnectionLost,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum SessionEvent {
    #[serde(rename_all = "camelCase")]
    State { state: SessionState },
    #[serde(rename_all = "camelCase")]
    Prompt { prompt: Prompt },
    #[serde(rename_all = "camelCase")]
    PromptClosed {
        prompt_id: String,
        reason: PromptClosedReason,
    },
    /// Server text, ≤ 8 KiB, sanitized for display by the frontend.
    #[serde(rename_all = "camelCase")]
    Banner { hop: HopRef, text: String },
    #[serde(rename_all = "camelCase")]
    Stats {
        tx_bytes_per_sec: u64,
        rx_bytes_per_sec: u64,
    },
    #[serde(rename_all = "camelCase")]
    Warning { warning: AppError },
    /// The bookmark this session was opened from was saved. Limits are already applied to the
    /// running session; `needs_reconnect` = connection-relevant fields differ from the live ones.
    #[serde(rename_all = "camelCase")]
    BookmarkUpdated {
        label: String,
        color: Option<BookmarkColor>,
        limit_overrides: LimitOverridesInfo,
        needs_reconnect: bool,
    },
}

// ---------------------------------------------------------------------------------------------
// terminals (`TermMessage = ArrayBuffer | TermEvent` is defined on the TS side)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TermSize {
    pub cols: u32,
    pub rows: u32,
    pub pixel_width: u32,
    pub pixel_height: u32,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum TermEvent {
    #[serde(rename_all = "camelCase")]
    Exit {
        code: Option<u32>,
        signal: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Closed {
        error: Option<AppError>,
    },
    Reconnected,
    /// Backlog overflow (§4.5).
    #[serde(rename_all = "camelCase")]
    OutputDropped {
        bytes: u64,
    },
}

// ---------------------------------------------------------------------------------------------
// sftp
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum FileKind {
    File,
    Dir,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub kind: FileKind,
    pub link_target_kind: Option<FileKind>,
    pub size: Option<u64>,
    /// Unix seconds.
    pub mtime: Option<i64>,
    pub permissions: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    /// Owner name from the `readdir` long name when it is consistent with the
    /// attributes; `null` otherwise (show `uid`). Server text.
    pub owner: Option<String>,
    /// Group name, same rules as `owner` (show `gid` when `null`).
    pub group: Option<String>,
    pub lossy_name: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DirListing {
    pub path: String,
    pub entries: Vec<FileEntry>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Etag {
    pub size: u64,
    /// Unix seconds.
    pub mtime: i64,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Eol {
    Lf,
    Crlf,
    Cr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TextDocument {
    pub path: String,
    pub text: String,
    pub encoding: String,
    pub has_bom: bool,
    pub eol: Eol,
    pub mixed_eol: bool,
    pub permissions: Option<u32>,
    pub etag: Etag,
    /// Only when `path` is a symlink: the file that was read (server `realpath`).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub resolved_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SaveRequest {
    pub path: String,
    pub text: String,
    pub encoding: String,
    pub has_bom: bool,
    pub eol: Eol,
    pub expected: Option<Etag>,
    pub force: bool,
    /// The file the buffer was read from (`TextDocument.resolvedPath ?? path`). If `path` now
    /// resolves elsewhere, the save is a conflict unless `force`. Omitted ⇒ not checked.
    #[serde(default)]
    #[ts(optional)]
    pub expected_target: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SaveStrategy {
    Atomic,
    InPlace,
    RenameFallback,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SaveResult {
    pub etag: Etag,
    pub strategy: SaveStrategy,
    pub warnings: Vec<AppError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TransferDirection {
    Upload,
    Download,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum TransferEvent {
    #[serde(rename_all = "camelCase")]
    Started {
        transfer_id: String,
        name: String,
        direction: TransferDirection,
        total: Option<u64>,
    },
    #[serde(rename_all = "camelCase")]
    Progress {
        transfer_id: String,
        bytes: u64,
        total: Option<u64>,
    },
    #[serde(rename_all = "camelCase")]
    Done {
        transfer_id: String,
        bytes: u64,
        entry: Option<FileEntry>,
    },
    #[serde(rename_all = "camelCase")]
    Failed {
        transfer_id: String,
        error: AppError,
        retryable: bool,
    },
    #[serde(rename_all = "camelCase")]
    Cancelled { transfer_id: String },
    /// `transfers.onConflict = "skip"` and the destination exists; nothing written.
    #[serde(rename_all = "camelCase")]
    Skipped { transfer_id: String },
}

// ---------------------------------------------------------------------------------------------
// bookmark import / export
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum BookmarkExportFormat {
    Json,
    SshConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ExportSkipCode {
    NotRepresentable,
    UnsafeValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExportSkipped {
    pub name: String,
    pub field: String,
    pub code: ExportSkipCode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BookmarksExportResult {
    pub saved: bool,
    pub file_name: Option<String>,
    pub count: u32,
    pub skipped: Vec<ExportSkipped>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ImportStrategy {
    Skip,
    Replace,
    Duplicate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ImportEntryKind {
    Bookmark,
    Group,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ImportInvalidCode {
    Missing,
    WrongType,
    Invalid,
    DuplicateId,
    UnresolvedReference,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImportInvalid {
    pub entry: ImportEntryKind,
    /// Position in the file's `bookmarks` / `groups` array.
    pub index: u32,
    /// DTO field path, e.g. `"host"` or `"auth.identityFile"`.
    pub field: String,
    pub code: ImportInvalidCode,
}

/// A valid startup command found in the import file (shown before anything is saved, so the
/// user can decide whether to keep the file's commands). `Debug`-masked, never logged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImportStartupCommand {
    /// The bookmark's name in the file (it may still be renamed by `duplicate`).
    pub name: String,
    pub command: SensitiveText,
    /// A bookmark with this name (case-insensitive) already exists in the store (the same test
    /// as `BookmarksImportPreview.conflicts`): `skip` skips the entry, `replace` replaces it.
    pub exists: bool,
}

/// What `bookmarks_import_open` found in the chosen file; nothing has been written yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BookmarksImportPreview {
    /// `false` = the dialog was cancelled (`token` is then `null`).
    pub picked: bool,
    /// Pass to `bookmarks_import_apply` / `bookmarks_import_cancel`; single use.
    pub token: Option<String>,
    /// Bookmarks that passed validation.
    pub bookmarks: u32,
    /// Valid bookmarks whose name (case-insensitive) already exists in the store.
    pub conflicts: u32,
    /// Entries rejected while parsing the file.
    pub invalid: Vec<ImportInvalid>,
    /// Valid bookmarks that carry a valid startup command (file order).
    pub startup_commands: Vec<ImportStartupCommand>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BookmarksImportResult {
    /// Always `true`: the file was already picked in `bookmarks_import_open`.
    pub picked: bool,
    pub added: u32,
    pub replaced: u32,
    pub skipped: u32,
    pub invalid: Vec<ImportInvalid>,
    /// The store after the import when something changed.
    pub bookmarks: Option<BookmarksDto>,
    /// Names of imported (added or replaced) bookmarks whose `startupCommand` was
    /// dropped (not imported unless the user opts in per import; an invalid one is always
    /// dropped).
    pub startup_commands_stripped: Vec<String>,
}

// ---------------------------------------------------------------------------------------------
// OS notifications
// ---------------------------------------------------------------------------------------------

/// Localized notification texts from the frontend (`notifications_set_texts`); the backend never
/// authors prose (§8.2.2). `{{name}}` / `{{count}}` are replaced by the backend.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NotificationTexts {
    pub connection_lost_title: String,
    /// `{{name}}` = the connection's tab label.
    pub connection_lost_body: String,
    pub transfers_done_title: String,
    /// One finished transfer: `{{name}}` = the file name.
    pub transfer_done_body: String,
    /// Several finished transfers: `{{count}}`.
    pub transfers_done_body: String,
}

// ---------------------------------------------------------------------------------------------
// global event payloads (§4.2)
// ---------------------------------------------------------------------------------------------

/// Global event names (`app.emit` → `listen`).
pub mod events {
    pub const SSH_CONFIG_CHANGED: &str = "ssh-config:changed";
    pub const RECENTS_CHANGED: &str = "recents:changed";
    pub const QUIT_REQUESTED: &str = "app:quit-requested";
    pub const DROP_HOVER: &str = "drop:hover";
    pub const DROP_LEAVE: &str = "drop:leave";
    pub const DROP_FILES: &str = "drop:files";
    /// `WindowState` after a maximize/fullscreen/focus change.
    pub const WINDOW_STATE: &str = "window:state";
    /// `MenuActionPayload`.
    pub const MENU_ACTION: &str = "menu:action";
    /// Updater: `UpdateInfo` found by an automatic check (never for a skipped version).
    pub const UPDATE_AVAILABLE: &str = "update:available";
    /// Updater: `AppError` when a downloaded, verified update could not be installed at
    /// the end of the quit flow; the app stays open (sessions already closed).
    pub const UPDATE_INSTALL_FAILED: &str = "update:install-failed";
}

// ---------------------------------------------------------------------------------------------
// updater (§9.8)
// ---------------------------------------------------------------------------------------------

/// How this installation applies an update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum UpdateInstallKind {
    /// Download, verify the signature, install and restart (`update_install`).
    SelfUpdate,
    /// Linux .deb/.rpm: the app cannot replace itself; offer `releaseUrl` instead.
    ManualDownload,
}

/// An available update. `notes` is untrusted server text (render with `displayServerText`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UpdateInfo {
    /// SemVer of the update (no `v` prefix).
    pub version: String,
    pub current_version: String,
    /// Publication date (RFC 3339), if the manifest has one.
    pub date: Option<String>,
    /// Release notes, at most `updater::MAX_NOTES_CHARS` characters.
    pub notes: String,
    /// `notes` was cut (the full text is on the release page).
    pub notes_truncated: bool,
    pub install: UpdateInstallKind,
    /// The GitHub release page of this version (built by the backend from the version).
    pub release_url: String,
}

/// Result of a manual `update_check`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "status", rename_all = "camelCase")]
#[ts(export)]
pub enum UpdateCheckOutcome {
    UpToDate,
    Available { update: UpdateInfo },
}

/// Progress of `update_install` over its `Channel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "phase", rename_all = "camelCase")]
#[ts(export)]
pub enum UpdateProgress {
    /// Bytes received so far; `total` from Content-Length when the server sent it.
    #[serde(rename_all = "camelCase")]
    Downloading { downloaded: u64, total: Option<u64> },
    /// Download complete and signature verified; the quit flow starts next.
    Ready,
}

/// `{}` payload of `app:quit-requested` and `drop:leave`.
#[derive(Debug, Clone, Copy, Default, Serialize, TS)]
#[ts(export)]
pub struct EmptyPayload {}

/// `drop:hover` payload (logical px).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DropHoverPayload {
    pub position_x: f64,
    pub position_y: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DropEntry {
    pub name: String,
    pub is_dir: bool,
}

/// `drop:files` payload; local paths stay in Rust.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DropFilesPayload {
    pub drop_id: String,
    pub entries: Vec<DropEntry>,
    pub position_x: f64,
    pub position_y: f64,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::error::{AppError, ErrorCode};
    use pretty_assertions::assert_eq;
    use serde_json::json;

    const SECRET: &str = "S3cr3t-Value-42";

    /// §9.2: `{:?}` of every secret-bearing inbound DTO never contains the value (exhaustive list).
    #[test]
    fn inbound_debug_never_contains_secrets() {
        let draft: OpenSessionRequest = serde_json::from_value(json!({
            "target": { "kind": "draft", "password": SECRET, "bookmark": {
                "id": null, "name": "n", "host": "h", "port": 22, "user": "u", "auth": { "method": "password" },
                "proxyJump": null, "groupId": null, "tags": [], "startDir": null, "useSshConfig": true } },
            "probeOnly": false
        }))
        .unwrap();
        let answers: Vec<PromptAnswer> = serde_json::from_value(json!([
            { "kind": "passphrase", "passphrase": SECRET, "addToAgent": true },
            { "kind": "password", "password": SECRET, "remember": false },
            { "kind": "keyboardInteractive", "responses": [SECRET, SECRET] },
        ]))
        .unwrap();
        let bookmark_pw: Option<Secret<String>> = serde_json::from_value(json!(SECRET)).unwrap();
        // The bookmark startup command is never logged either.
        let bookmark_input: BookmarkInput = serde_json::from_value(json!({
            "id": null, "name": "n", "host": "h", "port": 22, "user": "u", "auth": { "method": "auto" },
            "proxyJump": null, "groupId": null, "tags": [], "startDir": null, "useSshConfig": false,
            "startupCommand": SECRET }))
        .unwrap();

        for dbg in [
            format!("{draft:?}"),
            format!("{answers:?}"),
            format!("{bookmark_pw:?}"),
            format!("{bookmark_input:?}"),
        ] {
            assert!(!dbg.contains(SECRET), "secret leaked: {dbg}");
            assert!(dbg.contains("***"), "{dbg}");
        }
        match &answers[0] {
            PromptAnswer::Passphrase {
                passphrase,
                add_to_agent,
            } => {
                assert_eq!(passphrase.expose(), SECRET);
                assert!(add_to_agent);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn prompt_answer_unit_variants() {
        let a: Vec<PromptAnswer> = serde_json::from_value(json!([
            { "kind": "useSavedPassword" }, { "kind": "skip" }, { "kind": "cancel" },
            { "kind": "hostKey", "accept": true, "remember": false }
        ]))
        .unwrap();
        assert!(matches!(a[0], PromptAnswer::UseSavedPassword));
        assert!(matches!(a[1], PromptAnswer::Skip));
        assert!(matches!(a[2], PromptAnswer::Cancel));
        assert!(matches!(
            a[3],
            PromptAnswer::HostKey {
                accept: true,
                remember: false
            }
        ));
    }

    #[test]
    fn target_kinds() {
        let t: Vec<Target> = serde_json::from_value(json!([
            { "kind": "sshConfig", "alias": "a" }, { "kind": "bookmark", "id": "b" }, { "kind": "quick", "spec": "u@h:2" }
        ]))
        .unwrap();
        assert!(matches!(&t[0], Target::SshConfig { alias } if alias == "a"));
        assert!(matches!(&t[1], Target::Bookmark { id } if id == "b"));
        assert!(matches!(&t[2], Target::Quick { spec } if spec == "u@h:2"));
    }

    fn hop() -> HopRef {
        HopRef {
            index: 0,
            label: "bastion".into(),
            user: "u".into(),
            host: "h".into(),
            port: 22,
        }
    }

    #[test]
    fn session_event_shapes() {
        let ev = SessionEvent::State {
            state: SessionState::Reconnecting {
                stage: ReconnectStage::Waiting,
                hop: None,
                attempt: 2,
                max_attempts: 5,
                next_attempt_at_ms: Some(1_792_500_000_000),
                cause: AppError::new(ErrorCode::Cancelled),
            },
        };
        assert_eq!(
            serde_json::to_value(&ev).unwrap(),
            json!({ "type": "state", "state": { "phase": "reconnecting", "stage": "waiting", "hop": null,
                "attempt": 2, "maxAttempts": 5, "nextAttemptAtMs": 1_792_500_000_000_i64, "cause": { "code": "cancelled" } } })
        );
        let ev = SessionEvent::Stats {
            tx_bytes_per_sec: 1,
            rx_bytes_per_sec: 2,
        };
        assert_eq!(
            serde_json::to_value(&ev).unwrap(),
            json!({ "type": "stats", "txBytesPerSec": 1, "rxBytesPerSec": 2 })
        );
        let ev = SessionEvent::State {
            state: SessionState::Closed,
        };
        assert_eq!(
            serde_json::to_value(&ev).unwrap(),
            json!({ "type": "state", "state": { "phase": "closed" } })
        );
    }

    #[test]
    fn hop_info_flattens_hop_ref() {
        let h = HopInfo {
            hop: hop(),
            host_key: HostKeyInfo {
                key_type: "ssh-ed25519".into(),
                fingerprint: "SHA256:x".into(),
                verification: HostKeyVerification::AutoAccepted,
            },
        };
        assert_eq!(
            serde_json::to_value(&h).unwrap(),
            json!({ "index": 0, "label": "bastion", "user": "u", "host": "h", "port": 22,
                "hostKey": { "keyType": "ssh-ed25519", "fingerprint": "SHA256:x", "verification": "autoAccepted" } })
        );
    }

    #[test]
    fn prompt_and_config_warning_shapes() {
        let p = Prompt::KeyboardInteractive {
            id: "p1".into(),
            hop: hop(),
            name: String::new(),
            instructions: String::new(),
            fields: vec![KbdInteractiveField {
                label: "Code:".into(),
                echo: true,
            }],
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["kind"], "keyboardInteractive");
        assert_eq!(v["fields"], json!([{ "label": "Code:", "echo": true }]));

        let w = ConfigWarning {
            location: SourceLocation {
                file: "~/.ssh/config".into(),
                line: 3,
            },
            kind: ConfigWarningCode::IncludeTooManyFiles {
                pattern: "conf.d/*".into(),
            },
        };
        assert_eq!(
            serde_json::to_value(&w).unwrap(),
            json!({ "location": { "file": "~/.ssh/config", "line": 3 }, "code": "includeTooManyFiles",
                "params": { "pattern": "conf.d/*" } })
        );
    }

    #[test]
    fn auth_and_optional_fields() {
        assert_eq!(
            serde_json::to_value(AuthConfig::Auto).unwrap(),
            json!({ "method": "auto" })
        );
        assert_eq!(
            serde_json::to_value(AuthConfig::Password {
                password_saved: true
            })
            .unwrap(),
            json!({ "method": "password", "passwordSaved": true })
        );
        let k = AgentKey {
            key_type: "ssh-rsa".into(),
            fingerprint: "f".into(),
            comment: String::new(),
            bits: None,
        };
        assert_eq!(
            serde_json::to_value(&k).unwrap(),
            json!({ "keyType": "ssh-rsa", "fingerprint": "f", "comment": "" })
        );
        let s = AgentStatus {
            available: false,
            kind: None,
            endpoint: None,
            identities: vec![],
            warning: None,
        };
        assert_eq!(
            serde_json::to_value(&s).unwrap(),
            json!({ "available": false, "kind": null, "endpoint": null, "identities": [] })
        );
        assert_eq!(serde_json::to_value(EmptyPayload {}).unwrap(), json!({}));
    }

    #[test]
    fn settings_max_file_size_key() {
        let v = serde_json::to_value(crate::storage::settings::default_settings()).unwrap();
        assert!(v["editor"].get("maxFileSizeMiB").is_some(), "{v}");
    }
}
