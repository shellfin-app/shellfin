//! Settings defaults and ranges (§4.4, §7.2): values outside the ranges are **rejected** on
//! `settings_update` (`invalidArgument{field}`) and **clamped** on load.
//! Persistence: `settings.json` (atomic write, migrations, quarantine). On load the stored JSON
//! is deep-merged over the defaults, so missing fields get default values and unknown fields
//! are ignored. A value of the wrong type (or an unknown enum variant) makes the file corrupt:
//! the original is quarantined and only the offending fields fall back to their defaults, the
//! other stored values are kept and written back ([`repair`]).

use std::path::Path;

use serde_json::Value;

use crate::error::{AppError, ErrorCode, IoOp};
use crate::ipc::dto::{
    AdvancedSettings, AppearanceSettings, ConflictPolicy, ConnectionSettings, CursorStyle,
    DEFAULT_WORD_SEPARATORS, DateFormat, DoubleClickAction, EditorSettings, FilesSettings,
    GeneralSettings, NewFileEol, NewTerminalCwd, ReconnectSettings, RightClickAction, Settings,
    ShowWhitespace, SizeUnits, TermType, TerminalBell, TerminalExitAction, TerminalSettings,
    TransferSettings, UiSettings,
};
use crate::storage::file::{corrupt_warning, io_error, quarantine_copy, write_json};
use crate::storage::migrations::StoreFile;
use crate::storage::{LoadedDoc, load_document};

pub const SETTINGS_VERSION: u32 = 1;

/// Inclusive `(min, max)` ranges, keyed by the DTO field path used in `invalidArgument{field}`.
pub mod ranges {
    pub const TERMINAL_FONT_SIZE: (u32, u32) = (8, 32);
    pub const TERMINAL_SCROLLBACK: (u32, u32) = (10, 1_000_000);
    pub const EDITOR_FONT_SIZE: (u32, u32) = (8, 32);
    pub const EDITOR_TAB_SIZE: (u32, u32) = (1, 8);
    pub const EDITOR_MAX_FILE_SIZE_MIB: (u32, u32) = (1, 256);
    pub const KEEPALIVE_INTERVAL_SECS: (u32, u32) = (5, 300);
    pub const KEEPALIVE_COUNT_MAX: (u32, u32) = (1, 10);
    pub const CONNECT_TIMEOUT_SECS: (u32, u32) = (3, 120);
    pub const PROMPT_TIMEOUT_SECS: (u32, u32) = (30, 110);
    pub const RECONNECT_MAX_ATTEMPTS: (u32, u32) = (0, 10);
    /// KiB/s; 0 = unlimited.
    pub const TRANSFER_LIMIT_KIBPS: (u32, u32) = (0, 1_048_576);
    /// UI layout sizes (logical px); not range-checked by the doc beyond being finite and sane.
    pub const UI_SIZE_PX: (f64, f64) = (0.0, 10_000.0);
    pub const TERMINAL_LINE_HEIGHT: (f64, f64) = (1.0, 2.0);
    pub const TRANSFER_MAX_CONCURRENT: (u32, u32) = (1, 64);
    pub const UI_FONT_SIZE: (u32, u32) = (12, 16);
    pub const TERMINAL_LETTER_SPACING: (u32, u32) = (0, 5);
    pub const TERMINAL_MIN_CONTRAST: (f64, f64) = (1.0, 21.0);
    pub const TERMINAL_SCROLL_SENSITIVITY: (u32, u32) = (1, 10);
    /// `files.autoRefresh` seconds (0 = off).
    pub const FILES_AUTO_REFRESH: [u32; 5] = [0, 5, 10, 30, 60];
    /// Longest `terminal.wordSeparators`.
    pub const WORD_SEPARATORS_MAX: usize = 64;
}

pub fn default_settings() -> Settings {
    Settings {
        version: SETTINGS_VERSION,
        locale: "system".to_owned(),
        terminal: TerminalSettings {
            font_size: 13,
            scrollback: 10_000,
            cursor_blink: true,
            copy_on_select: false,
            font_family: None,
            cursor_style: CursorStyle::Block,
            line_height: 1.2,
            bell: TerminalBell::Off,
            right_click: RightClickAction::Menu,
            letter_spacing: 0,
            mouse_reporting: true,
            hide_mouse_pointer_while_typing: false,
            minimum_contrast_ratio: 1.0,
            word_separators: DEFAULT_WORD_SEPARATORS.to_owned(),
            multiline_paste_warning: true,
            on_exit: TerminalExitAction::Keep,
            new_terminal_cwd: NewTerminalCwd::Home,
            open_terminal_on_connect: true,
            term_type: TermType::Xterm256Color,
            alt_is_meta: false,
            scroll_sensitivity: 1,
            background: None,
            screen_reader_mode: false,
        },
        editor: EditorSettings {
            font_size: 13,
            tab_size: 4,
            word_wrap: false,
            max_file_size_mib: 5,
            font_family: None,
            line_numbers: true,
            highlight_active_line: true,
            indent_with_tabs: false,
            trim_trailing_whitespace: false,
            insert_final_newline: false,
            detect_indentation: true,
            new_file_eol: NewFileEol::Lf,
            auto_close_brackets: false,
            bracket_matching: true,
            show_whitespace: ShowWhitespace::None,
            code_folding: true,
            font_ligatures: false,
        },
        connection: ConnectionSettings {
            keepalive_interval_secs: 15,
            keepalive_count_max: 3,
            connect_timeout_secs: 15,
            prompt_timeout_secs: 110,
            reconnect: ReconnectSettings {
                enabled: true,
                max_attempts: 5,
            },
            default_user: String::new(),
            read_ssh_config: true,
            remember_passwords: true,
        },
        files: FilesSettings {
            show_hidden: false,
            folders_first: true,
            confirm_delete: true,
            double_click: DoubleClickAction::Open,
            size_units: SizeUnits::Binary,
            date_format: DateFormat::Relative,
            show_permissions: false,
            auto_refresh: 0,
            columns: true,
        },
        ui: UiSettings {
            sidebar_width: 280.0,
            terminal_panel_height: 260.0,
            terminal_panel_open: true,
        },
        transfers: TransferSettings {
            upload_limit_kibps: 0,
            download_limit_kibps: 0,
            max_concurrent: 3,
            on_conflict: ConflictPolicy::Ask,
            preserve_timestamps: true,
            notify_on_complete: false,
        },
        general: GeneralSettings::default(),
        advanced: AdvancedSettings::default(),
        appearance: AppearanceSettings::default(),
    }
}

impl Default for Settings {
    fn default() -> Self {
        default_settings()
    }
}

fn check(value: u32, (min, max): (u32, u32), field: &str) -> Result<(), AppError> {
    if (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(AppError::invalid_argument(field))
    }
}

fn check_f64(value: f64, (min, max): (f64, f64), field: &str) -> Result<(), AppError> {
    if value.is_finite() && (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(AppError::invalid_argument(field))
    }
}

fn valid_locale(locale: &str) -> bool {
    // "system" or a BCP-47-looking tag: ASCII alphanumerics and '-', 2..=35 chars.
    locale == "system"
        || ((2..=35).contains(&locale.len())
            && locale
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

/// Maximum length of `terminal.fontFamily`.
pub const FONT_FAMILY_MAX_LEN: usize = 200;

/// A CSS font-family list of names only: ASCII letters, digits, space, `-`, `_`, quotes, commas.
fn valid_font_family(f: &str) -> bool {
    f.len() <= FONT_FAMILY_MAX_LEN
        && f.bytes().any(|b| b.is_ascii_alphanumeric())
        && f.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b' ' | b'-' | b'_' | b'\'' | b'"' | b',')
        })
}

/// `terminal.background` is exactly `#RRGGBB` (hex digits in either case).
pub fn valid_hex_color(c: &str) -> bool {
    c.len() == 7 && c.starts_with('#') && c.bytes().skip(1).all(|b| b.is_ascii_hexdigit())
}

/// `terminal.wordSeparators`: ≤ 64 printable ASCII characters (0x20–0x7E).
fn valid_word_separators(s: &str) -> bool {
    s.len() <= ranges::WORD_SEPARATORS_MAX && s.bytes().all(|b| (0x20..=0x7e).contains(&b))
}

/// `connection.defaultUser`: empty or a valid bookmark user.
fn valid_default_user(u: &str) -> bool {
    u.is_empty() || crate::storage::bookmarks::valid_user(u)
}

/// Validation of the newer settings fields (first offending field).
fn validate_v3(s: &Settings) -> Result<(), AppError> {
    use ranges::*;
    check(
        s.appearance.ui_font_size,
        UI_FONT_SIZE,
        "appearance.uiFontSize",
    )?;
    check(
        s.terminal.letter_spacing,
        TERMINAL_LETTER_SPACING,
        "terminal.letterSpacing",
    )?;
    check_f64(
        s.terminal.minimum_contrast_ratio,
        TERMINAL_MIN_CONTRAST,
        "terminal.minimumContrastRatio",
    )?;
    if !valid_word_separators(&s.terminal.word_separators) {
        return Err(AppError::invalid_argument("terminal.wordSeparators"));
    }
    check(
        s.terminal.scroll_sensitivity,
        TERMINAL_SCROLL_SENSITIVITY,
        "terminal.scrollSensitivity",
    )?;
    if s.terminal
        .background
        .as_deref()
        .is_some_and(|c| !valid_hex_color(c))
    {
        return Err(AppError::invalid_argument("terminal.background"));
    }
    if !FILES_AUTO_REFRESH.contains(&s.files.auto_refresh) {
        return Err(AppError::invalid_argument("files.autoRefresh"));
    }
    if !valid_default_user(&s.connection.default_user) {
        return Err(AppError::invalid_argument("connection.defaultUser"));
    }
    Ok(())
}

/// Validation for `settings_update`: rejects out-of-range values with the first offending field.
pub fn validate(s: &Settings) -> Result<(), AppError> {
    use ranges::*;
    if s.version != SETTINGS_VERSION {
        return Err(AppError::invalid_argument("version"));
    }
    validate_v3(s)?;
    if !valid_locale(&s.locale) {
        return Err(AppError::invalid_argument("locale"));
    }
    check(
        s.terminal.font_size,
        TERMINAL_FONT_SIZE,
        "terminal.fontSize",
    )?;
    check(
        s.terminal.scrollback,
        TERMINAL_SCROLLBACK,
        "terminal.scrollback",
    )?;
    if let Some(f) = &s.terminal.font_family
        && !valid_font_family(f)
    {
        return Err(AppError::invalid_argument("terminal.fontFamily"));
    }
    check_f64(
        s.terminal.line_height,
        TERMINAL_LINE_HEIGHT,
        "terminal.lineHeight",
    )?;
    if let Some(f) = &s.editor.font_family
        && !valid_font_family(f)
    {
        return Err(AppError::invalid_argument("editor.fontFamily"));
    }
    check(
        s.transfers.max_concurrent,
        TRANSFER_MAX_CONCURRENT,
        "transfers.maxConcurrent",
    )?;
    check(s.editor.font_size, EDITOR_FONT_SIZE, "editor.fontSize")?;
    check(s.editor.tab_size, EDITOR_TAB_SIZE, "editor.tabSize")?;
    check(
        s.editor.max_file_size_mib,
        EDITOR_MAX_FILE_SIZE_MIB,
        "editor.maxFileSizeMiB",
    )?;
    check(
        s.connection.keepalive_interval_secs,
        KEEPALIVE_INTERVAL_SECS,
        "connection.keepaliveIntervalSecs",
    )?;
    check(
        s.connection.keepalive_count_max,
        KEEPALIVE_COUNT_MAX,
        "connection.keepaliveCountMax",
    )?;
    check(
        s.connection.connect_timeout_secs,
        CONNECT_TIMEOUT_SECS,
        "connection.connectTimeoutSecs",
    )?;
    check(
        s.connection.prompt_timeout_secs,
        PROMPT_TIMEOUT_SECS,
        "connection.promptTimeoutSecs",
    )?;
    check(
        s.connection.reconnect.max_attempts,
        RECONNECT_MAX_ATTEMPTS,
        "connection.reconnect.maxAttempts",
    )?;
    check(
        s.transfers.upload_limit_kibps,
        TRANSFER_LIMIT_KIBPS,
        "transfers.uploadLimitKiBps",
    )?;
    check(
        s.transfers.download_limit_kibps,
        TRANSFER_LIMIT_KIBPS,
        "transfers.downloadLimitKiBps",
    )?;
    check_f64(s.ui.sidebar_width, UI_SIZE_PX, "ui.sidebarWidth")?;
    check_f64(
        s.ui.terminal_panel_height,
        UI_SIZE_PX,
        "ui.terminalPanelHeight",
    )?;
    Ok(())
}

fn clamp_u32(value: &mut u32, (min, max): (u32, u32)) {
    *value = (*value).clamp(min, max);
}

fn clamp_f64(value: &mut f64, (min, max): (f64, f64), default: f64) {
    *value = if value.is_finite() {
        value.clamp(min, max)
    } else {
        default
    };
}

/// Clamp on load (never rejects). Unknown locale → `"system"`.
pub fn clamp(mut s: Settings) -> Settings {
    use ranges::*;
    let d = default_settings();
    s.version = SETTINGS_VERSION;
    if !valid_locale(&s.locale) {
        s.locale = d.locale;
    }
    if s.terminal
        .font_family
        .as_deref()
        .is_some_and(|f| !valid_font_family(f))
    {
        s.terminal.font_family = None;
    }
    if s.editor
        .font_family
        .as_deref()
        .is_some_and(|f| !valid_font_family(f))
    {
        s.editor.font_family = None;
    }
    clamp_f64(
        &mut s.terminal.line_height,
        TERMINAL_LINE_HEIGHT,
        d.terminal.line_height,
    );
    clamp_u32(&mut s.transfers.max_concurrent, TRANSFER_MAX_CONCURRENT);
    clamp_u32(&mut s.terminal.font_size, TERMINAL_FONT_SIZE);
    clamp_u32(&mut s.terminal.scrollback, TERMINAL_SCROLLBACK);
    clamp_u32(&mut s.editor.font_size, EDITOR_FONT_SIZE);
    clamp_u32(&mut s.editor.tab_size, EDITOR_TAB_SIZE);
    clamp_u32(&mut s.editor.max_file_size_mib, EDITOR_MAX_FILE_SIZE_MIB);
    clamp_u32(
        &mut s.connection.keepalive_interval_secs,
        KEEPALIVE_INTERVAL_SECS,
    );
    clamp_u32(&mut s.connection.keepalive_count_max, KEEPALIVE_COUNT_MAX);
    clamp_u32(&mut s.connection.connect_timeout_secs, CONNECT_TIMEOUT_SECS);
    clamp_u32(&mut s.connection.prompt_timeout_secs, PROMPT_TIMEOUT_SECS);
    clamp_u32(
        &mut s.connection.reconnect.max_attempts,
        RECONNECT_MAX_ATTEMPTS,
    );
    clamp_u32(&mut s.transfers.upload_limit_kibps, TRANSFER_LIMIT_KIBPS);
    clamp_u32(&mut s.transfers.download_limit_kibps, TRANSFER_LIMIT_KIBPS);
    clamp_f64(&mut s.ui.sidebar_width, UI_SIZE_PX, d.ui.sidebar_width);
    clamp_f64(
        &mut s.ui.terminal_panel_height,
        UI_SIZE_PX,
        d.ui.terminal_panel_height,
    );
    clamp_u32(&mut s.appearance.ui_font_size, UI_FONT_SIZE);
    clamp_u32(&mut s.terminal.letter_spacing, TERMINAL_LETTER_SPACING);
    clamp_f64(
        &mut s.terminal.minimum_contrast_ratio,
        TERMINAL_MIN_CONTRAST,
        d.terminal.minimum_contrast_ratio,
    );
    if !valid_word_separators(&s.terminal.word_separators) {
        s.terminal.word_separators = d.terminal.word_separators;
    }
    clamp_u32(
        &mut s.terminal.scroll_sensitivity,
        TERMINAL_SCROLL_SENSITIVITY,
    );
    if s.terminal
        .background
        .as_deref()
        .is_some_and(|c| !valid_hex_color(c))
    {
        s.terminal.background = None;
    }
    if !FILES_AUTO_REFRESH.contains(&s.files.auto_refresh) {
        s.files.auto_refresh = 0;
    }
    if !valid_default_user(&s.connection.default_user) {
        s.connection.default_user = String::new();
    }
    s
}

/// Settings as loaded from disk.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedSettings {
    pub settings: Settings,
    /// `settings.json` was written by a newer version: never write it (`storageReadOnly`).
    pub read_only: bool,
}

/// Recursively overlay `over` onto `base` (objects merge key by key; anything else replaces).
fn merge(base: &mut Value, over: Value) {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            for (k, v) in o {
                match b.get_mut(&k) {
                    Some(slot) => merge(slot, v),
                    None => {
                        b.insert(k, v);
                    }
                }
            }
        }
        (slot, v) => *slot = v,
    }
}

/// Load `<dir>/settings.json` (missing → defaults), clamped to the §4.4 ranges.
pub fn load_full(dir: &Path) -> (LoadedSettings, Vec<AppError>) {
    let path = dir.join(StoreFile::Settings.file_name());
    let LoadedDoc {
        value,
        read_only,
        mut warnings,
    } = load_document(&path, StoreFile::Settings);
    let mut loaded = LoadedSettings {
        settings: default_settings(),
        read_only,
    };
    let Some(value) = value else {
        return (loaded, warnings);
    };
    let mut merged = serde_json::to_value(default_settings()).unwrap_or(Value::Null);
    merge(&mut merged, value.clone());
    match serde_json::from_value::<Settings>(merged) {
        // `clamp` also normalises `version` to the version this build understands.
        Ok(s) => loaded.settings = clamp(s),
        Err(e) => {
            // Keep every field that is still valid; only the bad ones fall back.
            let (repaired, reset) = repair(&value);
            loaded.settings = clamp(repaired);
            if !read_only {
                let reason = format!("{e}; fields reset to defaults: {}", reset_list(&reset));
                match quarantine_copy(
                    &path,
                    &serde_json::to_vec_pretty(&value).unwrap_or_default(),
                ) {
                    Ok(q) => warnings.push(corrupt_warning(&path, &q, reason)),
                    Err(io) => warnings.push(io_error(IoOp::WriteStorage, &path, &io)),
                }
                if let Err(e) = save(dir, &loaded.settings) {
                    warnings.push(e);
                }
            }
        }
    }
    (loaded, warnings)
}

/// One stored value to try on top of the defaults: the path of its parent object and its key.
struct Field {
    parent: Vec<String>,
    key: String,
    value: Value,
}

/// The stored leaves whose key the defaults know (unknown keys are ignored by serde anyway, and
/// skipping them bounds the work by the number of settings, whatever the file holds). A value
/// whose default is not an object, or that is not an object itself, is one leaf.
fn known_leaves(default: &Value, stored: &Value, parent: &mut Vec<String>, out: &mut Vec<Field>) {
    let (Value::Object(d), Value::Object(s)) = (default, stored) else {
        return;
    };
    for (key, value) in s {
        let Some(dv) = d.get(key) else { continue };
        if dv.is_object() && value.is_object() {
            parent.push(key.clone());
            known_leaves(dv, value, parent, out);
            parent.pop();
        } else {
            out.push(Field {
                parent: parent.clone(),
                key: key.clone(),
                value: value.clone(),
            });
        }
    }
}

/// Per-field fallback: start from the defaults and apply the stored values one field at a
/// time, keeping each only if the whole document still deserializes. A single bad value (say an
/// enum variant from a newer or older build) thus resets only that field instead of the whole
/// file. Returns the settings and the dotted paths of the fields that were reset. Ranges are not
/// checked here (`clamp` does that).
fn repair(stored: &Value) -> (Settings, Vec<String>) {
    let defaults = default_settings();
    let Ok(mut doc) = serde_json::to_value(&defaults) else {
        return (defaults, Vec::new());
    };
    let mut leaves = Vec::new();
    known_leaves(&doc.clone(), stored, &mut Vec::new(), &mut leaves);
    let mut reset = Vec::new();
    for Field { parent, key, value } in leaves {
        let pointer: String = parent.iter().map(|p| format!("/{p}")).collect();
        let Some(Value::Object(obj)) = doc.pointer_mut(&pointer) else {
            continue;
        };
        let Some(old) = obj.insert(key.clone(), value) else {
            continue;
        };
        if serde_json::from_value::<Settings>(doc.clone()).is_err() {
            if let Some(Value::Object(obj)) = doc.pointer_mut(&pointer) {
                obj.insert(key.clone(), old);
            }
            reset.push(
                parent
                    .iter()
                    .chain([&key])
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("."),
            );
        }
    }
    let settings = serde_json::from_value(doc).unwrap_or(defaults);
    (settings, reset)
}

/// Field paths for the warning detail (at most 20, so a hostile file cannot make it huge).
fn reset_list(reset: &[String]) -> String {
    const MAX: usize = 20;
    let mut s = reset
        .iter()
        .take(MAX)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if reset.len() > MAX {
        s.push_str(&format!(" (+{} more)", reset.len() - MAX));
    }
    if s.is_empty() {
        s.push_str("none");
    }
    s
}

/// Load `<dir>/settings.json`; see [`load_full`].
pub fn load(dir: &Path) -> (Settings, Vec<AppError>) {
    let (loaded, warnings) = load_full(dir);
    (loaded.settings, warnings)
}

/// Persist settings atomically (pretty JSON, top-level `"version"`).
pub fn save(dir: &Path, settings: &Settings) -> Result<(), AppError> {
    let path = dir.join(StoreFile::Settings.file_name());
    write_json(&path, settings).map_err(|e| io_error(IoOp::WriteStorage, &path, &e))
}

/// `settings_update`: validate, refuse when read-only, persist.
pub fn update(dir: &Path, read_only: bool, settings: &Settings) -> Result<(), AppError> {
    validate(settings)?;
    if read_only {
        return Err(AppError::new(ErrorCode::StorageReadOnly));
    }
    save(dir, settings)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::error::ErrorCode;

    #[test]
    fn load_missing_gives_defaults_and_save_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let (s, w) = load(dir.path());
        assert!(w.is_empty());
        assert_eq!(s, default_settings());
        let mut s2 = s.clone();
        s2.terminal.font_size = 20;
        update(dir.path(), false, &s2).unwrap();
        assert_eq!(load(dir.path()).0, s2);
        let raw: Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("settings.json")).unwrap())
                .unwrap();
        assert_eq!(raw["version"], 1);
        assert_eq!(raw["editor"]["maxFileSizeMiB"], 5);
    }

    #[test]
    fn editor_font_ligatures_roundtrips_and_has_no_terminal_counterpart() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = default_settings();
        assert!(!s.editor.font_ligatures);
        s.editor.font_ligatures = true;
        update(dir.path(), false, &s).unwrap();
        assert!(load(dir.path()).0.editor.font_ligatures);
        let raw: Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("settings.json")).unwrap())
                .unwrap();
        assert_eq!(raw["editor"]["fontLigatures"], true);
        assert!(raw["terminal"].get("fontLigatures").is_none());
    }

    #[test]
    fn terminal_screen_reader_mode_defaults_off_and_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = default_settings();
        assert!(!s.terminal.screen_reader_mode);
        s.terminal.screen_reader_mode = true;
        update(dir.path(), false, &s).unwrap();
        assert!(load(dir.path()).0.terminal.screen_reader_mode);
        let raw: Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("settings.json")).unwrap())
                .unwrap();
        assert_eq!(raw["terminal"]["screenReaderMode"], true);
        assert_eq!(raw["version"], 1, "the file version stays 1");
    }

    #[test]
    fn load_merges_over_defaults_and_clamps() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            br#"{"version":1,"terminal":{"fontSize":99},"connection":{"reconnect":{"maxAttempts":50}},"future":true}"#,
        )
        .unwrap();
        let (s, w) = load(dir.path());
        assert!(w.is_empty());
        assert_eq!(s.terminal.font_size, 32);
        assert_eq!(
            s.terminal.scrollback,
            default_settings().terminal.scrollback
        );
        assert_eq!(s.connection.reconnect.max_attempts, 10);
        assert!(s.connection.reconnect.enabled);
    }

    #[test]
    fn wrong_types_quarantine_and_fall_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            br#"{"version":1,"terminal":{"fontSize":"big"}}"#,
        )
        .unwrap();
        let (s, w) = load(dir.path());
        assert_eq!(s, default_settings());
        assert_eq!(
            serde_json::to_value(&w[0]).unwrap()["code"],
            "storageCorrupt"
        );
        assert!(load(dir.path()).1.is_empty());
    }

    /// One bad value resets only that field; the rest of the file survives, the original
    /// is quarantined and the repaired settings are written back.
    #[test]
    fn a_bad_field_falls_back_alone() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            br#"{"version":1,
                "terminal":{"fontSize":20,"cursorStyle":"hologram","scrollback":5000,"screenReaderMode":true},
                "editor":{"tabSize":"wide","wordWrap":true},
                "appearance":{"theme":"light","accent":42},
                "files":"oops",
                "general":{"autoUpdate":false,"future":{"x":1}},
                "unknownTop":[1,2,3]}"#,
        )
        .unwrap();
        let (s, w) = load(dir.path());
        let d = default_settings();
        assert_eq!(s.terminal.font_size, 20);
        assert_eq!(s.terminal.scrollback, 5000);
        assert!(s.terminal.screen_reader_mode);
        assert_eq!(s.terminal.cursor_style, d.terminal.cursor_style, "reset");
        assert_eq!(s.editor.tab_size, d.editor.tab_size, "reset");
        assert!(s.editor.word_wrap);
        assert_eq!(
            serde_json::to_value(s.appearance.theme).unwrap(),
            "light",
            "kept"
        );
        assert_eq!(s.appearance.accent, d.appearance.accent, "reset");
        assert_eq!(s.files, d.files, "a non-object section falls back whole");
        assert!(!s.general.auto_update);
        assert_eq!(w.len(), 1);
        let warning = serde_json::to_value(&w[0]).unwrap();
        assert_eq!(warning["code"], "storageCorrupt");
        let detail = w[0].detail.clone().unwrap();
        for f in [
            "terminal.cursorStyle",
            "editor.tabSize",
            "appearance.accent",
            "files",
        ] {
            assert!(detail.contains(f), "{f} in {detail}");
        }
        assert!(!detail.contains("fontSize"), "{detail}");
        // Written back repaired: the next load is clean and identical.
        let (again, w) = load(dir.path());
        assert!(w.is_empty());
        assert_eq!(again, s);
        // The original stays available as the quarantine copy.
        let copies = std::fs::read_dir(dir.path())
            .unwrap()
            .filter(|e| {
                let name = e.as_ref().unwrap().file_name();
                let name = name.to_string_lossy();
                name.starts_with("settings") && name != "settings.json"
            })
            .count();
        assert_eq!(copies, 1);
    }

    /// Every field of a fully non-default (valid) file survives when another field is bad, so no
    /// field is lost by the per-field repair (e.g. through serialization quirks).
    #[test]
    fn repair_keeps_every_valid_non_default_field() {
        let mut custom = default_settings();
        custom.locale = "uk".into();
        custom.terminal.font_size = 17;
        custom.terminal.font_family = Some("Fira Code".into());
        custom.terminal.background = Some("#102030".into());
        custom.terminal.screen_reader_mode = true;
        custom.terminal.word_separators = " ()".into();
        custom.editor.font_ligatures = true;
        custom.editor.max_file_size_mib = 9;
        custom.connection.reconnect.max_attempts = 2;
        custom.connection.default_user = "deploy".into();
        custom.files.auto_refresh = 30;
        custom.ui.sidebar_width = 333.0;
        custom.transfers.max_concurrent = 7;
        custom.general.auto_update = false;
        custom.appearance.ui_font_size = 15;
        let mut raw = serde_json::to_value(&custom).unwrap();
        raw["editor"]["newFileEol"] = Value::from("vt");
        let (repaired, reset) = repair(&raw);
        assert_eq!(reset, vec!["editor.newFileEol".to_owned()]);
        let mut expected = custom.clone();
        expected.editor.new_file_eol = default_settings().editor.new_file_eol;
        assert_eq!(repaired, expected);
        // Nothing to repair in a valid document.
        let (same, reset) = repair(&serde_json::to_value(&custom).unwrap());
        assert!(reset.is_empty());
        assert_eq!(same, custom);
    }

    /// Hostile input: a huge number of unknown keys or deep nesting does not make the repair
    /// slow (only known fields are tried), and a non-object document gives the defaults.
    #[test]
    fn repair_is_bounded_on_hostile_documents() {
        let mut terminal = serde_json::Map::new();
        for i in 0..100_000 {
            terminal.insert(format!("k{i}"), Value::from(i));
        }
        terminal.insert("cursorStyle".into(), Value::from(7));
        let raw = serde_json::json!({ "version": 1, "terminal": terminal });
        let started = std::time::Instant::now();
        let (s, reset) = repair(&raw);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert_eq!(reset, vec!["terminal.cursorStyle".to_owned()]);
        assert_eq!(s, default_settings());
        for raw in [Value::Null, Value::from(5), serde_json::json!([1, 2])] {
            let (s, reset) = repair(&raw);
            assert_eq!(s, default_settings());
            assert!(reset.is_empty());
        }
        let many: Vec<String> = (0..25).map(|i| format!("f{i}")).collect();
        assert!(reset_list(&many).ends_with("(+5 more)"));
        assert_eq!(reset_list(&[]), "none");
    }

    #[test]
    fn unparsable_and_versionless_files_are_corrupt() {
        for content in [&b"{"[..], br#"{"terminal":{}}"#] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("settings.json"), content).unwrap();
            let (s, w) = load(dir.path());
            assert_eq!(s, default_settings());
            assert_eq!(
                serde_json::to_value(&w[0]).unwrap()["code"],
                "storageCorrupt"
            );
        }
    }

    #[test]
    fn newer_version_is_read_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            br#"{"version":3,"terminal":{"fontSize":20}}"#,
        )
        .unwrap();
        let (l, w) = load_full(dir.path());
        assert!(l.read_only);
        assert_eq!(l.settings.terminal.font_size, 20);
        assert_eq!(
            serde_json::to_value(&w[0]).unwrap()["code"],
            "storageReadOnly"
        );
        assert_eq!(
            update(dir.path(), true, &l.settings).unwrap_err().kind,
            ErrorCode::StorageReadOnly
        );
        // Invalid values are still rejected first.
        let mut bad = l.settings.clone();
        bad.editor.tab_size = 0;
        assert!(matches!(
            update(dir.path(), true, &bad).unwrap_err().kind,
            ErrorCode::InvalidArgument { .. }
        ));
    }

    /// The `transfers` section is optional on disk, validated on update, clamped on load.
    #[test]
    fn transfer_limits_default_validate_and_clamp() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            br#"{"version":1,"transfers":{"uploadLimitKiBps":5000000}}"#,
        )
        .unwrap();
        let (s, w) = load(dir.path());
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(s.transfers.upload_limit_kibps, 1_048_576);
        assert_eq!(s.transfers.download_limit_kibps, 0);

        let (s, _) = load(tempfile::tempdir().unwrap().path());
        assert_eq!(s.transfers.upload_limit_kibps, 0);

        for (field, set) in [
            (
                "transfers.uploadLimitKiBps",
                (|s: &mut Settings| s.transfers.upload_limit_kibps = 1_048_577)
                    as fn(&mut Settings),
            ),
            ("transfers.downloadLimitKiBps", |s: &mut Settings| {
                s.transfers.download_limit_kibps = u32::MAX;
            }),
        ] {
            let mut bad = default_settings();
            set(&mut bad);
            let e = validate(&bad).unwrap_err();
            assert_eq!(serde_json::to_value(&e).unwrap()["params"]["field"], field);
        }
        let mut ok = default_settings();
        ok.transfers.upload_limit_kibps = 1_048_576;
        ok.transfers.download_limit_kibps = 1;
        validate(&ok).unwrap();
    }

    #[test]
    fn font_family_validation_default_and_migration() {
        // Old file without the field -> null.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            br#"{"version":1,"terminal":{"fontSize":14}}"#,
        )
        .unwrap();
        let (s, w) = load(dir.path());
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(s.terminal.font_family, None);
        // Invalid stored value -> null on load.
        std::fs::write(
            dir.path().join("settings.json"),
            br#"{"version":1,"terminal":{"fontFamily":"a;b"}}"#,
        )
        .unwrap();
        assert_eq!(load(dir.path()).0.terminal.font_family, None);

        let set = |f: &str| {
            let mut s = default_settings();
            s.terminal.font_family = Some(f.to_owned());
            validate(&s)
        };
        set("\"Cascadia Code\", 'Fira-Code', monospace_1").unwrap();
        let long = "a".repeat(201);
        for bad in [
            "",
            " , ",
            "url(x)",
            "a;b",
            "a{b}",
            "a\\b",
            "@import",
            "a\nb",
            "a\u{0}b",
            "Шрифт",
            long.as_str(),
        ] {
            assert_eq!(
                set(bad).unwrap_err().kind,
                ErrorCode::InvalidArgument {
                    field: "terminal.fontFamily".into()
                },
                "{bad:?}"
            );
        }
        set(&"a".repeat(200)).unwrap();
    }

    /// An old file without any new field loads with the documented defaults.
    #[test]
    fn v2_fields_default_when_missing_from_an_old_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            br#"{"version":1,"locale":"en","terminal":{"fontSize":14,"scrollback":5000,"cursorBlink":false,"copyOnSelect":true,"fontFamily":null},"editor":{"fontSize":12,"tabSize":2,"wordWrap":true,"maxFileSizeMiB":7},"files":{"showHidden":true},"transfers":{"uploadLimitKiBps":5,"downloadLimitKiBps":6}}"#,
        )
        .unwrap();
        let (s, w) = load(dir.path());
        assert!(w.is_empty(), "{w:?}");
        let d = default_settings();
        assert!(s.general.confirm_close_with_terminals);
        assert_eq!(s.terminal.cursor_style, CursorStyle::Block);
        assert!((s.terminal.line_height - 1.2).abs() < f64::EPSILON);
        assert_eq!(s.terminal.bell, TerminalBell::Off);
        assert_eq!(s.terminal.right_click, RightClickAction::Menu);
        assert_eq!(s.editor.font_family, None);
        assert!(s.editor.line_numbers && s.editor.highlight_active_line);
        assert!(!s.editor.indent_with_tabs);
        assert!(!s.editor.trim_trailing_whitespace && !s.editor.insert_final_newline);
        assert!(s.files.show_hidden && s.files.folders_first && s.files.confirm_delete);
        assert_eq!(s.transfers.max_concurrent, 3);
        assert_eq!(s.transfers.on_conflict, ConflictPolicy::Ask);
        assert_eq!(s.transfers.upload_limit_kibps, 5);
        assert_eq!(s.advanced.log_level, crate::ipc::dto::LogLevel::Info);
        // What the user had stays.
        assert_eq!(s.terminal.font_size, 14);
        assert_eq!(s.editor.tab_size, 2);
        // The same document with the v2 fields spelled out equals the defaults for them.
        assert_eq!(s.general, d.general);
        assert_eq!(s.advanced, d.advanced);
        // An old frontend's `settings_update` payload (no v2 fields) still deserializes.
        let mut v = serde_json::to_value(default_settings()).unwrap();
        v["terminal"].as_object_mut().unwrap().remove("lineHeight");
        v["transfers"]
            .as_object_mut()
            .unwrap()
            .remove("maxConcurrent");
        v.as_object_mut().unwrap().remove("general");
        v.as_object_mut().unwrap().remove("advanced");
        assert_eq!(
            serde_json::from_value::<Settings>(v).unwrap(),
            default_settings()
        );
    }

    #[test]
    fn v2_ranges_validation_and_clamp() {
        let reject = |set: fn(&mut Settings), field: &str| {
            let mut s = default_settings();
            set(&mut s);
            assert_eq!(
                validate(&s).unwrap_err().kind,
                ErrorCode::InvalidArgument {
                    field: field.into()
                },
                "{field}"
            );
        };
        reject(|s| s.terminal.line_height = 0.99, "terminal.lineHeight");
        reject(|s| s.terminal.line_height = 2.01, "terminal.lineHeight");
        reject(|s| s.terminal.line_height = f64::NAN, "terminal.lineHeight");
        reject(
            |s| s.terminal.line_height = f64::INFINITY,
            "terminal.lineHeight",
        );
        reject(
            |s| s.transfers.max_concurrent = 0,
            "transfers.maxConcurrent",
        );
        reject(
            |s| s.transfers.max_concurrent = 65,
            "transfers.maxConcurrent",
        );
        reject(
            |s| s.editor.font_family = Some("url(x);".into()),
            "editor.fontFamily",
        );
        reject(
            |s| s.editor.font_family = Some(String::new()),
            "editor.fontFamily",
        );
        reject(
            |s| s.editor.font_family = Some("a".repeat(201)),
            "editor.fontFamily",
        );
        let mut ok = default_settings();
        ok.terminal.line_height = 1.0;
        validate(&ok).unwrap();
        ok.terminal.line_height = 2.0;
        ok.transfers.max_concurrent = 64;
        ok.transfers.on_conflict = ConflictPolicy::Skip;
        ok.editor.font_family = Some("\"Cascadia Code\", monospace".into());
        validate(&ok).unwrap();
        ok.transfers.max_concurrent = 1;
        validate(&ok).unwrap();

        // Clamp / reset on load.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            br#"{"version":1,"terminal":{"lineHeight":9},"transfers":{"maxConcurrent":99},"editor":{"fontFamily":"a;b"}}"#,
        )
        .unwrap();
        let (s, w) = load(dir.path());
        assert!(w.is_empty(), "{w:?}");
        assert!((s.terminal.line_height - 2.0).abs() < f64::EPSILON);
        assert_eq!(s.transfers.max_concurrent, 64);
        assert_eq!(s.editor.font_family, None);
        std::fs::write(
            dir.path().join("settings.json"),
            br#"{"version":1,"terminal":{"lineHeight":0.1},"transfers":{"maxConcurrent":0}}"#,
        )
        .unwrap();
        let (s, _) = load(dir.path());
        assert!((s.terminal.line_height - 1.0).abs() < f64::EPSILON);
        assert_eq!(s.transfers.max_concurrent, 1);
    }

    #[test]
    fn v2_enums_roundtrip_and_unknown_values_are_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = default_settings();
        s.terminal.cursor_style = CursorStyle::Underline;
        s.terminal.bell = TerminalBell::Visual;
        s.terminal.right_click = RightClickAction::Paste;
        s.transfers.on_conflict = ConflictPolicy::Overwrite;
        s.advanced.log_level = crate::ipc::dto::LogLevel::Debug;
        s.general.confirm_close_with_terminals = false;
        update(dir.path(), false, &s).unwrap();
        let raw: Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("settings.json")).unwrap())
                .unwrap();
        assert_eq!(raw["terminal"]["cursorStyle"], "underline");
        assert_eq!(raw["terminal"]["bell"], "visual");
        assert_eq!(raw["terminal"]["rightClick"], "paste");
        assert_eq!(raw["transfers"]["onConflict"], "overwrite");
        assert_eq!(raw["advanced"]["logLevel"], "debug");
        assert_eq!(raw["general"]["confirmCloseWithTerminals"], false);
        assert_eq!(raw["version"], 1);
        assert_eq!(load(dir.path()).0, s);

        for bad in [
            br#"{"version":1,"terminal":{"cursorStyle":"beam"}}"#.as_slice(),
            br#"{"version":1,"transfers":{"onConflict":"always"}}"#.as_slice(),
            br#"{"version":1,"advanced":{"logLevel":"trace"}}"#.as_slice(),
            br#"{"version":1,"terminal":{"bell":true}}"#.as_slice(),
        ] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("settings.json"), bad).unwrap();
            let (s, w) = load(dir.path());
            assert_eq!(s, default_settings());
            assert_eq!(
                serde_json::to_value(&w[0]).unwrap()["code"],
                "storageCorrupt"
            );
        }
    }

    /// An old file without the newer fields loads with the documented defaults, and
    /// an older frontend's payload still deserializes.
    #[test]
    fn v3_fields_default_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            br#"{"version":1,"terminal":{"fontSize":14,"scrollback":5000,"cursorBlink":false,"copyOnSelect":true,"fontFamily":null,"bell":"visual"},"editor":{"fontSize":12,"tabSize":2,"wordWrap":true,"maxFileSizeMiB":7},"transfers":{"uploadLimitKiBps":5,"downloadLimitKiBps":6,"maxConcurrent":4},"general":{"confirmCloseWithTerminals":false}}"#,
        )
        .unwrap();
        let (s, w) = load(dir.path());
        assert!(w.is_empty(), "{w:?}");
        let d = default_settings();
        assert_eq!(s.appearance, d.appearance);
        assert_eq!(s.appearance.theme, crate::ipc::dto::ThemeMode::Dark);
        assert_eq!(s.appearance.accent, crate::ipc::dto::AccentColor::Teal);
        assert_eq!(s.appearance.ui_font_size, 13);
        assert_eq!(
            s.general.confirm_quit,
            crate::ipc::dto::ConfirmQuit::WithConnections
        );
        assert_eq!(
            s.general.on_startup,
            crate::ipc::dto::StartupAction::Welcome
        );
        assert!(!s.general.notify_on_disconnect);
        assert!(!s.general.confirm_close_with_terminals, "v2 value kept");
        assert_eq!(s.terminal.bell, TerminalBell::Visual);
        assert_eq!(s.terminal.letter_spacing, 0);
        assert!(s.terminal.mouse_reporting && s.terminal.multiline_paste_warning);
        assert!(!s.terminal.hide_mouse_pointer_while_typing && !s.terminal.alt_is_meta);
        assert!((s.terminal.minimum_contrast_ratio - 1.0).abs() < f64::EPSILON);
        assert_eq!(s.terminal.word_separators, " ()[]{}'\"");
        assert_eq!(s.terminal.on_exit, TerminalExitAction::Keep);
        assert_eq!(s.terminal.new_terminal_cwd, NewTerminalCwd::Home);
        assert!(s.terminal.open_terminal_on_connect);
        assert_eq!(s.terminal.term_type, TermType::Xterm256Color);
        assert_eq!(s.terminal.scroll_sensitivity, 1);
        assert_eq!(s.terminal.background, None);
        assert!(
            !s.terminal.screen_reader_mode,
            "screen reader mode defaults to off"
        );
        assert!(s.editor.detect_indentation && s.editor.bracket_matching && s.editor.code_folding);
        assert!(!s.editor.auto_close_brackets);
        assert_eq!(s.editor.show_whitespace, ShowWhitespace::None);
        assert!(!s.editor.font_ligatures, "ligatures default to off");
        assert_eq!(s.editor.new_file_eol, NewFileEol::Lf);
        assert_eq!(s.files.double_click, DoubleClickAction::Open);
        assert_eq!(s.files.size_units, SizeUnits::Binary);
        assert_eq!(s.files.date_format, DateFormat::Relative);
        assert!(!s.files.show_permissions);
        assert_eq!(s.files.auto_refresh, 0);
        assert!(s.files.columns, "file tree columns default to on");
        assert_eq!(s.connection.default_user, "");
        assert!(s.connection.read_ssh_config && s.connection.remember_passwords);
        assert!(s.transfers.preserve_timestamps && !s.transfers.notify_on_complete);
        assert_eq!(s.transfers.max_concurrent, 4);
        assert_eq!(s.editor.max_file_size_mib, 7);

        // An older frontend payload (no newer fields) still deserializes to the defaults.
        let mut v = serde_json::to_value(default_settings()).unwrap();
        v.as_object_mut().unwrap().remove("appearance");
        for key in [
            "letterSpacing",
            "mouseReporting",
            "termType",
            "background",
            "wordSeparators",
            "minimumContrastRatio",
        ] {
            v["terminal"].as_object_mut().unwrap().remove(key);
        }
        for key in ["defaultUser", "readSshConfig", "rememberPasswords"] {
            v["connection"].as_object_mut().unwrap().remove(key);
        }
        v["transfers"]
            .as_object_mut()
            .unwrap()
            .remove("preserveTimestamps");
        for key in ["autoRefresh", "columns"] {
            v["files"].as_object_mut().unwrap().remove(key);
        }
        assert_eq!(
            serde_json::from_value::<Settings>(v).unwrap(),
            default_settings()
        );
    }

    #[test]
    fn v3_validation_rejects_with_field_paths() {
        let reject = |set: &dyn Fn(&mut Settings), field: &str| {
            let mut s = default_settings();
            set(&mut s);
            assert_eq!(
                validate(&s).unwrap_err().kind,
                ErrorCode::InvalidArgument {
                    field: field.into()
                },
                "{field}"
            );
        };
        reject(&|s| s.terminal.scrollback = 9, "terminal.scrollback");
        reject(
            &|s| s.terminal.scrollback = 1_000_001,
            "terminal.scrollback",
        );
        reject(
            &|s| s.editor.max_file_size_mib = 257,
            "editor.maxFileSizeMiB",
        );
        reject(&|s| s.editor.max_file_size_mib = 0, "editor.maxFileSizeMiB");
        reject(&|s| s.appearance.ui_font_size = 11, "appearance.uiFontSize");
        reject(&|s| s.appearance.ui_font_size = 17, "appearance.uiFontSize");
        reject(&|s| s.terminal.letter_spacing = 6, "terminal.letterSpacing");
        for bad in [0.99, 21.01, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            reject(
                &|s| s.terminal.minimum_contrast_ratio = bad,
                "terminal.minimumContrastRatio",
            );
        }
        reject(
            &|s| s.terminal.scroll_sensitivity = 0,
            "terminal.scrollSensitivity",
        );
        reject(
            &|s| s.terminal.scroll_sensitivity = 11,
            "terminal.scrollSensitivity",
        );
        for bad in [
            "a".repeat(65),
            "a\tb".to_owned(),
            "a\nb".to_owned(),
            "\u{7f}".to_owned(),
            "\u{e9}".to_owned(),
            "\u{202e}".to_owned(),
        ] {
            reject(
                &|s| s.terminal.word_separators = bad.clone(),
                "terminal.wordSeparators",
            );
        }
        for bad in [
            "",
            "#",
            "#12345",
            "#1234567",
            "123456",
            "#12345g",
            "#ff00ff ",
            " #ff00ff",
            "red",
            "#fff",
            "#\u{ff11}23456",
            "url(x)",
        ] {
            reject(
                &|s| s.terminal.background = Some(bad.to_owned()),
                "terminal.background",
            );
        }
        for bad in [1, 4, 15, 61, 3600, u32::MAX] {
            reject(&|s| s.files.auto_refresh = bad, "files.autoRefresh");
        }
        for bad in ["a b", "a\u{0}", "\t", &"u".repeat(256)] {
            reject(
                &|s| s.connection.default_user = bad.to_owned(),
                "connection.defaultUser",
            );
        }

        let mut ok = default_settings();
        ok.terminal.scrollback = 10;
        ok.editor.max_file_size_mib = 256;
        ok.appearance.ui_font_size = 16;
        ok.terminal.letter_spacing = 5;
        ok.terminal.minimum_contrast_ratio = 4.5;
        ok.terminal.word_separators = String::new();
        ok.terminal.scroll_sensitivity = 10;
        ok.terminal.background = Some("#0F1115".into());
        ok.files.auto_refresh = 60;
        ok.connection.default_user = "alice".into();
        validate(&ok).unwrap();
        ok.terminal.scrollback = 1_000_000;
        ok.terminal.word_separators = "~".repeat(64);
        ok.terminal.background = Some("#fbf9f4".into());
        validate(&ok).unwrap();
    }

    #[test]
    fn v3_enums_roundtrip_and_load_clamps() {
        use crate::ipc::dto::{AccentColor, ConfirmQuit, ReduceMotion, StartupAction, ThemeMode};
        let dir = tempfile::tempdir().unwrap();
        let mut s = default_settings();
        s.general.confirm_quit = ConfirmQuit::Never;
        s.general.on_startup = StartupAction::Restore;
        s.appearance.theme = ThemeMode::System;
        s.appearance.accent = AccentColor::Pink;
        s.appearance.reduce_motion = ReduceMotion::On;
        s.terminal.bell = TerminalBell::Sound;
        s.terminal.on_exit = TerminalExitAction::CloseOnSuccess;
        s.terminal.new_terminal_cwd = NewTerminalCwd::CurrentFolder;
        s.terminal.term_type = TermType::Vt100;
        s.editor.new_file_eol = NewFileEol::Crlf;
        s.files.double_click = DoubleClickAction::Download;
        s.files.size_units = SizeUnits::Decimal;
        s.files.date_format = DateFormat::Absolute;
        update(dir.path(), false, &s).unwrap();
        let raw: Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("settings.json")).unwrap())
                .unwrap();
        assert_eq!(raw["version"], 1);
        assert_eq!(raw["general"]["confirmQuit"], "never");
        assert_eq!(raw["general"]["onStartup"], "restore");
        assert_eq!(raw["appearance"]["theme"], "system");
        assert_eq!(raw["appearance"]["reduceMotion"], "on");
        assert_eq!(raw["terminal"]["bell"], "sound");
        assert_eq!(raw["terminal"]["onExit"], "closeOnSuccess");
        assert_eq!(raw["terminal"]["newTerminalCwd"], "currentFolder");
        assert_eq!(raw["terminal"]["termType"], "vt100");
        assert_eq!(raw["editor"]["newFileEol"], "crlf");
        assert_eq!(raw["files"]["doubleClick"], "download");
        assert_eq!(load(dir.path()).0, s);
        for (t, want) in [
            (TermType::Xterm256Color, "xterm-256color"),
            (TermType::Xterm, "xterm"),
            (TermType::Vt100, "vt100"),
        ] {
            assert_eq!(serde_json::to_value(t).unwrap(), want);
            assert_eq!(t.as_term(), want);
        }

        // Unknown enum strings make the file corrupt (like v2).
        for bad in [
            br#"{"version":1,"terminal":{"termType":"xterm-kitty"}}"#.as_slice(),
            br#"{"version":1,"appearance":{"theme":"solarized"}}"#.as_slice(),
            br#"{"version":1,"general":{"onStartup":"everything"}}"#.as_slice(),
        ] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("settings.json"), bad).unwrap();
            let (s, w) = load(dir.path());
            assert_eq!(s, default_settings());
            assert_eq!(
                serde_json::to_value(&w[0]).unwrap()["code"],
                "storageCorrupt"
            );
        }

        // Out-of-range numbers are clamped, bad strings reset.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            br##"{"version":1,"appearance":{"uiFontSize":40},"terminal":{"scrollback":5,"letterSpacing":99,"minimumContrastRatio":50,"scrollSensitivity":0,"wordSeparators":"a\nb","background":"#zzzzzz"},"editor":{"maxFileSizeMiB":9999},"files":{"autoRefresh":7},"connection":{"defaultUser":"bad user"},"transfers":{"maxConcurrent":500}}"##,
        )
        .unwrap();
        let (s, w) = load(dir.path());
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(s.appearance.ui_font_size, 16);
        assert_eq!(s.terminal.scrollback, 10);
        assert_eq!(s.terminal.letter_spacing, 5);
        assert!((s.terminal.minimum_contrast_ratio - 21.0).abs() < f64::EPSILON);
        assert_eq!(s.terminal.scroll_sensitivity, 1);
        assert_eq!(s.terminal.word_separators, DEFAULT_WORD_SEPARATORS);
        assert_eq!(s.terminal.background, None);
        assert_eq!(s.editor.max_file_size_mib, 256);
        assert_eq!(s.files.auto_refresh, 0);
        assert_eq!(s.connection.default_user, "");
        assert_eq!(s.transfers.max_concurrent, 64);
        validate(&s).unwrap();
    }

    /// Updater: `general.autoUpdate` defaults to on when missing, round-trips without a version
    /// bump, and a value of the wrong type is rejected like every other field.
    #[test]
    fn updater_settings_default_roundtrip_and_validate() {
        assert!(default_settings().general.auto_update);

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            br#"{"version":1,"general":{"confirmCloseWithTerminals":false}}"#,
        )
        .unwrap();
        let (s, w) = load(dir.path());
        assert!(w.is_empty(), "{w:?}");
        assert!(s.general.auto_update);

        let mut s = default_settings();
        s.general.auto_update = false;
        update(dir.path(), false, &s).unwrap();
        let raw: Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("settings.json")).unwrap())
                .unwrap();
        assert_eq!(raw["version"], 1);
        assert_eq!(raw["general"]["autoUpdate"], false);
        assert_eq!(load(dir.path()).0, s);

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            br#"{"version":1,"general":{"autoUpdate":"yes"}}"#,
        )
        .unwrap();
        let (s, w) = load(dir.path());
        assert_eq!(s, default_settings());
        assert_eq!(
            serde_json::to_value(&w[0]).unwrap()["code"],
            "storageCorrupt"
        );
    }

    /// The removed `general.updateChannel` (0.1.x and 0.2.0-experimental.1 wrote `"stable"` or
    /// `"experimental"`) is an unknown key now: any value, even garbage, loads without a warning,
    /// quarantine or version bump, every other stored value is kept, and the next save drops it.
    #[test]
    fn removed_update_channel_is_ignored_on_load() {
        for channel in [
            r#""experimental""#,
            r#""stable""#,
            r#""nightly""#,
            "42",
            "null",
            r#"{"x":[1,2]}"#,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let text = format!(
                r#"{{"version":1,"locale":"uk","terminal":{{"fontSize":17}},
                    "general":{{"autoUpdate":false,"updateChannel":{channel},"notifyOnDisconnect":true}}}}"#
            );
            std::fs::write(dir.path().join("settings.json"), text).unwrap();
            let (s, w) = load(dir.path());
            assert!(w.is_empty(), "{channel}: {w:?}");
            let mut expected = default_settings();
            expected.locale = "uk".into();
            expected.terminal.font_size = 17;
            expected.general.auto_update = false;
            expected.general.notify_on_disconnect = true;
            assert_eq!(s, expected, "{channel}");
            let names: Vec<_> = std::fs::read_dir(dir.path())
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            assert_eq!(names, ["settings.json"], "{channel}: no quarantine copy");

            update(dir.path(), false, &s).unwrap();
            let raw: Value =
                serde_json::from_slice(&std::fs::read(dir.path().join("settings.json")).unwrap())
                    .unwrap();
            assert_eq!(raw["version"], 1);
            assert!(raw["general"].get("updateChannel").is_none(), "{raw}");
            let (again, w) = load(dir.path());
            assert!(w.is_empty(), "{channel}: {w:?}");
            assert_eq!(again, expected, "{channel}");
        }
    }

    /// `editor.showWhitespace` was a boolean before it had three modes: a stored `true`/`false`
    /// loads as `all`/`none` without a warning, quarantine or version bump, every other stored
    /// value is kept, and the next save writes the string.
    #[test]
    fn legacy_boolean_show_whitespace_loads_as_a_mode() {
        for (stored, mode, written) in [
            ("true", ShowWhitespace::All, "all"),
            ("false", ShowWhitespace::None, "none"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let text = format!(
                r#"{{"version":1,"locale":"uk","editor":{{"fontSize":15,"showWhitespace":{stored},"codeFolding":false}}}}"#
            );
            std::fs::write(dir.path().join("settings.json"), text).unwrap();
            let (s, w) = load(dir.path());
            assert!(w.is_empty(), "{stored}: {w:?}");
            let mut expected = default_settings();
            expected.locale = "uk".into();
            expected.editor.font_size = 15;
            expected.editor.code_folding = false;
            expected.editor.show_whitespace = mode;
            assert_eq!(s, expected, "{stored}");
            let names: Vec<_> = std::fs::read_dir(dir.path())
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            assert_eq!(names, ["settings.json"], "{stored}: no quarantine copy");

            update(dir.path(), false, &s).unwrap();
            let raw: Value =
                serde_json::from_slice(&std::fs::read(dir.path().join("settings.json")).unwrap())
                    .unwrap();
            assert_eq!(raw["version"], 1);
            assert_eq!(raw["editor"]["showWhitespace"], written);
            assert_eq!(load(dir.path()).0, expected, "{stored}");
        }
    }

    #[test]
    fn show_whitespace_modes_roundtrip() {
        for (mode, json) in [
            (ShowWhitespace::None, "none"),
            (ShowWhitespace::ActiveLine, "activeLine"),
            (ShowWhitespace::All, "all"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let mut s = default_settings();
            s.editor.show_whitespace = mode;
            update(dir.path(), false, &s).unwrap();
            let raw: Value =
                serde_json::from_slice(&std::fs::read(dir.path().join("settings.json")).unwrap())
                    .unwrap();
            assert_eq!(raw["editor"]["showWhitespace"], json);
            let (loaded, w) = load(dir.path());
            assert!(w.is_empty(), "{json}: {w:?}");
            assert_eq!(loaded, s);
        }
    }

    /// Anything else in `editor.showWhitespace` is a wrong type: only that field falls back to
    /// `none`, the other stored values are kept, and the file is quarantined with a warning.
    #[test]
    fn invalid_show_whitespace_resets_only_that_field() {
        for bad in [
            r#""ActiveLine""#,
            r#""active-line""#,
            r#""""#,
            "1",
            "0",
            "null",
            "[true]",
            r#"{"all":true}"#,
            r#""all ""#,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let text =
                format!(r#"{{"version":1,"editor":{{"tabSize":2,"showWhitespace":{bad}}}}}"#);
            std::fs::write(dir.path().join("settings.json"), text).unwrap();
            let (s, w) = load(dir.path());
            let mut expected = default_settings();
            expected.editor.tab_size = 2;
            assert_eq!(s, expected, "{bad}");
            assert_eq!(
                serde_json::to_value(&w[0]).unwrap()["code"],
                "storageCorrupt",
                "{bad}"
            );
        }
    }

    #[test]
    fn defaults_are_valid() {
        validate(&default_settings()).unwrap();
        assert_eq!(clamp(default_settings()), default_settings());
    }

    #[test]
    fn out_of_range_is_rejected_with_field_path() {
        let mut s = default_settings();
        s.connection.prompt_timeout_secs = 111;
        assert_eq!(
            validate(&s).unwrap_err().kind,
            ErrorCode::InvalidArgument {
                field: "connection.promptTimeoutSecs".into()
            }
        );
        let mut s = default_settings();
        s.editor.max_file_size_mib = 0;
        assert_eq!(
            validate(&s).unwrap_err().kind,
            ErrorCode::InvalidArgument {
                field: "editor.maxFileSizeMiB".into()
            }
        );
        let mut s = default_settings();
        s.ui.sidebar_width = f64::NAN;
        assert_eq!(
            validate(&s).unwrap_err().kind,
            ErrorCode::InvalidArgument {
                field: "ui.sidebarWidth".into()
            }
        );
        let mut s = default_settings();
        s.locale = "../../etc".into();
        assert_eq!(
            validate(&s).unwrap_err().kind,
            ErrorCode::InvalidArgument {
                field: "locale".into()
            }
        );
    }

    #[test]
    fn clamp_on_load() {
        let mut s = default_settings();
        s.terminal.font_size = 99;
        s.terminal.scrollback = 1;
        s.connection.reconnect.max_attempts = 1_000;
        s.ui.terminal_panel_height = f64::INFINITY;
        let c = clamp(s);
        assert_eq!(c.terminal.font_size, 32);
        assert_eq!(c.terminal.scrollback, 10);
        assert_eq!(c.connection.reconnect.max_attempts, 10);
        assert!((c.ui.terminal_panel_height - 260.0).abs() < f64::EPSILON);
        validate(&c).unwrap();
    }
}
