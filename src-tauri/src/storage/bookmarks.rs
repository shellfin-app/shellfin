//! Bookmarks and groups (§7.2, §7.3, §5.6 password binding).
//!
//! On disk (`bookmarks.json`, version 2) the `password` auth additionally stores the
//! backend-owned `passwordBinding {hostname, port, user}`; the `Bookmark` DTO never exposes it.
//! v2 adds `order` (dense 0..n per group / among groups, normalised on load and on every write)
//! and the optional `color` (an unknown value loads as "no color").
//!
//! The same field rules apply on load ([`validate_all`], which drops invalid entries) and on
//! every mutation (the per-entry checks behind `save`/`group_save`/import, plus
//! `check_all_chains` for jump chains through the changed bookmark):
//! - ids: 1–64 chars of `[A-Za-z0-9-]` (they become keychain account names `bookmark:<id>`);
//! - `name`: 1–100 chars, no control characters, unique (case-insensitive) — per collection;
//! - `host`: [`valid_hostname`] (→ `invalidHostname{value}`); `port`: 1–65535;
//! - `user`: 1–255 chars, no whitespace/control characters;
//! - `auth.identityFile` (key auth): 1–4096 chars, no NUL;
//! - `groupId`: must exist; `proxyJump`: `spec` = comma list of `[user@]host[:port]`,
//!   `sshConfig` alias passes `valid_hostname`, `bookmark` id exists, is not the bookmark itself
//!   and the bookmark-to-bookmark jump chain has no cycle and at most `MAX_JUMPS` (8) jumps
//!   (the connect-time limit); a save that would push another bookmark's chain past it
//!   is refused;
//! - `tags`: ≤ 20 tags of 1–50 chars without control characters;
//! - `startDir`: ≤ 4096 chars, no NUL.
//!
//! On load, invalid entries are dropped (iterated to a fixpoint, since dropping a group or a
//! jump target can invalidate others); the original document is kept as
//! `bookmarks.corrupt-<ms>.json` and a `storageCorrupt` warning is returned.
//!
//! Deleting a bookmark clears every `proxyJump` that pointed to it; deleting a group moves its
//! bookmarks to "ungrouped". Changing host, port, user, useSshConfig or proxyJump of a bookmark
//! with a saved password — or switching its auth away from `password` — forgets the password
//! (`SaveOutcome::forget_password`; the caller deletes the keychain entry).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{AppError, EntityKind, ErrorCode, IoOp};
use crate::ipc::dto::{
    AuthConfig, AuthInput, Bookmark, BookmarkColor, BookmarkInput, BookmarksDto, Group, GroupInput,
    JumpRef,
};
use crate::ipc::secret::SensitiveText;
use crate::sshconfig::validate::{parse_host_spec, valid_hostname};
use crate::storage::file::{io_error, quarantine_copy, write_json};
use crate::storage::migrations::StoreFile;
use crate::storage::{LoadedDoc, load_document};

mod io;

pub use io::{ImportOutcome, ParsedImport, export_json, export_ssh_config, parse_import};

pub const MAX_NAME_CHARS: usize = 100;
pub const MAX_USER_CHARS: usize = 255;
pub const MAX_PATH_CHARS: usize = 4096;
pub const MAX_TAGS: usize = 20;
pub const MAX_TAG_CHARS: usize = 50;
pub const MAX_ID_CHARS: usize = 64;
/// `startupCommand` / `notes` lengths (chars).
pub const MAX_STARTUP_COMMAND_CHARS: usize = 1024;
pub const MAX_NOTES_CHARS: usize = 4000;
/// Upper bound for `copy_from_ssh_config` name de-duplication (`alias (2)` …).
const MAX_NAME_SUFFIX: u32 = 1000;

/// Keychain binding of a saved password (§5.6 D, §7.2). Storage-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PasswordBinding {
    pub hostname: String,
    pub port: u16,
    pub user: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "camelCase")]
enum StoredAuth {
    Auto,
    #[serde(rename_all = "camelCase")]
    Key {
        identity_file: String,
    },
    #[serde(rename_all = "camelCase")]
    Password {
        password_saved: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        password_binding: Option<PasswordBinding>,
    },
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredBookmark {
    id: String,
    name: String,
    host: String,
    port: Option<u16>,
    user: Option<String>,
    auth: StoredAuth,
    proxy_jump: Option<JumpRef>,
    group_id: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    start_dir: Option<String>,
    use_ssh_config: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    color: Option<BookmarkColor>,
    /// Position among the bookmarks of the same group (schema v2); normalised to 0..n.
    #[serde(default)]
    order: i32,
    #[serde(
        default,
        rename = "uploadLimitKiBps",
        skip_serializing_if = "Option::is_none"
    )]
    upload_limit_kibps: Option<u32>,
    #[serde(
        default,
        rename = "downloadLimitKiBps",
        skip_serializing_if = "Option::is_none"
    )]
    download_limit_kibps: Option<u32>,
    /// (no schema bump: missing = default).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    confirm_connect: bool,
    /// Never logged (`Debug` is masked).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    startup_command: Option<SensitiveText>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    notes: Option<String>,
    created_at_ms: i64,
    updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredGroup {
    id: String,
    name: String,
    order: i32,
}

/// The on-disk document. Entries are kept as raw JSON on load so that one malformed entry is
/// dropped instead of failing the whole file.
#[derive(Debug, Deserialize)]
struct RawFile {
    #[serde(default)]
    groups: Vec<Value>,
    #[serde(default)]
    bookmarks: Vec<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileOut<'a> {
    version: u64,
    groups: &'a [StoredGroup],
    bookmarks: &'a [StoredBookmark],
}

#[derive(Debug, Clone)]
pub struct SaveOutcome {
    pub bookmark: Bookmark,
    /// A saved password was invalidated by this edit: delete keychain entry `bookmark:<id>`.
    pub forget_password: bool,
}

#[derive(Debug, Clone)]
pub struct DeleteOutcome {
    /// The bookmark had a saved password: delete keychain entry `bookmark:<id>`.
    pub had_password: bool,
    /// Bookmarks that jumped via the deleted one and had a saved password; their jump was
    /// cleared (binding-relevant), so delete keychain entries `bookmark:<id>` for them too.
    pub forgotten_dependents: Vec<String>,
}

#[derive(Debug)]
pub struct BookmarkStore {
    path: PathBuf,
    read_only: bool,
    groups: Vec<StoredGroup>,
    bookmarks: Vec<StoredBookmark>,
    /// Bookmark ids whose keychain entry can no longer be used: dropped by validate-on-load, or
    /// `passwordSaved` without a binding (normalised to "not saved"). The app deletes
    /// those entries after startup.
    orphaned_keychain_ids: Vec<String>,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_CHARS
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn valid_name(name: &str) -> bool {
    let n = name.chars().count();
    (1..=MAX_NAME_CHARS).contains(&n) && !name.chars().any(char::is_control) && name.trim() == name
}

/// A bookmark `user`: 1–255 chars, no whitespace or control characters.
pub fn valid_user(u: &str) -> bool {
    (1..=MAX_USER_CHARS).contains(&u.chars().count())
        && !u.chars().any(|c| c.is_whitespace() || c.is_control())
}

fn valid_path_text(s: &str) -> bool {
    (1..=MAX_PATH_CHARS).contains(&s.chars().count()) && !s.contains('\0')
}

fn invalid(field: &str) -> AppError {
    AppError::invalid_argument(field)
}

/// A `startupCommand` is one line of ≤ 1024 chars without any control character
/// (C0 incl. tab/CR/LF, DEL, C1) or Unicode line/paragraph separator: it is typed into a shell,
/// where such characters would run or alter commands.
pub fn valid_startup_command(s: &str) -> bool {
    (1..=MAX_STARTUP_COMMAND_CHARS).contains(&s.chars().count())
        && !s
            .chars()
            .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
}

/// `notes` are plain text of ≤ 4000 chars; LF, CR and tab are the only control
/// characters allowed.
pub fn valid_notes(s: &str) -> bool {
    (1..=MAX_NOTES_CHARS).contains(&s.chars().count())
        && !s
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
}

/// Whitespace-only (or empty) text means "none"; anything else is kept verbatim.
fn none_if_whitespace(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.trim().is_empty())
}

fn validate_group(g: &StoredGroup, others: &[StoredGroup]) -> Result<(), AppError> {
    if !valid_id(&g.id) {
        return Err(invalid("id"));
    }
    if !valid_name(&g.name) {
        return Err(invalid("name"));
    }
    let lower = g.name.to_lowercase();
    if others
        .iter()
        .any(|o| o.id != g.id && o.name.to_lowercase() == lower)
    {
        return Err(invalid("name"));
    }
    Ok(())
}

fn validate_jump_spec(spec: &str) -> bool {
    !spec.is_empty() && spec.split(',').all(|part| parse_host_spec(part).is_some())
}

/// Most bookmark→bookmark jumps in one chain: the connect-time limit
/// ([`crate::ssh::target::MAX_JUMPS`]), so a stored bookmark can always be resolved without deep
/// recursion.
pub(crate) const MAX_BOOKMARK_JUMPS: usize = crate::ssh::target::MAX_JUMPS;

/// Lookups for validating many bookmarks against one collection, built once per pass (/// no per-comparison allocation, no per-call map rebuild).
struct BookmarkIndex<'a> {
    by_id: HashMap<&'a str, &'a StoredBookmark>,
    /// Lower-cased name → ids carrying it.
    names: HashMap<String, Vec<&'a str>>,
    groups: HashSet<&'a str>,
}

impl<'a> BookmarkIndex<'a> {
    fn new(all: &'a [StoredBookmark], groups: &'a [StoredGroup]) -> Self {
        let mut names: HashMap<String, Vec<&'a str>> = HashMap::with_capacity(all.len());
        for b in all {
            names
                .entry(b.name.to_lowercase())
                .or_default()
                .push(b.id.as_str());
        }
        Self {
            by_id: all.iter().map(|b| (b.id.as_str(), b)).collect(),
            names,
            groups: groups.iter().map(|g| g.id.as_str()).collect(),
        }
    }

    fn get(&self, id: &str) -> Option<&'a StoredBookmark> {
        self.by_id.get(id).copied()
    }

    fn name_taken_by_other(&self, b: &StoredBookmark) -> bool {
        self.names
            .get(&b.name.to_lowercase())
            .is_some_and(|ids| ids.iter().any(|id| *id != b.id))
    }

    /// Following bookmark→bookmark jumps from `start`'s jump `next`: does the chain loop, or
    /// take more than [`MAX_BOOKMARK_JUMPS`] jumps? Walks at most `MAX_BOOKMARK_JUMPS + 1`
    /// steps.
    fn jump_chain_bad(&self, start: &str, next: &str) -> bool {
        let mut seen: HashSet<&str> = HashSet::from([start]);
        let mut cur = next;
        let mut depth = 1usize;
        loop {
            if !seen.insert(cur) || depth > MAX_BOOKMARK_JUMPS {
                return true;
            }
            match self.by_id.get(cur).and_then(|b| b.proxy_jump.as_ref()) {
                Some(JumpRef::Bookmark { id }) => {
                    cur = id;
                    depth += 1;
                }
                _ => return false,
            }
        }
    }

    /// Bookmarks whose jump chain (as stored) loops or is too deep.
    fn bad_chains(&self) -> Vec<&'a str> {
        self.by_id
            .values()
            .filter_map(|b| match &b.proxy_jump {
                Some(JumpRef::Bookmark { id }) if self.jump_chain_bad(&b.id, id) => {
                    Some(b.id.as_str())
                }
                _ => None,
            })
            .collect()
    }
}

/// Validate one bookmark against the rest of the collection.
fn validate_bookmark(
    b: &StoredBookmark,
    all: &[StoredBookmark],
    groups: &[StoredGroup],
) -> Result<(), AppError> {
    validate_bookmark_in(b, &BookmarkIndex::new(all, groups))
}

/// [`validate_bookmark`] with a prebuilt index of the collection.
fn validate_bookmark_in(b: &StoredBookmark, idx: &BookmarkIndex<'_>) -> Result<(), AppError> {
    if !valid_id(&b.id) {
        return Err(invalid("id"));
    }
    if !valid_name(&b.name) {
        return Err(invalid("name"));
    }
    if idx.name_taken_by_other(b) {
        return Err(invalid("name"));
    }
    if !valid_hostname(&b.host) {
        return Err(AppError::new(ErrorCode::InvalidHostname {
            value: b.host.clone(),
        }));
    }
    if b.port == Some(0) {
        return Err(invalid("port"));
    }
    if let Some(u) = &b.user
        && !valid_user(u)
    {
        return Err(invalid("user"));
    }
    match &b.auth {
        StoredAuth::Key { identity_file } if !valid_path_text(identity_file) => {
            return Err(invalid("auth.identityFile"));
        }
        StoredAuth::Password {
            password_saved: false,
            password_binding: Some(_),
        } => {
            return Err(invalid("auth"));
        }
        _ => {}
    }
    if let Some(g) = &b.group_id
        && !idx.groups.contains(g.as_str())
    {
        return Err(invalid("groupId"));
    }
    match &b.proxy_jump {
        None => {}
        Some(JumpRef::Spec { spec }) if !validate_jump_spec(spec) => {
            return Err(invalid("proxyJump"));
        }
        Some(JumpRef::SshConfig { alias }) if !valid_hostname(alias) => {
            return Err(invalid("proxyJump"));
        }
        Some(JumpRef::Bookmark { id }) => {
            if id == &b.id || !idx.by_id.contains_key(id.as_str()) || idx.jump_chain_bad(&b.id, id)
            {
                return Err(invalid("proxyJump"));
            }
        }
        Some(_) => {}
    }
    for (limit, field) in [
        (b.upload_limit_kibps, "uploadLimitKiBps"),
        (b.download_limit_kibps, "downloadLimitKiBps"),
    ] {
        if limit.is_some_and(|v| v > crate::storage::settings::ranges::TRANSFER_LIMIT_KIBPS.1) {
            return Err(invalid(field));
        }
    }
    if b.tags.len() > MAX_TAGS
        || b.tags.iter().any(|t| {
            let n = t.chars().count();
            !(1..=MAX_TAG_CHARS).contains(&n) || t.chars().any(char::is_control)
        })
    {
        return Err(invalid("tags"));
    }
    if let Some(d) = &b.start_dir
        && !valid_path_text(d)
    {
        return Err(invalid("startDir"));
    }
    if b.startup_command
        .as_ref()
        .is_some_and(|c| !valid_startup_command(c.expose()))
    {
        return Err(invalid("startupCommand"));
    }
    if b.notes.as_deref().is_some_and(|n| !valid_notes(n)) {
        return Err(invalid("notes"));
    }
    Ok(())
}

/// After a mutation: no *other* bookmark's chain may loop or grow past
/// [`MAX_BOOKMARK_JUMPS`] through the changed one (it would be dropped on the next load).
fn check_all_chains(all: &[StoredBookmark], groups: &[StoredGroup]) -> Result<(), AppError> {
    if BookmarkIndex::new(all, groups).bad_chains().is_empty() {
        Ok(())
    } else {
        Err(invalid("proxyJump"))
    }
}

/// Drop invalid entries until nothing changes. Returns the number of dropped entries.
/// Each pass is O(n) (one index, chain walks bounded by [`MAX_BOOKMARK_JUMPS`]); a drop can only
/// invalidate bookmarks that jump through it, at most `MAX_BOOKMARK_JUMPS` levels up, so the
/// number of passes is bounded too.
fn validate_all(groups: &mut Vec<StoredGroup>, bookmarks: &mut Vec<StoredBookmark>) -> usize {
    let mut dropped = 0;
    // Groups: keep the first of duplicate ids/names.
    let mut kept: Vec<StoredGroup> = Vec::with_capacity(groups.len());
    for g in groups.drain(..) {
        if kept.iter().all(|k| k.id != g.id) && validate_group(&g, &kept).is_ok() {
            kept.push(g);
        } else {
            dropped += 1;
        }
    }
    *groups = kept;
    // Bookmarks: drop duplicate ids (keep first) and later duplicate names, then iterate.
    let mut unique: Vec<StoredBookmark> = Vec::with_capacity(bookmarks.len());
    let mut seen_ids: HashSet<String> = HashSet::with_capacity(bookmarks.len());
    let mut seen_names: HashSet<String> = HashSet::with_capacity(bookmarks.len());
    for b in bookmarks.drain(..) {
        let lower = b.name.to_lowercase();
        if seen_ids.contains(&b.id) || seen_names.contains(&lower) {
            dropped += 1;
        } else {
            seen_ids.insert(b.id.clone());
            seen_names.insert(lower);
            unique.push(b);
        }
    }
    loop {
        let before = unique.len();
        let bad: HashSet<String> = {
            let idx = BookmarkIndex::new(&unique, groups);
            unique
                .iter()
                .filter(|b| validate_bookmark_in(b, &idx).is_err())
                .map(|b| b.id.clone())
                .collect()
        };
        unique.retain(|b| !bad.contains(&b.id));
        dropped += before - unique.len();
        if unique.len() == before {
            break;
        }
    }
    *bookmarks = unique;
    dropped
}

/// Make `order` dense (0..n): groups by their order, bookmarks per `groupId` by their order. The
/// sort is stable, so ties (and the all-zero orders of a hand-edited file) keep file order.
fn normalize_order(groups: &mut [StoredGroup], bookmarks: &mut [StoredBookmark]) {
    groups.sort_by_key(|g| g.order);
    for (rank, g) in groups.iter_mut().enumerate() {
        g.order = i32::try_from(rank).unwrap_or(i32::MAX);
    }
    let mut ranked: Vec<&mut StoredBookmark> = bookmarks.iter_mut().collect();
    ranked.sort_by_key(|b| b.order);
    let mut next: HashMap<Option<String>, i32> = HashMap::new();
    for b in ranked {
        let n = next.entry(b.group_id.clone()).or_insert(0);
        b.order = *n;
        *n = n.saturating_add(1);
    }
}

/// Place `id` at `index` (clamped) among `ids`, the current members in order.
fn insert_at(mut ids: Vec<String>, id: &str, index: u32) -> Vec<String> {
    ids.retain(|i| i != id);
    let at = usize::try_from(index).map_or(ids.len(), |i| i.min(ids.len()));
    ids.insert(at, id.to_owned());
    ids
}

/// An unknown `color` value (newer palette, hand edit) becomes "no color" instead of dropping
/// the whole bookmark.
fn sanitize_color(mut v: Value) -> Value {
    if let Some(obj) = v.as_object_mut()
        && let Some(c) = obj.get("color")
        && !c.is_null()
        && serde_json::from_value::<BookmarkColor>(c.clone()).is_err()
    {
        obj.remove("color");
    }
    v
}

/// An invalid `startupCommand` or `notes` (hand edit) loads as "none" instead of
/// dropping the whole bookmark; a wrong `confirmConnect` type loads as `false`.
fn sanitize_v3_fields(mut v: Value) -> Value {
    if let Some(obj) = v.as_object_mut() {
        let bad_command = obj.get("startupCommand").is_some_and(|c| {
            !c.is_null()
                && !c
                    .as_str()
                    .is_some_and(|s| valid_startup_command(s) && !s.trim().is_empty())
        });
        if bad_command {
            obj.remove("startupCommand");
        }
        let bad_notes = obj.get("notes").is_some_and(|n| {
            !n.is_null()
                && !n
                    .as_str()
                    .is_some_and(|s| valid_notes(s) && !s.trim().is_empty())
        });
        if bad_notes {
            obj.remove("notes");
        }
        if obj.get("confirmConnect").is_some_and(|c| !c.is_boolean()) {
            obj.remove("confirmConnect");
        }
    }
    v
}

fn to_dto(b: &StoredBookmark) -> Bookmark {
    Bookmark {
        id: b.id.clone(),
        name: b.name.clone(),
        host: b.host.clone(),
        port: b.port,
        user: b.user.clone(),
        auth: match &b.auth {
            StoredAuth::Auto => AuthConfig::Auto,
            StoredAuth::Key { identity_file } => AuthConfig::Key {
                identity_file: identity_file.clone(),
            },
            StoredAuth::Password { password_saved, .. } => AuthConfig::Password {
                password_saved: *password_saved,
            },
            StoredAuth::Agent => AuthConfig::Agent,
        },
        proxy_jump: b.proxy_jump.clone(),
        group_id: b.group_id.clone(),
        tags: b.tags.clone(),
        start_dir: b.start_dir.clone(),
        use_ssh_config: b.use_ssh_config,
        color: b.color,
        order: b.order,
        upload_limit_kibps: b.upload_limit_kibps,
        download_limit_kibps: b.download_limit_kibps,
        confirm_connect: b.confirm_connect,
        startup_command: b.startup_command.clone(),
        notes: b.notes.clone(),
        created_at_ms: b.created_at_ms,
        updated_at_ms: b.updated_at_ms,
    }
}

fn group_dto(g: &StoredGroup) -> Group {
    Group {
        id: g.id.clone(),
        name: g.name.clone(),
        order: g.order,
    }
}

/// Keep the saved password of `old` on `candidate` when nothing binding-relevant changed (host,
/// port, user, `useSshConfig`, `proxyJump`, auth still `password`). Returns `true` when `old` had
/// a saved password that must be forgotten (the caller deletes the keychain entry).
/// Whether `candidate` may reach a different server or account than `old` (host, port, user,
/// ssh-config use or jump chain changed). Secrets bound to the old target must not follow.
fn connection_target_changed(old: &StoredBookmark, candidate: &StoredBookmark) -> bool {
    old.host != candidate.host
        || old.port != candidate.port
        || old.user != candidate.user
        || old.use_ssh_config != candidate.use_ssh_config
        || old.proxy_jump != candidate.proxy_jump
}

fn carry_password(old: &StoredBookmark, candidate: &mut StoredBookmark) -> bool {
    let StoredAuth::Password {
        password_saved: true,
        password_binding,
    } = &old.auth
    else {
        return false;
    };
    let binding_relevant_change = connection_target_changed(old, candidate);
    match &mut candidate.auth {
        StoredAuth::Password {
            password_saved,
            password_binding: new_binding,
        } if !binding_relevant_change => {
            *password_saved = true;
            new_binding.clone_from(password_binding);
            false
        }
        _ => true,
    }
}

fn none_if_blank(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

fn not_found(what: EntityKind, id: &str) -> AppError {
    AppError::new(ErrorCode::NotFound {
        what,
        id: id.to_owned(),
    })
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

impl BookmarkStore {
    /// Load `<dir>/bookmarks.json` (missing → empty). Never fails: problems become warnings.
    pub fn load(dir: &Path) -> (Self, Vec<AppError>) {
        let path = dir.join(StoreFile::Bookmarks.file_name());
        let LoadedDoc {
            value,
            read_only,
            mut warnings,
        } = load_document(&path, StoreFile::Bookmarks);
        let mut store = Self {
            path,
            read_only,
            groups: Vec::new(),
            bookmarks: Vec::new(),
            orphaned_keychain_ids: Vec::new(),
        };
        let Some(value) = value else {
            return (store, warnings);
        };
        let raw: RawFile = match serde_json::from_value(value.clone()) {
            Ok(r) => r,
            Err(_) if store.read_only => {
                // Never touch the files of a read-only store (newer version, second instance).
                warnings.push(AppError::new(ErrorCode::StorageReadOnly));
                return (store, warnings);
            }
            Err(e) => {
                // Structurally wrong document (e.g. `bookmarks` is not an array).
                match quarantine_copy(
                    &store.path,
                    &serde_json::to_vec_pretty(&value).unwrap_or_default(),
                ) {
                    Ok(q) => warnings.push(crate::storage::file::corrupt_warning(
                        &store.path,
                        &q,
                        e.to_string(),
                    )),
                    Err(io) => warnings.push(io_error(IoOp::WriteStorage, &store.path, &io)),
                }
                store.persist_or_warn(&mut warnings);
                return (store, warnings);
            }
        };
        let total = raw.groups.len() + raw.bookmarks.len();
        // Ids that claim a saved password (read from the raw JSON, so entries that fail to parse
        // or validate are covered too).
        let claimed_saved: Vec<String> = raw
            .bookmarks
            .iter()
            .filter(|v| v.pointer("/auth/passwordSaved") == Some(&serde_json::Value::Bool(true)))
            .filter_map(|v| v.get("id").and_then(serde_json::Value::as_str))
            .filter(|id| valid_id(id))
            .map(str::to_owned)
            .collect();
        let mut groups: Vec<StoredGroup> = raw
            .groups
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect();
        let mut bookmarks: Vec<StoredBookmark> = raw
            .bookmarks
            .into_iter()
            .map(sanitize_color)
            .map(sanitize_v3_fields)
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect();
        let malformed = total - groups.len() - bookmarks.len();
        let dropped = malformed + validate_all(&mut groups, &mut bookmarks);
        normalize_order(&mut groups, &mut bookmarks);
        // A saved password without a binding is not usable — mark it not saved.
        let mut unbound = 0usize;
        for b in &mut bookmarks {
            if let StoredAuth::Password {
                password_saved,
                password_binding: None,
            } = &mut b.auth
                && *password_saved
            {
                *password_saved = false;
                unbound += 1;
            }
        }
        // A read-only store (newer version, unreadable for writing) may hold entries
        // this version merely cannot understand; never delete their keychain entries.
        store.orphaned_keychain_ids = claimed_saved
            .into_iter()
            .filter(|_| !store.read_only)
            .filter(|id| {
                !bookmarks.iter().any(|b| {
                    b.id == *id
                        && matches!(
                            b.auth,
                            StoredAuth::Password {
                                password_saved: true,
                                ..
                            }
                        )
                })
            })
            .collect();
        store.groups = groups;
        store.bookmarks = bookmarks;
        if unbound > 0 && dropped == 0 {
            store.persist_or_warn(&mut warnings);
        }
        if dropped > 0 {
            let reason = format!("dropped {dropped} invalid entries");
            if store.read_only {
                warnings.push(AppError::new(ErrorCode::StorageReadOnly).with_detail(reason));
            } else {
                match quarantine_copy(
                    &store.path,
                    &serde_json::to_vec_pretty(&value).unwrap_or_default(),
                ) {
                    Ok(q) => warnings.push(crate::storage::file::corrupt_warning(
                        &store.path,
                        &q,
                        reason,
                    )),
                    Err(io) => warnings.push(io_error(IoOp::WriteStorage, &store.path, &io)),
                }
                store.persist_or_warn(&mut warnings);
            }
        }
        (store, warnings)
    }

    fn persist_or_warn(&self, warnings: &mut Vec<AppError>) {
        if !self.read_only
            && let Err(e) = self.write(&self.groups, &self.bookmarks)
        {
            warnings.push(e);
        }
    }

    fn write(&self, groups: &[StoredGroup], bookmarks: &[StoredBookmark]) -> Result<(), AppError> {
        let doc = FileOut {
            version: StoreFile::Bookmarks.current_version(),
            groups,
            bookmarks,
        };
        write_json(&self.path, &doc).map_err(|e| io_error(IoOp::WriteStorage, &self.path, &e))
    }

    /// Validate a candidate state, persist it, and only then commit it in memory.
    fn commit(
        &mut self,
        mut groups: Vec<StoredGroup>,
        mut bookmarks: Vec<StoredBookmark>,
    ) -> Result<(), AppError> {
        normalize_order(&mut groups, &mut bookmarks);
        self.write(&groups, &bookmarks)?;
        self.groups = groups;
        self.bookmarks = bookmarks;
        Ok(())
    }

    fn ensure_writable(&self) -> Result<(), AppError> {
        if self.read_only {
            Err(AppError::new(ErrorCode::StorageReadOnly))
        } else {
            Ok(())
        }
    }

    /// Ids whose keychain entries must be deleted (once, after startup).
    pub fn take_orphaned_keychain_ids(&mut self) -> Vec<String> {
        std::mem::take(&mut self.orphaned_keychain_ids)
    }

    pub fn read_only(&self) -> bool {
        self.read_only
    }

    pub fn dto(&self) -> BookmarksDto {
        BookmarksDto {
            bookmarks: self.bookmarks.iter().map(to_dto).collect(),
            groups: self.groups.iter().map(group_dto).collect(),
            read_only: self.read_only,
        }
    }

    pub fn get(&self, id: &str) -> Option<Bookmark> {
        self.bookmarks.iter().find(|b| b.id == id).map(to_dto)
    }

    /// The storage-only binding of a saved password (`None` if no password is saved).
    pub fn password_binding(&self, id: &str) -> Option<PasswordBinding> {
        self.bookmarks
            .iter()
            .find(|b| b.id == id)
            .and_then(|b| match &b.auth {
                StoredAuth::Password {
                    password_saved: true,
                    password_binding,
                } => password_binding.clone(),
                _ => None,
            })
    }

    /// Create (`input.id == null`) or update a bookmark.
    pub fn save(&mut self, input: BookmarkInput, now_ms: i64) -> Result<SaveOutcome, AppError> {
        self.ensure_writable()?;
        let existing = match &input.id {
            Some(id) => Some(
                self.bookmarks
                    .iter()
                    .find(|b| &b.id == id)
                    .cloned()
                    .ok_or_else(|| not_found(EntityKind::Bookmark, id))?,
            ),
            None => None,
        };
        let mut candidate = StoredBookmark {
            id: input.id.clone().unwrap_or_else(new_id),
            name: input.name.trim().to_owned(),
            host: input.host.trim().to_owned(),
            port: input.port,
            user: none_if_blank(input.user),
            auth: match input.auth {
                AuthInput::Auto => StoredAuth::Auto,
                AuthInput::Key { identity_file } => StoredAuth::Key {
                    identity_file: identity_file.trim().to_owned(),
                },
                AuthInput::Password => StoredAuth::Password {
                    password_saved: false,
                    password_binding: None,
                },
                AuthInput::Agent => StoredAuth::Agent,
            },
            proxy_jump: input.proxy_jump.map(|j| match j {
                JumpRef::Spec { spec } => JumpRef::Spec {
                    spec: spec.trim().to_owned(),
                },
                other => other,
            }),
            group_id: input.group_id.clone(),
            tags: input
                .tags
                .into_iter()
                .map(|t| t.trim().to_owned())
                .collect(),
            start_dir: none_if_blank(input.start_dir),
            use_ssh_config: input.use_ssh_config,
            color: input.color,
            // A new bookmark, or one that changes group, goes to the end (normalised on commit).
            order: existing
                .as_ref()
                .filter(|e| e.group_id == input.group_id)
                .map_or(i32::MAX, |e| e.order),
            upload_limit_kibps: input.upload_limit_kibps,
            download_limit_kibps: input.download_limit_kibps,
            confirm_connect: input.confirm_connect.unwrap_or(false),
            startup_command: input
                .startup_command
                .filter(|c| !c.expose().trim().is_empty()),
            notes: none_if_whitespace(input.notes),
            created_at_ms: existing.as_ref().map_or(now_ms, |e| e.created_at_ms),
            updated_at_ms: now_ms,
        };
        let forget_password = existing
            .as_ref()
            .is_some_and(|old| carry_password(old, &mut candidate));
        let mut bookmarks = self.bookmarks.clone();
        match bookmarks.iter_mut().find(|b| b.id == candidate.id) {
            Some(slot) => *slot = candidate.clone(),
            None => bookmarks.push(candidate.clone()),
        }
        validate_bookmark(&candidate, &bookmarks, &self.groups)?;
        // Bookmarks that jump through this one must stay within the depth limit too.
        check_all_chains(&bookmarks, &self.groups)?;
        self.commit(self.groups.clone(), bookmarks)?;
        Ok(SaveOutcome {
            bookmark: self
                .get(&candidate.id)
                .unwrap_or_else(|| to_dto(&candidate)),
            forget_password,
        })
    }

    /// Delete a bookmark; jumps pointing to it are cleared.
    pub fn delete(&mut self, id: &str, now_ms: i64) -> Result<DeleteOutcome, AppError> {
        self.ensure_writable()?;
        let target = self
            .bookmarks
            .iter()
            .find(|b| b.id == id)
            .ok_or_else(|| not_found(EntityKind::Bookmark, id))?;
        let had_password = matches!(
            target.auth,
            StoredAuth::Password {
                password_saved: true,
                ..
            }
        );
        let forgotten_dependents = self.dependents_with_password(id);
        let mut bookmarks: Vec<StoredBookmark> = self
            .bookmarks
            .iter()
            .filter(|b| b.id != id)
            .cloned()
            .collect();
        for b in &mut bookmarks {
            if matches!(&b.proxy_jump, Some(JumpRef::Bookmark { id: j }) if j == id) {
                b.proxy_jump = None;
                b.updated_at_ms = now_ms;
                // The jump host is binding-relevant (§7.2): forget a saved password of b too.
                if let StoredAuth::Password {
                    password_saved,
                    password_binding,
                } = &mut b.auth
                {
                    *password_saved = false;
                    *password_binding = None;
                }
            }
        }
        self.commit(self.groups.clone(), bookmarks)?;
        Ok(DeleteOutcome {
            had_password,
            forgotten_dependents,
        })
    }

    /// Ids of bookmarks whose saved password would be forgotten by deleting `id` (they jump
    /// via it). The caller deletes their keychain entries too.
    pub fn dependents_with_password(&self, id: &str) -> Vec<String> {
        self.bookmarks
            .iter()
            .filter(|b| matches!(&b.proxy_jump, Some(JumpRef::Bookmark { id: j }) if j == id))
            .filter(|b| {
                matches!(
                    b.auth,
                    StoredAuth::Password {
                        password_saved: true,
                        ..
                    }
                )
            })
            .map(|b| b.id.clone())
            .collect()
    }

    /// "Copy to my bookmarks" for an ssh_config alias: `host = alias`, `useSshConfig`, `auth = auto`.
    pub fn copy_from_ssh_config(&mut self, alias: &str, now_ms: i64) -> Result<Bookmark, AppError> {
        self.ensure_writable()?;
        if !valid_hostname(alias) {
            return Err(AppError::new(ErrorCode::InvalidHostname {
                value: alias.to_owned(),
            }));
        }
        let taken = |name: &str| {
            self.bookmarks
                .iter()
                .any(|b| b.name.to_lowercase() == name.to_lowercase())
        };
        let base: String = alias.chars().take(MAX_NAME_CHARS).collect();
        let mut name = base.clone();
        let mut n = 2;
        while taken(&name) {
            if n > MAX_NAME_SUFFIX {
                return Err(invalid("name"));
            }
            let suffix = format!(" ({n})");
            let keep = MAX_NAME_CHARS - suffix.chars().count();
            name = format!("{}{suffix}", base.chars().take(keep).collect::<String>());
            n += 1;
        }
        let candidate = StoredBookmark {
            id: new_id(),
            name,
            host: alias.to_owned(),
            port: None,
            user: None,
            auth: StoredAuth::Auto,
            proxy_jump: None,
            group_id: None,
            tags: Vec::new(),
            start_dir: None,
            use_ssh_config: true,
            color: None,
            order: i32::MAX,
            upload_limit_kibps: None,
            download_limit_kibps: None,
            confirm_connect: false,
            startup_command: None,
            notes: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        };
        let mut bookmarks = self.bookmarks.clone();
        bookmarks.push(candidate.clone());
        validate_bookmark(&candidate, &bookmarks, &self.groups)?;
        self.commit(self.groups.clone(), bookmarks)?;
        Ok(self
            .get(&candidate.id)
            .unwrap_or_else(|| to_dto(&candidate)))
    }

    /// Record that a password was saved (`Some(binding)`) or removed (`None`) in the keychain.
    /// Only `password` auth can have a saved password (`invalidArgument{field:"auth"}` otherwise).
    pub fn set_password_saved(
        &mut self,
        id: &str,
        binding: Option<PasswordBinding>,
        now_ms: i64,
    ) -> Result<Bookmark, AppError> {
        self.ensure_writable()?;
        let mut bookmarks = self.bookmarks.clone();
        let b = bookmarks
            .iter_mut()
            .find(|b| b.id == id)
            .ok_or_else(|| not_found(EntityKind::Bookmark, id))?;
        let StoredAuth::Password {
            password_saved,
            password_binding,
        } = &mut b.auth
        else {
            return Err(invalid("auth"));
        };
        *password_saved = binding.is_some();
        *password_binding = binding;
        b.updated_at_ms = now_ms;
        let out = to_dto(b);
        self.commit(self.groups.clone(), bookmarks)?;
        Ok(out)
    }

    pub fn group_save(&mut self, input: GroupInput) -> Result<Group, AppError> {
        self.ensure_writable()?;
        let mut groups = self.groups.clone();
        let candidate = match &input.id {
            Some(id) => {
                let slot = groups
                    .iter_mut()
                    .find(|g| &g.id == id)
                    .ok_or_else(|| not_found(EntityKind::Group, id))?;
                slot.name = input.name.trim().to_owned();
                slot.clone()
            }
            None => {
                let g = StoredGroup {
                    id: new_id(),
                    name: input.name.trim().to_owned(),
                    order: i32::MAX,
                };
                groups.push(g.clone());
                g
            }
        };
        validate_group(&candidate, &groups)?;
        self.commit(groups, self.bookmarks.clone())?;
        let saved = self.groups.iter().find(|g| g.id == candidate.id);
        Ok(group_dto(saved.unwrap_or(&candidate)))
    }

    /// Move a bookmark into `group_id` (`None` = ungrouped) at `index` (clamped) in one write.
    pub fn bookmark_move(
        &mut self,
        id: &str,
        group_id: Option<String>,
        index: u32,
        now_ms: i64,
    ) -> Result<BookmarksDto, AppError> {
        self.ensure_writable()?;
        if !self.bookmarks.iter().any(|b| b.id == id) {
            return Err(not_found(EntityKind::Bookmark, id));
        }
        if let Some(g) = &group_id
            && !self.groups.iter().any(|x| &x.id == g)
        {
            return Err(not_found(EntityKind::Group, g));
        }
        let mut bookmarks = self.bookmarks.clone();
        let mut members: Vec<&StoredBookmark> = bookmarks
            .iter()
            .filter(|b| b.group_id == group_id)
            .collect();
        members.sort_by_key(|b| b.order);
        let ids = insert_at(members.iter().map(|b| b.id.clone()).collect(), id, index);
        for b in &mut bookmarks {
            if b.id == id && b.group_id != group_id {
                b.group_id.clone_from(&group_id);
                b.updated_at_ms = now_ms;
            }
            if b.group_id == group_id
                && let Some(pos) = ids.iter().position(|i| *i == b.id)
            {
                b.order = i32::try_from(pos).unwrap_or(i32::MAX);
            }
        }
        // The old group's list closes its gap in `commit`.
        self.commit(self.groups.clone(), bookmarks)?;
        Ok(self.dto())
    }

    /// Move a group to `index` (clamped) among the groups in one write.
    pub fn group_move(&mut self, id: &str, index: u32) -> Result<BookmarksDto, AppError> {
        self.ensure_writable()?;
        if !self.groups.iter().any(|g| g.id == id) {
            return Err(not_found(EntityKind::Group, id));
        }
        let mut groups = self.groups.clone();
        groups.sort_by_key(|g| g.order);
        let ids = insert_at(groups.iter().map(|g| g.id.clone()).collect(), id, index);
        for g in &mut groups {
            if let Some(pos) = ids.iter().position(|i| *i == g.id) {
                g.order = i32::try_from(pos).unwrap_or(i32::MAX);
            }
        }
        self.commit(groups, self.bookmarks.clone())?;
        Ok(self.dto())
    }

    /// Delete a group; its bookmarks become ungrouped.
    pub fn group_delete(&mut self, id: &str, now_ms: i64) -> Result<(), AppError> {
        self.ensure_writable()?;
        if !self.groups.iter().any(|g| g.id == id) {
            return Err(not_found(EntityKind::Group, id));
        }
        let groups: Vec<StoredGroup> = self.groups.iter().filter(|g| g.id != id).cloned().collect();
        let mut bookmarks = self.bookmarks.clone();
        // The group's bookmarks follow the ungrouped ones, in their old relative order.
        let ungrouped = bookmarks.iter().filter(|b| b.group_id.is_none()).count();
        let base = i32::try_from(ungrouped).unwrap_or(i32::MAX);
        for b in &mut bookmarks {
            if b.group_id.as_deref() == Some(id) {
                b.group_id = None;
                b.order = base.saturating_add(b.order);
                b.updated_at_ms = now_ms;
            }
        }
        self.commit(groups, bookmarks)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::fs;

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
            use_ssh_config: true,
            color: None,
            upload_limit_kibps: None,
            download_limit_kibps: None,
            confirm_connect: None,
            startup_command: None,
            notes: None,
        }
    }

    fn edit(b: &Bookmark) -> BookmarkInput {
        BookmarkInput {
            id: Some(b.id.clone()),
            name: b.name.clone(),
            host: b.host.clone(),
            port: b.port,
            user: b.user.clone(),
            auth: match &b.auth {
                AuthConfig::Auto => AuthInput::Auto,
                AuthConfig::Key { identity_file } => AuthInput::Key {
                    identity_file: identity_file.clone(),
                },
                AuthConfig::Password { .. } => AuthInput::Password,
                AuthConfig::Agent => AuthInput::Agent,
            },
            proxy_jump: b.proxy_jump.clone(),
            group_id: b.group_id.clone(),
            tags: b.tags.clone(),
            start_dir: b.start_dir.clone(),
            use_ssh_config: b.use_ssh_config,
            color: b.color,
            upload_limit_kibps: b.upload_limit_kibps,
            download_limit_kibps: b.download_limit_kibps,
            confirm_connect: Some(b.confirm_connect),
            startup_command: b.startup_command.clone(),
            notes: b.notes.clone(),
        }
    }

    /// The new fields persist, validate (hostile input) and normalise.
    #[test]
    fn v3_fields_persist_validate_and_normalise() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = BookmarkStore::load(dir.path());
        let mut i = input("web", "web.example.com");
        i.confirm_connect = Some(true);
        i.startup_command = Some(SensitiveText::new(" cd /srv/app && tmux attach || tmux"));
        i.notes = Some("line 1\r\nline 2\twith tab\n".into());
        let b = s.save(i, 1).unwrap().bookmark;
        assert!(b.confirm_connect);
        assert_eq!(
            b.startup_command.as_ref().unwrap().expose(),
            " cd /srv/app && tmux attach || tmux",
            "kept verbatim (leading space matters to the shell)"
        );
        assert_eq!(b.notes.as_deref(), Some("line 1\r\nline 2\twith tab\n"));
        let (reloaded, w) = BookmarkStore::load(dir.path());
        assert!(w.is_empty(), "{w:?}");
        let r = reloaded.get(&b.id).unwrap();
        assert!(r.confirm_connect);
        assert_eq!(r.startup_command, b.startup_command);
        assert_eq!(r.notes, b.notes);
        // Debug output never contains the command.
        assert!(!format!("{r:?}").contains("tmux"));

        // Blank values become none.
        let mut e = edit(&b);
        e.startup_command = Some(SensitiveText::new("  \u{3000} "));
        e.notes = Some(" \n\t ".into());
        e.confirm_connect = None;
        let b2 = s.save(e, 2).unwrap().bookmark;
        assert_eq!(b2.startup_command, None);
        assert_eq!(b2.notes, None);
        assert!(!b2.confirm_connect);

        // Hostile commands: anything that could submit or alter a line in the shell.
        let long = "a".repeat(MAX_STARTUP_COMMAND_CHARS + 1);
        for bad in [
            "ls\nrm -rf ~",
            "ls\r",
            "ls\u{0}",
            "ls\tx",
            "\u{1b}[200~paste",
            "\u{1b}]0;title\u{7}",
            "ls\u{7f}",
            "ls\u{85}",
            "ls\u{9b}31m",
            "ls\u{2028}rm",
            "ls\u{2029}rm",
            long.as_str(),
        ] {
            let mut e = edit(&b);
            e.startup_command = Some(SensitiveText::new(bad));
            assert_eq!(
                code(&s.save(e, 3).unwrap_err()),
                ErrorCode::InvalidArgument {
                    field: "startupCommand".into()
                },
                "{bad:?}"
            );
        }
        let mut e = edit(&b);
        e.startup_command = Some(SensitiveText::new("é".repeat(MAX_STARTUP_COMMAND_CHARS)));
        s.save(e, 3).unwrap();

        let long_notes = "n".repeat(MAX_NOTES_CHARS + 1);
        for bad in [
            "a\u{0}b",
            "\u{1b}[31m",
            "a\u{7f}",
            "a\u{85}",
            long_notes.as_str(),
        ] {
            let mut e = edit(&b);
            e.notes = Some(bad.into());
            assert_eq!(
                code(&s.save(e, 4).unwrap_err()),
                ErrorCode::InvalidArgument {
                    field: "notes".into()
                },
                "{bad:?}"
            );
        }
        let mut e = edit(&b);
        e.notes = Some("ж".repeat(MAX_NOTES_CHARS));
        s.save(e, 4).unwrap();

        // An old frontend's input (no startup command, notes or confirm flag) deserializes with the defaults.
        let old: BookmarkInput = serde_json::from_value(json!({
            "id": null, "name": "n", "host": "h", "port": null, "user": null,
            "auth": { "method": "auto" }, "proxyJump": null, "groupId": null, "tags": [],
            "startDir": null, "useSshConfig": false
        }))
        .unwrap();
        assert!(old.confirm_connect.is_none());
        assert!(old.startup_command.is_none() && old.notes.is_none());
    }

    /// A hand-edited file with bad startup command or notes values keeps the bookmark, without them.
    #[test]
    fn invalid_v3_values_on_load_become_none() {
        let dir = tempfile::tempdir().unwrap();
        let entry = |id: &str, extra: Value| {
            let mut v = json!({ "id": id, "name": id, "host": "h.example.com", "port": null,
                "user": null, "auth": { "method": "auto" }, "proxyJump": null, "groupId": null,
                "tags": [], "startDir": null, "useSshConfig": false, "order": 0,
                "createdAtMs": 1, "updatedAtMs": 1 });
            for (k, x) in extra.as_object().unwrap() {
                v[k] = x.clone();
            }
            v
        };
        let doc = json!({ "version": 2, "groups": [], "bookmarks": [
            entry("a1", json!({ "startupCommand": "ls\nrm -rf /", "notes": "ok", "confirmConnect": true })),
            entry("a2", json!({ "startupCommand": 42, "notes": "bad\u{1b}", "confirmConnect": "yes" })),
            entry("a3", json!({ "startupCommand": "uptime", "notes": "x".repeat(MAX_NOTES_CHARS + 1) })),
        ]});
        fs::write(
            dir.path().join("bookmarks.json"),
            serde_json::to_vec(&doc).unwrap(),
        )
        .unwrap();
        let (s, w) = BookmarkStore::load(dir.path());
        assert!(w.is_empty(), "{w:?}");
        let a1 = s.get("a1").unwrap();
        assert_eq!(a1.startup_command, None);
        assert_eq!(a1.notes.as_deref(), Some("ok"));
        assert!(a1.confirm_connect);
        let a2 = s.get("a2").unwrap();
        assert_eq!((a2.startup_command.clone(), a2.notes.clone()), (None, None));
        assert!(!a2.confirm_connect);
        let a3 = s.get("a3").unwrap();
        assert_eq!(a3.startup_command.as_ref().unwrap().expose(), "uptime");
        assert_eq!(a3.notes, None);
    }

    fn code(e: &AppError) -> ErrorCode {
        e.kind.clone()
    }

    fn binding() -> PasswordBinding {
        PasswordBinding {
            hostname: "nas.example.com".into(),
            port: 2222,
            user: "alice".into(),
        }
    }

    #[test]
    fn create_update_persist_reload() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, w) = BookmarkStore::load(dir.path());
        assert!(w.is_empty());
        let g = s
            .group_save(GroupInput {
                id: None,
                name: "Home".into(),
                order: 0,
            })
            .unwrap();
        let mut i = input(" nas ", "nas.example.com");
        i.port = Some(2222);
        i.user = Some("alice".into());
        i.group_id = Some(g.id.clone());
        i.tags = vec!["backup".into()];
        i.start_dir = Some("/volume1/backups".into());
        i.proxy_jump = Some(JumpRef::SshConfig {
            alias: "bastion".into(),
        });
        let b = s.save(i, 100).unwrap().bookmark;
        assert_eq!(b.name, "nas");
        assert_eq!((b.created_at_ms, b.updated_at_ms), (100, 100));
        let mut e = edit(&b);
        e.tags = vec!["nas".into()];
        let b2 = s.save(e, 200).unwrap().bookmark;
        assert_eq!((b2.created_at_ms, b2.updated_at_ms), (100, 200));

        let (s2, w2) = BookmarkStore::load(dir.path());
        assert!(w2.is_empty(), "{w2:?}");
        assert_eq!(
            serde_json::to_value(s2.dto().bookmarks).unwrap(),
            serde_json::to_value(vec![b2]).unwrap()
        );
        assert_eq!(s2.dto().groups, vec![g]);
        let disk: Value =
            serde_json::from_slice(&fs::read(dir.path().join("bookmarks.json")).unwrap()).unwrap();
        assert_eq!(disk["version"], 2);
    }

    #[test]
    fn validation_errors_use_dto_field_paths() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = BookmarkStore::load(dir.path());
        s.save(input("a", "a.example.com"), 1).unwrap();
        let cases: Vec<(BookmarkInput, ErrorCode)> = vec![
            (
                input("", "h"),
                ErrorCode::InvalidArgument {
                    field: "name".into(),
                },
            ),
            (
                input(&"n".repeat(101), "h"),
                ErrorCode::InvalidArgument {
                    field: "name".into(),
                },
            ),
            (
                input("A", "h"),
                ErrorCode::InvalidArgument {
                    field: "name".into(),
                },
            ),
            (
                input("b", "bad host"),
                ErrorCode::InvalidHostname {
                    value: "bad host".into(),
                },
            ),
            (
                input("b", "h/../x"),
                ErrorCode::InvalidHostname {
                    value: "h/../x".into(),
                },
            ),
            (
                {
                    let mut i = input("b", "h");
                    i.port = Some(0);
                    i
                },
                ErrorCode::InvalidArgument {
                    field: "port".into(),
                },
            ),
            (
                {
                    let mut i = input("b", "h");
                    i.user = Some("a b".into());
                    i
                },
                ErrorCode::InvalidArgument {
                    field: "user".into(),
                },
            ),
            (
                {
                    let mut i = input("b", "h");
                    i.auth = AuthInput::Key {
                        identity_file: " ".into(),
                    };
                    i
                },
                ErrorCode::InvalidArgument {
                    field: "auth.identityFile".into(),
                },
            ),
            (
                {
                    let mut i = input("b", "h");
                    i.group_id = Some("nope".into());
                    i
                },
                ErrorCode::InvalidArgument {
                    field: "groupId".into(),
                },
            ),
            (
                {
                    let mut i = input("b", "h");
                    i.proxy_jump = Some(JumpRef::Spec {
                        spec: "x,,y".into(),
                    });
                    i
                },
                ErrorCode::InvalidArgument {
                    field: "proxyJump".into(),
                },
            ),
            (
                {
                    let mut i = input("b", "h");
                    i.proxy_jump = Some(JumpRef::Bookmark {
                        id: "missing".into(),
                    });
                    i
                },
                ErrorCode::InvalidArgument {
                    field: "proxyJump".into(),
                },
            ),
            (
                {
                    let mut i = input("b", "h");
                    i.proxy_jump = Some(JumpRef::SshConfig { alias: "a*".into() });
                    i
                },
                ErrorCode::InvalidArgument {
                    field: "proxyJump".into(),
                },
            ),
            (
                {
                    let mut i = input("b", "h");
                    i.tags = vec![String::new()];
                    i
                },
                ErrorCode::InvalidArgument {
                    field: "tags".into(),
                },
            ),
            (
                {
                    let mut i = input("b", "h");
                    i.tags = vec!["t".into(); 21];
                    i
                },
                ErrorCode::InvalidArgument {
                    field: "tags".into(),
                },
            ),
            (
                {
                    let mut i = input("b", "h");
                    i.start_dir = Some("/a\0b".into());
                    i
                },
                ErrorCode::InvalidArgument {
                    field: "startDir".into(),
                },
            ),
        ];
        for (i, expected) in cases {
            assert_eq!(code(&s.save(i, 2).unwrap_err()), expected);
        }
        assert_eq!(
            s.dto().bookmarks.len(),
            1,
            "failed saves must not change state"
        );
        let mut ok = input("b", "h");
        ok.proxy_jump = Some(JumpRef::Spec {
            spec: "alice@jump1:2222,[2001:db8::1]".into(),
        });
        s.save(ok, 3).unwrap();
        assert_eq!(
            code(
                &s.save(
                    BookmarkInput {
                        id: Some("missing".into()),
                        ..input("z", "h")
                    },
                    4
                )
                .unwrap_err()
            ),
            ErrorCode::NotFound {
                what: EntityKind::Bookmark,
                id: "missing".into()
            }
        );
    }

    /// Per-bookmark bandwidth overrides persist, reload, and are range-checked.
    #[test]
    fn transfer_limit_overrides_persist_and_validate() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = BookmarkStore::load(dir.path());
        let mut i = input("slow", "slow.example.com");
        i.upload_limit_kibps = Some(256);
        i.download_limit_kibps = Some(0);
        let saved = s.save(i, 1).unwrap().bookmark;
        assert_eq!(saved.upload_limit_kibps, Some(256));
        assert_eq!(saved.download_limit_kibps, Some(0));
        let (reloaded, w) = BookmarkStore::load(dir.path());
        assert!(w.is_empty(), "{w:?}");
        let b = reloaded.dto().bookmarks.into_iter().next().unwrap();
        assert_eq!(
            (b.upload_limit_kibps, b.download_limit_kibps),
            (Some(256), Some(0))
        );
        // Clearing the override (`null`) goes back to the global setting.
        let mut cleared = edit(&b);
        cleared.upload_limit_kibps = None;
        let b = s.save(cleared, 2).unwrap().bookmark;
        assert_eq!(b.upload_limit_kibps, None);
        for (field, set) in [
            (
                "uploadLimitKiBps",
                (|i: &mut BookmarkInput| i.upload_limit_kibps = Some(1_048_577))
                    as fn(&mut BookmarkInput),
            ),
            ("downloadLimitKiBps", |i: &mut BookmarkInput| {
                i.download_limit_kibps = Some(u32::MAX);
            }),
        ] {
            let mut bad = input("bad", "bad.example.com");
            set(&mut bad);
            let e = s.save(bad, 3).unwrap_err();
            assert_eq!(
                code(&e),
                ErrorCode::InvalidArgument {
                    field: field.into()
                }
            );
        }
        // An old file without the fields still loads.
        let raw = std::fs::read_to_string(dir.path().join("bookmarks.json")).unwrap();
        assert!(!raw.contains("downloadLimitKiBps\": null"), "{raw}");
    }

    #[test]
    fn jump_self_and_cycles_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = BookmarkStore::load(dir.path());
        let a = s.save(input("a", "a"), 1).unwrap().bookmark;
        let mut bi = input("b", "b");
        bi.proxy_jump = Some(JumpRef::Bookmark { id: a.id.clone() });
        let b = s.save(bi, 1).unwrap().bookmark;
        let mut self_ref = edit(&a);
        self_ref.proxy_jump = Some(JumpRef::Bookmark { id: a.id.clone() });
        assert_eq!(
            code(&s.save(self_ref, 2).unwrap_err()),
            ErrorCode::InvalidArgument {
                field: "proxyJump".into()
            }
        );
        let mut cyc = edit(&a);
        cyc.proxy_jump = Some(JumpRef::Bookmark { id: b.id.clone() });
        assert_eq!(
            code(&s.save(cyc, 2).unwrap_err()),
            ErrorCode::InvalidArgument {
                field: "proxyJump".into()
            }
        );
    }

    /// A bookmark→bookmark chain may have at most `MAX_JUMPS` (8) jumps; a save that
    /// would make another bookmark's chain longer is refused too.
    #[test]
    fn jump_chain_depth_is_capped() {
        assert_eq!(MAX_BOOKMARK_JUMPS, crate::ssh::target::MAX_JUMPS);
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = BookmarkStore::load(dir.path());
        let mut chain = vec![s.save(input("b0", "h0"), 1).unwrap().bookmark];
        for n in 1..=MAX_BOOKMARK_JUMPS {
            let mut i = input(&format!("b{n}"), &format!("h{n}"));
            i.proxy_jump = Some(JumpRef::Bookmark {
                id: chain.last().unwrap().id.clone(),
            });
            chain.push(s.save(i, 1).unwrap().bookmark);
        }
        // b8 jumps 8 times: allowed. A ninth jump is not.
        let mut too_deep = input("b9", "h9");
        too_deep.proxy_jump = Some(JumpRef::Bookmark {
            id: chain.last().unwrap().id.clone(),
        });
        assert_eq!(
            code(&s.save(too_deep, 2).unwrap_err()),
            ErrorCode::InvalidArgument {
                field: "proxyJump".into()
            }
        );
        // Making the chain's root jump somewhere would push b8 to 9 jumps.
        let x = s.save(input("x", "x"), 2).unwrap().bookmark;
        let mut root = edit(&chain[0]);
        root.proxy_jump = Some(JumpRef::Bookmark { id: x.id });
        assert_eq!(
            code(&s.save(root, 3).unwrap_err()),
            ErrorCode::InvalidArgument {
                field: "proxyJump".into()
            }
        );
        assert_eq!(s.dto().bookmarks.len(), MAX_BOOKMARK_JUMPS + 2);
    }

    fn chain_file(n: usize, tail: Option<&str>) -> Vec<u8> {
        let bookmarks: Vec<Value> = (0..n)
            .map(|i| {
                let jump = match (i + 1 < n, tail) {
                    (true, _) => json!({"kind": "bookmark", "id": format!("b{}", i + 1)}),
                    (false, Some(t)) => json!({"kind": "bookmark", "id": t}),
                    (false, None) => Value::Null,
                };
                json!({ "id": format!("b{i}"), "name": format!("b{i}"),
                        "host": format!("h{i}.example.com"), "port": null, "user": null,
                        "auth": { "method": "auto" }, "proxyJump": jump, "groupId": null,
                        "tags": [], "startDir": null, "useSshConfig": false,
                        "createdAtMs": 1, "updatedAtMs": 1 })
            })
            .collect();
        serde_json::to_vec(&json!({"version": 1, "groups": [], "bookmarks": bookmarks})).unwrap()
    }

    /// (load side): a hand-edited file with a 5000-long chain whose tail is missing is
    /// cleaned up in bounded time (a few O(n) passes), and only chains within the depth limit
    /// survive.
    #[test]
    fn hostile_chains_on_load_are_bounded() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("bookmarks.json"),
            chain_file(5000, Some("missing")),
        )
        .unwrap();
        let started = std::time::Instant::now();
        let (s, w) = BookmarkStore::load(dir.path());
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        assert!(s.dto().bookmarks.is_empty());
        assert!(!w.is_empty());

        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("bookmarks.json"), chain_file(5000, None)).unwrap();
        let (s, _) = BookmarkStore::load(dir.path());
        // b4991 … b4999: at most 8 jumps each.
        assert_eq!(s.dto().bookmarks.len(), MAX_BOOKMARK_JUMPS + 1);
    }

    #[test]
    fn password_binding_cleared_on_binding_relevant_edit() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = BookmarkStore::load(dir.path());
        let mut i = input("nas", "nas.example.com");
        i.auth = AuthInput::Password;
        i.port = Some(2222);
        i.user = Some("alice".into());
        let b = s.save(i, 1).unwrap().bookmark;
        assert_eq!(
            b.auth,
            AuthConfig::Password {
                password_saved: false
            }
        );
        let b = s.set_password_saved(&b.id, Some(binding()), 2).unwrap();
        assert_eq!(
            b.auth,
            AuthConfig::Password {
                password_saved: true
            }
        );
        assert_eq!(s.password_binding(&b.id), Some(binding()));

        // Non-relevant edit keeps the password.
        let mut e = edit(&b);
        e.name = "nas2".into();
        e.tags = vec!["x".into()];
        let out = s.save(e, 3).unwrap();
        assert!(!out.forget_password);
        assert_eq!(
            out.bookmark.auth,
            AuthConfig::Password {
                password_saved: true
            }
        );
        assert_eq!(s.password_binding(&b.id), Some(binding()));

        // Each binding-relevant field (and leaving password auth) forgets it.
        type Mutate = fn(&mut BookmarkInput);
        let mutations: [Mutate; 6] = [
            |e| e.host = "other.example.com".into(),
            |e| e.port = Some(22),
            |e| e.user = Some("bob".into()),
            |e| e.use_ssh_config = false,
            |e| {
                e.proxy_jump = Some(JumpRef::Spec {
                    spec: "jump".into(),
                })
            },
            |e| e.auth = AuthInput::Agent,
        ];
        for (n, m) in mutations.into_iter().enumerate() {
            let mut i = input(&format!("pw{n}"), "nas.example.com");
            i.auth = AuthInput::Password;
            i.port = Some(2222);
            i.user = Some("alice".into());
            let fresh = s.save(i, 1).unwrap().bookmark;
            let fresh = s.set_password_saved(&fresh.id, Some(binding()), 2).unwrap();
            let mut e = edit(&fresh);
            m(&mut e);
            let out = s.save(e, 5).unwrap();
            assert!(out.forget_password, "mutation {n}");
            assert_eq!(s.password_binding(&fresh.id), None);
            assert_ne!(
                out.bookmark.auth,
                AuthConfig::Password {
                    password_saved: true
                }
            );
            // Reload: nothing saved for it.
            let (s2, _) = BookmarkStore::load(dir.path());
            assert_eq!(s2.password_binding(&fresh.id), None);
        }
    }

    #[test]
    fn set_password_requires_password_auth() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = BookmarkStore::load(dir.path());
        let b = s.save(input("a", "a"), 1).unwrap().bookmark;
        assert_eq!(
            code(&s.set_password_saved(&b.id, Some(binding()), 2).unwrap_err()),
            ErrorCode::InvalidArgument {
                field: "auth".into()
            }
        );
    }

    #[test]
    fn delete_clears_dependent_jumps_and_reports_password() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = BookmarkStore::load(dir.path());
        let mut ji = input("jump", "jump.example.com");
        ji.auth = AuthInput::Password;
        let j = s.save(ji, 1).unwrap().bookmark;
        s.set_password_saved(&j.id, Some(binding()), 1).unwrap();
        let mut ti = input("target", "t.example.com");
        ti.auth = AuthInput::Password;
        ti.proxy_jump = Some(JumpRef::Bookmark { id: j.id.clone() });
        let t = s.save(ti, 1).unwrap().bookmark;
        s.set_password_saved(&t.id, Some(binding()), 1).unwrap();
        assert_eq!(s.dependents_with_password(&j.id), vec![t.id.clone()]);
        let out = s.delete(&j.id, 9).unwrap();
        assert!(out.had_password);
        assert_eq!(out.forgotten_dependents, vec![t.id.clone()]);
        let t = s.get(&t.id).unwrap();
        assert_eq!(t.proxy_jump, None);
        assert_eq!(
            t.auth,
            AuthConfig::Password {
                password_saved: false
            }
        );
        assert_eq!(
            code(&s.delete(&j.id, 9).unwrap_err()),
            ErrorCode::NotFound {
                what: EntityKind::Bookmark,
                id: j.id.clone()
            }
        );
    }

    #[test]
    fn empty_group_deletes_and_nonempty_keeps_bookmarks() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = BookmarkStore::load(dir.path());
        let mk = |s: &mut BookmarkStore, n: &str| {
            s.group_save(GroupInput {
                id: None,
                name: n.into(),
                order: 0,
            })
            .unwrap()
        };
        let empty = mk(&mut s, "empty");
        let full = mk(&mut s, "full");
        let mut i = input("a", "a");
        i.group_id = Some(full.id.clone());
        let b = s.save(i, 1).unwrap().bookmark;
        s.group_delete(&empty.id, 2).unwrap();
        assert_eq!(s.dto().groups.len(), 1);
        s.group_delete(&full.id, 3).unwrap();
        assert!(s.dto().groups.is_empty());
        assert_eq!(s.get(&b.id).unwrap().group_id, None);
        assert_eq!(s.dto().bookmarks.len(), 1);
        // Persisted.
        let (s2, _) = BookmarkStore::load(dir.path());
        assert!(s2.dto().groups.is_empty());
        assert_eq!(s2.dto().bookmarks.len(), 1);
    }

    #[test]
    fn groups_crud() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = BookmarkStore::load(dir.path());
        let g = s
            .group_save(GroupInput {
                id: None,
                name: "Work".into(),
                order: 1,
            })
            .unwrap();
        assert_eq!(
            code(
                &s.group_save(GroupInput {
                    id: None,
                    name: "work".into(),
                    order: 2
                })
                .unwrap_err()
            ),
            ErrorCode::InvalidArgument {
                field: "name".into()
            }
        );
        let g = s
            .group_save(GroupInput {
                id: Some(g.id.clone()),
                name: "Office".into(),
                order: 3,
            })
            .unwrap();
        // `GroupInput.order` is ignored since v2: the group keeps its place (the only one: 0).
        assert_eq!((g.name.as_str(), g.order), ("Office", 0));
        let mut i = input("a", "a");
        i.group_id = Some(g.id.clone());
        let b = s.save(i, 1).unwrap().bookmark;
        s.group_delete(&g.id, 5).unwrap();
        assert_eq!(s.get(&b.id).unwrap().group_id, None);
        assert!(s.dto().groups.is_empty());
        assert_eq!(
            code(&s.group_delete(&g.id, 5).unwrap_err()),
            ErrorCode::NotFound {
                what: EntityKind::Group,
                id: g.id.clone()
            }
        );
    }

    fn names_of(s: &BookmarkStore, group: Option<&str>) -> Vec<String> {
        let mut v: Vec<_> = s
            .dto()
            .bookmarks
            .into_iter()
            .filter(|b| b.group_id.as_deref() == group)
            .collect();
        v.sort_by_key(|b| b.order);
        assert!(
            v.iter()
                .enumerate()
                .all(|(i, b)| usize::try_from(b.order) == Ok(i)),
            "orders are dense"
        );
        v.into_iter().map(|b| b.name).collect()
    }

    fn group(s: &mut BookmarkStore, name: &str) -> Group {
        s.group_save(GroupInput {
            id: None,
            name: name.into(),
            order: 99,
        })
        .unwrap()
    }

    #[test]
    fn color_persists_validates_and_unknown_values_load_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = BookmarkStore::load(dir.path());
        let mut i = input("a", "a");
        i.color = Some(BookmarkColor::Purple);
        let b = s.save(i, 1).unwrap().bookmark;
        assert_eq!(b.color, Some(BookmarkColor::Purple));
        let mut e = edit(&b);
        e.color = None;
        assert_eq!(s.save(e, 2).unwrap().bookmark.color, None);
        let mut e = edit(&b);
        e.color = Some(BookmarkColor::Gray);
        s.save(e, 3).unwrap();
        let (s2, w) = BookmarkStore::load(dir.path());
        assert!(w.is_empty());
        assert_eq!(s2.get(&b.id).unwrap().color, Some(BookmarkColor::Gray));
        // Every palette name is accepted by serde; others are not.
        for c in BookmarkColor::ALL {
            let v = serde_json::to_value(c).unwrap();
            assert_eq!(serde_json::from_value::<BookmarkColor>(v).unwrap(), c);
        }
        assert!(serde_json::from_value::<BookmarkColor>(json!("pink")).is_err());
        // A hand-edited / newer palette value keeps the bookmark, without color.
        let path = dir.path().join("bookmarks.json");
        let mut doc: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        doc["bookmarks"][0]["color"] = json!("pink");
        fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
        let (s3, w) = BookmarkStore::load(dir.path());
        assert!(w.is_empty());
        assert_eq!(s3.get(&b.id).unwrap().color, None);
    }

    #[test]
    fn new_bookmarks_append_and_group_change_goes_to_the_end() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = BookmarkStore::load(dir.path());
        let g = group(&mut s, "G");
        let a = s.save(input("a", "a"), 1).unwrap().bookmark;
        s.save(input("b", "b"), 1).unwrap();
        let mut ig = input("c", "c");
        ig.group_id = Some(g.id.clone());
        s.save(ig, 1).unwrap();
        assert_eq!(a.order, 0);
        let mut e = edit(&a);
        e.group_id = Some(g.id.clone());
        let moved = s.save(e, 2).unwrap().bookmark;
        assert_eq!(moved.order, 1);
        assert_eq!(names_of(&s, None), ["b"]);
        assert_eq!(names_of(&s, Some(&g.id)), ["c", "a"]);
        // Editing in place keeps the position.
        let mut e = edit(&moved);
        e.name = "a2".into();
        assert_eq!(s.save(e, 3).unwrap().bookmark.order, 1);
        // Deleting a group appends its bookmarks to the ungrouped list.
        s.group_delete(&g.id, 4).unwrap();
        assert_eq!(names_of(&s, None), ["b", "c", "a2"]);
    }

    #[test]
    fn bookmark_move_reorders_regroups_clamps_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = BookmarkStore::load(dir.path());
        let g = group(&mut s, "G");
        let ids: Vec<String> = ["a", "b", "c", "d"]
            .iter()
            .map(|n| s.save(input(n, n), 1).unwrap().bookmark.id)
            .collect();
        // Within the ungrouped list: d to the front, a to far past the end.
        s.bookmark_move(&ids[3], None, 0, 2).unwrap();
        assert_eq!(names_of(&s, None), ["d", "a", "b", "c"]);
        let dto = s.bookmark_move(&ids[0], None, 1000, 2).unwrap();
        assert_eq!(names_of(&s, None), ["d", "b", "c", "a"]);
        assert_eq!(dto.bookmarks.len(), 4);
        // Into a group: the gap in the old list closes; a reorder alone does not touch updatedAt.
        assert_eq!(s.get(&ids[1]).unwrap().updated_at_ms, 1);
        s.bookmark_move(&ids[1], Some(g.id.clone()), 0, 7).unwrap();
        s.bookmark_move(&ids[2], Some(g.id.clone()), 0, 8).unwrap();
        assert_eq!(names_of(&s, Some(&g.id)), ["c", "b"]);
        assert_eq!(names_of(&s, None), ["d", "a"]);
        assert_eq!(s.get(&ids[1]).unwrap().updated_at_ms, 7);
        s.bookmark_move(&ids[2], Some(g.id.clone()), 1, 9).unwrap();
        assert_eq!(names_of(&s, Some(&g.id)), ["b", "c"]);
        assert_eq!(s.get(&ids[2]).unwrap().updated_at_ms, 8);
        // Out of the group again, to the middle.
        s.bookmark_move(&ids[1], None, 1, 10).unwrap();
        assert_eq!(names_of(&s, None), ["d", "b", "a"]);
        // Persisted.
        let (s2, w) = BookmarkStore::load(dir.path());
        assert!(w.is_empty());
        assert_eq!(names_of(&s2, None), ["d", "b", "a"]);
        assert_eq!(names_of(&s2, Some(&g.id)), ["c"]);
        // Validation.
        assert_eq!(
            code(&s.bookmark_move("nope", None, 0, 1).unwrap_err()),
            ErrorCode::NotFound {
                what: EntityKind::Bookmark,
                id: "nope".into()
            }
        );
        assert_eq!(
            code(
                &s.bookmark_move(&ids[0], Some("nogroup".into()), 0, 1)
                    .unwrap_err()
            ),
            ErrorCode::NotFound {
                what: EntityKind::Group,
                id: "nogroup".into()
            }
        );
    }

    #[test]
    fn group_move_reorders_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = BookmarkStore::load(dir.path());
        let a = group(&mut s, "A");
        let b = group(&mut s, "B");
        let c = group(&mut s, "C");
        let order = |s: &BookmarkStore| -> Vec<String> {
            let mut g = s.dto().groups;
            g.sort_by_key(|g| g.order);
            assert!(
                g.iter()
                    .enumerate()
                    .all(|(i, g)| usize::try_from(g.order) == Ok(i))
            );
            g.into_iter().map(|g| g.name).collect()
        };
        assert_eq!(order(&s), ["A", "B", "C"]);
        s.group_move(&c.id, 0).unwrap();
        assert_eq!(order(&s), ["C", "A", "B"]);
        s.group_move(&c.id, 99).unwrap();
        assert_eq!(order(&s), ["A", "B", "C"]);
        s.group_move(&a.id, 1).unwrap();
        assert_eq!(order(&s), ["B", "A", "C"]);
        let (s2, _) = BookmarkStore::load(dir.path());
        assert_eq!(order(&s2), ["B", "A", "C"]);
        assert_eq!(
            code(&s.group_move("nope", 0).unwrap_err()),
            ErrorCode::NotFound {
                what: EntityKind::Group,
                id: "nope".into()
            }
        );
        let _ = b;
    }

    #[test]
    fn v1_file_migrates_keeping_the_displayed_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bookmarks.json");
        let bm = |id: &str, name: &str, group: Value| {
            json!({"id": id, "name": name, "host": "h.example", "port": null, "user": null,
                "auth": {"method": "auto"}, "proxyJump": null, "groupId": group, "tags": [],
                "startDir": null, "useSshConfig": false, "createdAtMs": 1, "updatedAtMs": 1})
        };
        let v1 = json!({"version": 1,
            "groups": [{"id": "g2", "name": "Two", "order": 5}, {"id": "g1", "name": "One", "order": 2}],
            "bookmarks": [bm("b1", "zulu", json!("g1")), bm("b2", "Bravo", Value::Null),
                          bm("b3", "alpha", Value::Null), bm("b4", "alpha2", json!("g1"))]});
        fs::write(&path, serde_json::to_vec(&v1).unwrap()).unwrap();
        let (s, w) = BookmarkStore::load(dir.path());
        assert!(w.is_empty(), "{w:?}");
        assert!(!s.read_only());
        assert_eq!(names_of(&s, None), ["alpha", "Bravo"]);
        assert_eq!(names_of(&s, Some("g1")), ["alpha2", "zulu"]);
        let groups = s.dto().groups;
        assert_eq!(
            groups.iter().map(|g| g.id.as_str()).collect::<Vec<_>>(),
            ["g1", "g2"]
        );
        // The file was rewritten as v2 and the v1 document kept as a backup.
        let disk: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(disk["version"], 2);
        let bak: Value =
            serde_json::from_slice(&fs::read(dir.path().join("bookmarks.v1.bak.json")).unwrap())
                .unwrap();
        assert_eq!(bak, v1);
    }

    #[test]
    fn copy_from_ssh_config_uniquifies_names() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = BookmarkStore::load(dir.path());
        let a = s.copy_from_ssh_config("web", 1).unwrap();
        assert_eq!(
            (a.name.as_str(), a.host.as_str(), a.use_ssh_config, &a.auth),
            ("web", "web", true, &AuthConfig::Auto)
        );
        assert_eq!(s.copy_from_ssh_config("web", 1).unwrap().name, "web (2)");
        assert_eq!(s.copy_from_ssh_config("WEB", 1).unwrap().name, "WEB (3)");
        assert!(matches!(
            code(&s.copy_from_ssh_config("a*", 1).unwrap_err()),
            ErrorCode::InvalidHostname { .. }
        ));
        let long = "h".repeat(200);
        s.copy_from_ssh_config(&long, 1).unwrap();
        let n = s.copy_from_ssh_config(&long, 1).unwrap().name;
        assert_eq!(n.chars().count(), 100);
        assert!(n.ends_with(" (2)"));
    }

    #[test]
    fn load_drops_invalid_entries_and_keeps_corrupt_copy() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bookmarks.json");
        let doc = json!({
            "version": 2,
            "groups": [ {"id": "g1", "name": "Home", "order": 0}, {"id": "g2", "name": "home", "order": 1}, 42 ],
            "bookmarks": [
                {"id": "b1", "name": "nas", "host": "nas.local", "port": 2222, "user": "alice",
                 "auth": {"method": "password", "passwordSaved": true,
                          "passwordBinding": {"hostname": "nas.local", "port": 2222, "user": "alice"}},
                 "proxyJump": null, "groupId": "g1", "tags": ["backup"], "startDir": "/v",
                 "useSshConfig": true, "createdAtMs": 1, "updatedAtMs": 1},
                {"id": "b2", "name": "evil", "host": "x/../y", "port": null, "user": null, "auth": {"method": "auto"},
                 "proxyJump": null, "groupId": null, "tags": [], "startDir": null, "useSshConfig": true, "createdAtMs": 1, "updatedAtMs": 1},
                {"id": "b3", "name": "via-evil", "host": "z", "port": null, "user": null, "auth": {"method": "auto"},
                 "proxyJump": {"kind": "bookmark", "id": "b2"}, "groupId": null, "tags": [], "startDir": null, "useSshConfig": true, "createdAtMs": 1, "updatedAtMs": 1},
                {"id": "b4", "name": "in-dropped-group", "host": "z", "port": null, "user": null, "auth": {"method": "auto"},
                 "proxyJump": null, "groupId": "g2", "tags": [], "startDir": null, "useSshConfig": true, "createdAtMs": 1, "updatedAtMs": 1},
                {"id": "b5", "name": "garbage"}
            ]
        });
        fs::write(&p, serde_json::to_vec(&doc).unwrap()).unwrap();
        let (s, w) = BookmarkStore::load(dir.path());
        let dto = s.dto();
        assert_eq!(
            dto.bookmarks
                .iter()
                .map(|b| b.id.as_str())
                .collect::<Vec<_>>(),
            vec!["b1"]
        );
        assert_eq!(dto.groups.len(), 1);
        assert_eq!(
            s.password_binding("b1"),
            Some(PasswordBinding {
                hostname: "nas.local".into(),
                port: 2222,
                user: "alice".into()
            })
        );
        assert_eq!(w.len(), 1);
        let v = serde_json::to_value(&w[0]).unwrap();
        assert_eq!(v["code"], "storageCorrupt");
        let q = dir
            .path()
            .join(v["params"]["quarantinedAs"].as_str().unwrap());
        let kept: Value = serde_json::from_slice(&fs::read(q).unwrap()).unwrap();
        assert_eq!(kept, doc, "the corrupt copy keeps every original entry");
        // The cleaned file was rewritten; a reload is warning-free.
        let (_, w2) = BookmarkStore::load(dir.path());
        assert!(w2.is_empty());
    }

    #[test]
    fn unparsable_file_is_quarantined_and_store_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("bookmarks.json"), b"\xff\xfe nope").unwrap();
        let (s, w) = BookmarkStore::load(dir.path());
        assert!(s.dto().bookmarks.is_empty());
        assert_eq!(
            serde_json::to_value(&w[0]).unwrap()["code"],
            "storageCorrupt"
        );
    }

    #[test]
    fn newer_version_is_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bookmarks.json");
        let doc = json!({"version": 3, "groups": [], "bookmarks": [
            {"id": "b1", "name": "nas", "host": "nas.local", "port": null, "user": null, "auth": {"method": "auto"},
             "proxyJump": null, "groupId": null, "tags": [], "startDir": null, "useSshConfig": true,
             "createdAtMs": 1, "updatedAtMs": 1, "futureField": 1}]});
        fs::write(&p, serde_json::to_vec(&doc).unwrap()).unwrap();
        let before = fs::read(&p).unwrap();
        let (mut s, w) = BookmarkStore::load(dir.path());
        assert!(s.dto().read_only);
        assert_eq!(s.dto().bookmarks.len(), 1);
        assert_eq!(
            serde_json::to_value(&w[0]).unwrap()["code"],
            "storageReadOnly"
        );
        assert_eq!(
            code(&s.save(input("x", "x"), 1).unwrap_err()),
            ErrorCode::StorageReadOnly
        );
        assert_eq!(
            code(&s.delete("b1", 1).unwrap_err()),
            ErrorCode::StorageReadOnly
        );
        assert_eq!(
            code(
                &s.group_save(GroupInput {
                    id: None,
                    name: "g".into(),
                    order: 0
                })
                .unwrap_err()
            ),
            ErrorCode::StorageReadOnly
        );
        assert_eq!(
            code(&s.copy_from_ssh_config("x", 1).unwrap_err()),
            ErrorCode::StorageReadOnly
        );
        assert_eq!(
            fs::read(&p).unwrap(),
            before,
            "a newer file is never rewritten"
        );
    }

    #[test]
    fn dto_never_exposes_password_binding() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _) = BookmarkStore::load(dir.path());
        let mut i = input("nas", "nas.example.com");
        i.auth = AuthInput::Password;
        let b = s.save(i, 1).unwrap().bookmark;
        s.set_password_saved(&b.id, Some(binding()), 2).unwrap();
        let v = serde_json::to_string(&s.dto()).unwrap();
        assert!(!v.contains("passwordBinding") && !v.contains("2222"), "{v}");
        let disk = fs::read_to_string(dir.path().join("bookmarks.json")).unwrap();
        assert!(disk.contains("passwordBinding"));
    }
}
