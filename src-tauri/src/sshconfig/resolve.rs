//! Per-alias resolution (§7.4) with OpenSSH semantics:
//! - files in order user config → system config; options before the first `Host` apply to all;
//! - `Host` patterns are matched case-sensitively against the alias as typed (OpenSSH); a
//!   matching negated pattern excludes the block;
//! - `Match` subset (`all`, `host` = HostName so far, `originalhost`, `user` = User so far or
//!   the local user, `localuser`), AND-combined, `!`-negatable; `host`/`originalhost` compare
//!   ASCII case-insensitively, `user`/`localuser` case-sensitively (OpenSSH, see
//!   `pattern.rs`); unsupported criteria ⇒ the block never matches (warning);
//! - `Include` inside an inactive block makes the included file never match
//!   (`SSHCONF_NEVERMATCH`); a `Host`/`Match` inside an included file does not change the
//!   including file's state;
//! - first obtained value wins; `IdentityFile`/`CertificateFile` accumulate; `ProxyJump` and
//!   `ProxyCommand` exclude each other (whichever comes first wins);
//! - expansion order (OpenSSH): `HostName`'s own `%h`/`%%` → `valid_hostname` → `HostKeyAlias`,
//!   `Port` → `User` (tokens without `%r`/`%C`, only when it comes from the config and the
//!   connection has no user of its own; validated if expansion changed it) → final user →
//!   `%C` → file-option tokens. `Match user` sees the raw `User` value.

use std::collections::HashMap;

use crate::ipc::dto::{ConfigWarning, ConfigWarningCode, SourceLocation};
use crate::ssh::shellquote::is_deceptive_char;

use super::parser::{CritKind, Item, Key, OptLine, parse_bool, parse_time, warning};
use super::pattern::{Case, ListMatch, host_block_matches, match_list};
use super::tokens::{
    ExpandError, MAX_USER_CHARS, TokenCtx, canonical_host, connection_hash, expand_hostname,
    expand_path, expand_user, valid_env_name,
};
use super::validate::{parse_host_spec, valid_hostname};
use super::{ConfigEnv, ParsedConfig};

/// One `IdentityFile` (expanded). Defaults (`id_rsa`, `id_ecdsa`, `id_ed25519`) are added by
/// `ssh::keys` with `explicit: false`; everything from the config is explicit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityFileOpt {
    pub path: String,
    pub explicit: bool,
}

/// `IdentityAgent` (§5.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentSetting {
    /// `IdentityAgent none`.
    Disabled,
    /// `SSH_AUTH_SOCK`, `$NAME` or `${NAME}`: look the endpoint up in that environment variable.
    EnvVar(String),
    /// An explicit path or pipe name (`~`, `%d`, … expanded). Validated by `ssh::agent`.
    Path(String),
}

/// `ProxyJump`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyJumpOpt {
    /// `ProxyJump none`.
    Disabled,
    /// Raw comma-separated entries (`[user@]host[:port]` or aliases), validated by `ssh::target`.
    Hops(Vec<String>),
}

/// One `UserKnownHostsFile` / `GlobalKnownHostsFile` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KnownHostsFileOpt {
    /// `none`, `/dev/null` or `NUL` (case-insensitive): neither read nor written.
    Disabled,
    Path(String),
}

/// `StrictHostKeyChecking`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrictMode {
    /// `yes` / `true`.
    Yes,
    Ask,
    AcceptNew,
    /// `no` / `off` / `false` (still hard-fails on changed keys, §7.4).
    No,
}

/// Options resolved for one alias. Defaults (HostName = alias, User = local user, Port 22) are
/// applied by `ssh::target`, not here; `None`/empty means "not set in any config".
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HostConfig {
    /// After `%h`/`%%` expansion and `valid_hostname`.
    pub hostname: Option<String>,
    pub port: Option<u16>,
    /// `User` after token expansion (§7.4). Raw when the query had its own user
    /// ([`QueryOverrides::user`]: nothing is expanded then). `None` if unset or if it could
    /// not be expanded (then [`Self::user_unresolved`] is set).
    pub user: Option<String>,
    /// The config `User` text (as written, shortened) whose expansion failed or produced an
    /// unsafe name (`invalidUser` warning). A hop without its own user refuses to connect
    /// (`ErrorCode::InvalidUser`) instead of falling back to the local user.
    pub user_unresolved: Option<String>,
    /// Empty if `IdentityFile none` appeared anywhere in the applicable blocks.
    pub identity_files: Vec<IdentityFileOpt>,
    pub identities_only: Option<bool>,
    pub identity_agent: Option<AgentSetting>,
    pub proxy_jump: Option<ProxyJumpOpt>,
    /// Set only if `ProxyCommand` won over `ProxyJump` and is not `none` (⇒ `proxyCommandUnsupported`).
    pub proxy_command: Option<String>,
    pub user_known_hosts: Option<Vec<KnownHostsFileOpt>>,
    pub global_known_hosts: Option<Vec<KnownHostsFileOpt>>,
    /// A `UserKnownHostsFile`/`GlobalKnownHostsFile` argument could not be expanded (unknown
    /// token or unset variable → `unknownToken`; a token whose value is unavailable →
    /// `unavailableToken`: `%l`/`%L`/`%C` without a usable local host name, `%h`/`%C` with
    /// `CanonicalizeHostname`, `%h`/`%C` of a non-canonical numeric IPv4 or an IPv4-compatible
    /// / -mapped IPv6, `%u`/`%r`/`%C` with
    /// an unusable or inexact user; too long → `expansionTooLong`): that option falls back to
    /// the defaults and `accept-new`/`no` must ask instead (the files the user meant are not
    /// read).
    pub known_hosts_unresolved: bool,
    pub strict_host_key_checking: Option<StrictMode>,
    /// Validated with `valid_hostname`.
    pub host_key_alias: Option<String>,
    /// `HostKeyAlgorithms +…ssh-rsa…` (a `+` list containing `ssh-rsa`); other forms are ignored.
    pub host_key_algorithms_add_ssh_rsa: bool,
    pub hash_known_hosts: Option<bool>,
    pub server_alive_interval: Option<u32>,
    pub server_alive_count_max: Option<u32>,
    pub connect_timeout: Option<u32>,
    pub preferred_authentications: Option<Vec<String>>,
    /// `unbound`/`host-bound` count as `true`.
    pub pubkey_authentication: Option<bool>,
    pub password_authentication: Option<bool>,
    /// Also set by `ChallengeResponseAuthentication`.
    pub kbd_interactive_authentication: Option<bool>,
    /// Only from a **user**-scope file (§5.5); `yes`/`true` → true, everything else
    /// (`no`, `ask`, `confirm`, a lifetime) → false. A system-file value yields `None`.
    pub add_keys_to_agent: Option<bool>,
    /// Stored, unused in v1.
    pub certificate_files: Vec<String>,
    /// Resolution warnings (`unknownToken`, `unsupportedMatchCriteria` for blocks that would
    /// otherwise apply to this host).
    pub warnings: Vec<ConfigWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QueryError {
    /// `HostName` (after expansion) or `HostKeyAlias` failed `valid_hostname`.
    #[error("invalid hostname in ssh config: {value}")]
    InvalidHostname { value: String },
}

/// Internal result with locations for snapshot warnings.
pub(crate) struct Resolution {
    pub config: HostConfig,
    pub proxy_command_loc: Option<SourceLocation>,
}

pub(crate) struct ResolveError {
    pub error: QueryError,
    pub loc: SourceLocation,
}

#[derive(Default)]
struct Collector<'a> {
    first: HashMap<Key, &'a OptLine>,
    identity_files: Vec<&'a OptLine>,
    identity_none: bool,
    certificate_files: Vec<&'a OptLine>,
    warnings: Vec<ConfigWarning>,
    /// A block skipped only because of unsupported `Match` criteria (`exec`, …) would have set
    /// an option that decides which known_hosts files are read or what `%r`/`%C` are (OpenSSH
    /// may apply it): the hop must ask instead of `accept-new`/`no` (`known_hosts_unresolved`).
    known_hosts_uncertain: bool,
}

/// Options that decide the known_hosts files or the inputs of `%h`/`%p`/`%r`/`%j`/`%k`/`%C`
/// (and the strict mode). An unsupported `Match` block setting one of them, while it is still
/// unset, makes host-key checking uncertain.
fn affects_known_hosts(key: Key) -> bool {
    matches!(
        key,
        Key::User
            | Key::HostName
            | Key::Port
            | Key::HostKeyAlias
            | Key::ProxyJump
            | Key::ProxyCommand
            | Key::UserKnownHostsFile
            | Key::GlobalKnownHostsFile
            | Key::StrictHostKeyChecking
            | Key::CanonicalizeHostname
    )
}

impl<'a> Collector<'a> {
    fn apply(&mut self, o: &'a OptLine) {
        match o.key {
            Key::IdentityFile => {
                if o.first().eq_ignore_ascii_case("none") {
                    self.identity_none = true;
                } else {
                    self.identity_files.push(o);
                }
            }
            Key::CertificateFile => self.certificate_files.push(o),
            Key::ProxyJump | Key::ProxyCommand => {
                if !self.first.contains_key(&Key::ProxyJump)
                    && !self.first.contains_key(&Key::ProxyCommand)
                {
                    self.first.insert(o.key, o);
                }
            }
            key => {
                self.first.entry(key).or_insert(o);
            }
        }
    }

    fn get(&self, key: Key) -> Option<&'a OptLine> {
        self.first.get(&key).copied()
    }

    /// `o` sits in a block Shellfin skipped only for unsupported `Match` criteria; note if
    /// OpenSSH, applying that block, could take it for a known_hosts-relevant option.
    fn note_skipped(&mut self, o: &OptLine) {
        let unset = match o.key {
            Key::ProxyJump | Key::ProxyCommand => {
                !self.first.contains_key(&Key::ProxyJump)
                    && !self.first.contains_key(&Key::ProxyCommand)
            }
            key => !self.first.contains_key(&key),
        };
        if unset && affects_known_hosts(o.key) {
            self.known_hosts_uncertain = true;
        }
    }
}

/// Per-connection values that take precedence over the config (OpenSSH applies the
/// command-line user and port before reading the config): `bob@host` makes `Match user`
/// and `%r` see `bob`, a bookmark port makes `%p` see that port.
#[derive(Debug, Clone, Copy, Default)]
pub struct QueryOverrides<'a> {
    /// The connection's own user, as given. `Some` ⇒ the config `User` is not expanded (it is
    /// not used). `Match user` sees it as given (a pattern match, never a path, as OpenSSH);
    /// `%r` only if it is usable in a local path ([`path_safe_user`]), otherwise `%r` is
    /// unavailable. `%C` always hashes it as given.
    pub user: Option<&'a str>,
    pub port: Option<u16>,
    /// `%j`: host part of the hop this connection jumps through last, from the connection
    /// chain (a bookmark's own ProxyJump, the previous element of a jump list; `""` = no jump).
    /// `None` = take it from the config's `ProxyJump`.
    pub jump: Option<&'a str>,
}

/// A user name that may appear in a local path via `%r` (`IdentityFile ~/.ssh/id_%r`): no
/// path separator, no `..` (`DOMAIN\alice`, or a hostile imported bookmark, are not).
pub fn path_safe_user(user: &str) -> bool {
    !user.contains(['/', '\\']) && !user.contains("..")
}

/// Characters OpenSSH's `valid_ruser` refuses in an expanded user (shell metacharacters).
const USER_SHELL_CHARS: &[char] = &[
    '\'', '`', '"', ';', '&', '<', '>', '|', '(', ')', '{', '}', '$', '\\',
];

/// An expanded `User` (only checked when expansion changed the value; literal values are
/// used as written): OpenSSH `valid_ruser` (no controls, no shell metacharacters, no leading
/// `-`, no whitespace before a `-`) plus stricter rules: 1–255 characters, no whitespace, no
/// invisible/bidi characters, no `/`, no `..`.
fn valid_expanded_user(user: &str) -> bool {
    let n = user.chars().count();
    (1..=MAX_USER_CHARS).contains(&n)
        && !user.starts_with('-')
        && path_safe_user(user)
        && !user.chars().any(|c| {
            c.is_whitespace()
                || c.is_control()
                || is_deceptive_char(c)
                || USER_SHELL_CHARS.contains(&c)
        })
}

/// The config `User` text for the `invalidUser` warning/error: as written, at most 255
/// characters (a hostile line may be 4 MiB).
fn user_warning_value(raw: &str) -> String {
    if raw.chars().count() <= MAX_USER_CHARS {
        return raw.to_owned();
    }
    let mut v: String = raw.chars().take(MAX_USER_CHARS).collect();
    v.push('…');
    v
}

struct Walker<'e> {
    alias: &'e str,
    env: &'e ConfigEnv,
    user_override: Option<&'e str>,
}

impl Walker<'_> {
    /// HostName so far (for `Match host`): `%h` expanded with the alias; the alias if unset.
    fn host_so_far(&self, c: &Collector<'_>) -> String {
        c.get(Key::HostName)
            .and_then(|o| expand_hostname(o.first(), self.alias).ok())
            .unwrap_or_else(|| self.alias.to_owned())
    }

    fn user_so_far(&self, c: &Collector<'_>) -> String {
        if let Some(u) = self.user_override {
            return u.to_owned();
        }
        c.get(Key::User)
            .map_or_else(|| self.env.local_user.clone(), |o| o.first().to_owned())
    }

    /// `uncertain`: these items belong to a block skipped only for unsupported `Match`
    /// criteria (an `Include` inside it); see [`Collector::note_skipped`].
    fn walk<'a>(
        &self,
        items: &'a [Item],
        never_match: bool,
        uncertain: bool,
        c: &mut Collector<'a>,
    ) {
        let mut active = !never_match;
        // The current block was skipped only because of unsupported criteria.
        let mut skipped_unsupported = uncertain;
        for item in items {
            match item {
                Item::Host { patterns, .. } => {
                    active = !never_match && host_block_matches(self.alias, patterns);
                    skipped_unsupported = uncertain;
                }
                Item::Match {
                    criteria,
                    unsupported,
                    loc,
                } => {
                    if never_match {
                        active = false;
                        skipped_unsupported = uncertain;
                        continue;
                    }
                    let supported_ok = criteria.iter().all(|cr| {
                        let ok = match cr.kind {
                            CritKind::All => true,
                            CritKind::Host => {
                                match_list(&self.host_so_far(c), &cr.list, Case::AsciiInsensitive)
                                    == ListMatch::Positive
                            }
                            CritKind::OriginalHost => {
                                match_list(self.alias, &cr.list, Case::AsciiInsensitive)
                                    == ListMatch::Positive
                            }
                            CritKind::User => {
                                match_list(&self.user_so_far(c), &cr.list, Case::Sensitive)
                                    == ListMatch::Positive
                            }
                            CritKind::LocalUser => {
                                match_list(&self.env.local_user, &cr.list, Case::Sensitive)
                                    == ListMatch::Positive
                            }
                        };
                        ok != cr.negate
                    });
                    if supported_ok && !unsupported.is_empty() {
                        c.warnings.push(warning(
                            loc.clone(),
                            ConfigWarningCode::UnsupportedMatchCriteria {
                                criteria: unsupported.join(","),
                            },
                        ));
                    }
                    active = supported_ok && unsupported.is_empty();
                    skipped_unsupported = uncertain || (supported_ok && !unsupported.is_empty());
                }
                Item::Opt(o) => {
                    if active {
                        c.apply(o);
                    } else if skipped_unsupported {
                        c.note_skipped(o);
                    }
                }
                Item::Include { files } => {
                    let child_never = never_match || !active;
                    let child_uncertain = !active && skipped_unsupported;
                    for f in files {
                        self.walk(&f.items, child_never, child_uncertain, c);
                    }
                }
            }
        }
    }
}

fn unknown_token(c_warnings: &mut Vec<ConfigWarning>, loc: &SourceLocation, token: String) {
    c_warnings.push(warning(
        loc.clone(),
        ConfigWarningCode::UnknownToken { token },
    ));
}

/// The warning for a file option that could not be expanded (the value is dropped).
fn expand_warning(key: Key, err: ExpandError) -> ConfigWarningCode {
    match err {
        ExpandError::Unknown(token) => ConfigWarningCode::UnknownToken { token },
        ExpandError::Unavailable(token) => ConfigWarningCode::UnavailableToken { token },
        ExpandError::TooLong => ConfigWarningCode::ExpansionTooLong {
            option: path_keyword(key).to_owned(),
        },
    }
}

/// `User` from the config: as written if it has no `%`/`${`; else expanded, and validated if
/// expansion changed it. `None` = unexpandable or unsafe (`invalidUser`).
fn expand_config_user(raw: &str, ctx: &TokenCtx<'_>) -> Option<String> {
    if !raw.contains('%') && !raw.contains("${") {
        return Some(raw.to_owned());
    }
    match expand_user(raw, ctx) {
        Ok(v) if v == raw || valid_expanded_user(&v) => Some(v),
        _ => None,
    }
}

/// `%j` from the config (OpenSSH `jump_host`): the host part of the LAST `ProxyJump` entry;
/// `""` without `ProxyJump`, with `ProxyJump none`, or when `ProxyCommand` came first.
/// `None` if that entry is not a valid `[user@]host[:port]` (the connection fails anyway).
fn config_jump_host(c: &Collector<'_>) -> Option<String> {
    let Some(o) = c.get(Key::ProxyJump) else {
        return Some(String::new());
    };
    if o.first().eq_ignore_ascii_case("none") {
        return Some(String::new());
    }
    let last = list_arg(o).pop()?;
    parse_host_spec(&last).map(|spec| spec.host)
}

/// The keyword of a path option (for the `expansionTooLong` warning).
fn path_keyword(key: Key) -> &'static str {
    match key {
        Key::IdentityFile => "IdentityFile",
        Key::CertificateFile => "CertificateFile",
        Key::IdentityAgent => "IdentityAgent",
        Key::UserKnownHostsFile => "UserKnownHostsFile",
        Key::GlobalKnownHostsFile => "GlobalKnownHostsFile",
        _ => "?",
    }
}

/// Join a list option's arguments: `ProxyJump a, b` lexes as `["a,", "b"]`; OpenSSH rejects
/// the extra argument, and silently using only the first would drop a hop.
fn list_arg(o: &OptLine) -> Vec<String> {
    o.args
        .iter()
        .flat_map(|a| a.split(','))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

pub(crate) fn resolve(
    parsed: &ParsedConfig,
    alias: &str,
    env: &ConfigEnv,
    overrides: QueryOverrides<'_>,
) -> Result<Resolution, ResolveError> {
    let walker = Walker {
        alias,
        env,
        // `Match user` compares the connection's user as given (OpenSSH), even one that is
        // not path-safe: matching never builds a path, and filtering it would fall back to
        // the config `User` / local user and apply blocks OpenSSH does not apply.
        user_override: overrides.user,
    };
    let mut c = Collector::default();
    for file in [&parsed.user, &parsed.system].into_iter().flatten() {
        walker.walk(&file.items, false, false, &mut c);
    }
    let mut warnings = std::mem::take(&mut c.warnings);
    let mut cfg = HostConfig::default();

    // (a) HostName's own %h/%% → (b) valid_hostname → (c) used in file-option %h below.
    if let Some(o) = c.get(Key::HostName) {
        match expand_hostname(o.first(), alias) {
            Ok(v) if valid_hostname(&v) => cfg.hostname = Some(v),
            Ok(v) => {
                return Err(ResolveError {
                    error: QueryError::InvalidHostname { value: v },
                    loc: o.loc.clone(),
                });
            }
            // Never fall back to the bare alias (another machine via DNS); OpenSSH
            // aborts too.
            Err(_token) => {
                return Err(ResolveError {
                    error: QueryError::InvalidHostname {
                        value: o.first().to_owned(),
                    },
                    loc: o.loc.clone(),
                });
            }
        }
    }
    if let Some(o) = c.get(Key::HostKeyAlias) {
        let v = o.first().to_owned();
        if !valid_hostname(&v) {
            return Err(ResolveError {
                error: QueryError::InvalidHostname { value: v },
                loc: o.loc.clone(),
            });
        }
        cfg.host_key_alias = Some(v);
    }
    cfg.port = c.get(Key::Port).and_then(|o| o.first().parse().ok());

    // `%h` (User, file options, `%C`) is the lowercased / normalised host, as OpenSSH; `None`
    // for a numeric IPv4 the C resolver would rewrite (`127.1`) or an IPv6 form C libraries
    // spell differently. With `CanonicalizeHostname` other than `no` OpenSSH replaces the host
    // with the canonicalised name (DNS, `CanonicalDomains`) before expanding `User` and the
    // file options; Shellfin does not canonicalise, so `%h` (and with it `%C`) is unavailable.
    let canonicalize = c
        .get(Key::CanonicalizeHostname)
        .is_some_and(|o| !matches!(o.first().to_ascii_lowercase().as_str(), "no" | "false"));
    let host_for_tokens = if canonicalize {
        None
    } else {
        canonical_host(cfg.hostname.as_deref().unwrap_or(alias))
    };
    let local_user = env.local_user_exact.then_some(env.local_user.as_str());
    let port = overrides.port.or(cfg.port).unwrap_or(22);
    let key_alias = cfg
        .host_key_alias
        .clone()
        .unwrap_or_else(|| alias.to_owned());
    let jump_host = match overrides.jump {
        Some(j) => Some(j.to_owned()),
        None => config_jump_host(&c),
    };
    // Checked again here (defence in depth; `platform::local_host_name` already filters): a
    // name that could act as path syntax is never substituted.
    let local_host = env
        .local_host
        .as_deref()
        .filter(|h| crate::platform::plain_local_host(h));
    // Tokens for `User`: no `%r` (itself) and no `%C` (hashes the user).
    let user_ctx = TokenCtx {
        home: &env.home,
        local_user,
        host: host_for_tokens.as_deref(),
        port,
        remote_user: None,
        alias,
        key_alias: &key_alias,
        local_host,
        uid: env
            .uid
            .as_deref()
            .filter(|u| !u.is_empty() && u.bytes().all(|b| b.is_ascii_digit())),
        jump_host: jump_host.as_deref(),
        conn_hash: None,
        env: env.env.as_ref(),
    };
    if let Some(o) = c.get(Key::User) {
        let raw = o.first();
        if overrides.user.is_some() {
            // The connection's own user wins and the config value is not used: keep it raw,
            // never expand it (OpenSSH expands only a config-supplied User).
            cfg.user = Some(raw.to_owned());
        } else if let Some(user) = expand_config_user(raw, &user_ctx) {
            cfg.user = Some(user);
        } else {
            let value = user_warning_value(raw);
            warnings.push(warning(
                o.loc.clone(),
                ConfigWarningCode::InvalidUser {
                    value: value.clone(),
                },
            ));
            cfg.user_unresolved = Some(value);
        }
    }

    // The final remote user: the connection's own, else the (expanded) config User, else the
    // local user; unknown if the config User could not be expanded, or if it is the local user
    // and that name is not exactly OpenSSH's (a Windows domain account: `%C` would then name
    // another file than OpenSSH reads, and `accept-new` would trust silently).
    let final_user: Option<&str> = match overrides.user {
        Some(u) => Some(u),
        None if cfg.user_unresolved.is_some() => None,
        None => cfg.user.as_deref().or(local_user),
    };
    // `%r` ends up in local paths: a connection user with a path separator is not used
    // (the token is then unavailable instead of silently naming another user's file).
    let remote_user = match overrides.user {
        Some(u) => path_safe_user(u).then_some(u),
        None => final_user,
    };
    // `%C` hashes the user as given (the result is hex, so any user is path-safe); it is
    // unavailable whenever `%h` is (also with `CanonicalizeHostname`, see above).
    let conn_hash = match (
        local_host,
        host_for_tokens.as_deref(),
        final_user,
        jump_host.as_deref(),
    ) {
        (Some(local), Some(host), Some(user), Some(jump)) => {
            Some(connection_hash(local, host, port, user, jump))
        }
        _ => None,
    };
    let ctx = TokenCtx {
        remote_user,
        conn_hash: conn_hash.as_deref(),
        ..user_ctx
    };
    // An unexpandable value (unknown or unavailable token, longer than any usable path) is
    // dropped with a warning saying which.
    let expand = |o: &OptLine, value: &str, warnings: &mut Vec<ConfigWarning>| -> Option<String> {
        expand_path(value, &ctx)
            .map_err(|e| warnings.push(warning(o.loc.clone(), expand_warning(o.key, e))))
            .ok()
    };

    if !c.identity_none {
        for o in &c.identity_files {
            if let Some(path) = expand(o, o.first(), &mut warnings) {
                cfg.identity_files.push(IdentityFileOpt {
                    path,
                    explicit: true,
                });
            }
        }
    }
    for o in &c.certificate_files {
        if let Some(path) = expand(o, o.first(), &mut warnings) {
            cfg.certificate_files.push(path);
        }
    }
    if let Some(o) = c.get(Key::IdentityAgent) {
        let v = o.first();
        let braced = v.strip_prefix("${").and_then(|r| r.strip_suffix('}'));
        cfg.identity_agent = if v.eq_ignore_ascii_case("none") {
            Some(AgentSetting::Disabled)
        } else if v == "SSH_AUTH_SOCK" {
            Some(AgentSetting::EnvVar(v.to_owned()))
        } else if let Some(name) = braced.or_else(|| v.strip_prefix('$')) {
            if valid_env_name(name) {
                Some(AgentSetting::EnvVar(name.to_owned()))
            } else {
                unknown_token(&mut warnings, &o.loc, v.to_owned());
                None
            }
        } else {
            expand(o, v, &mut warnings).map(AgentSetting::Path)
        };
    }
    for (key, slot) in [
        (Key::UserKnownHostsFile, &mut cfg.user_known_hosts),
        (Key::GlobalKnownHostsFile, &mut cfg.global_known_hosts),
    ] {
        if let Some(o) = c.get(key) {
            let mut files = Vec::new();
            let mut unresolved = false;
            for arg in &o.args {
                if arg.eq_ignore_ascii_case("none")
                    || arg == "/dev/null"
                    || arg.eq_ignore_ascii_case("NUL")
                {
                    files.push(KnownHostsFileOpt::Disabled);
                } else if let Some(p) = expand(o, arg, &mut warnings) {
                    files.push(KnownHostsFileOpt::Path(p));
                } else {
                    unresolved = true;
                }
            }
            // A dropped argument must never leave an empty list (no files read, nothing
            // learned, every key "new"). Fall back to the defaults and make the hop ask.
            if unresolved {
                cfg.known_hosts_unresolved = true;
            } else {
                *slot = Some(files);
            }
        }
    }
    // An unsupported `Match` block OpenSSH might apply could change the files or `%r`/`%C`.
    if c.known_hosts_uncertain {
        cfg.known_hosts_unresolved = true;
    }
    if let Some(o) = c.get(Key::ProxyJump) {
        cfg.proxy_jump = Some(if o.first().eq_ignore_ascii_case("none") {
            ProxyJumpOpt::Disabled
        } else {
            ProxyJumpOpt::Hops(list_arg(o))
        });
    }
    let mut proxy_command_loc = None;
    if let Some(o) = c.get(Key::ProxyCommand)
        && !o.first().eq_ignore_ascii_case("none")
    {
        cfg.proxy_command = Some(o.args.join(" "));
        proxy_command_loc = Some(o.loc.clone());
    }
    cfg.strict_host_key_checking = c.get(Key::StrictHostKeyChecking).and_then(|o| {
        match o.first().to_ascii_lowercase().as_str() {
            "yes" | "true" => Some(StrictMode::Yes),
            "ask" => Some(StrictMode::Ask),
            "accept-new" => Some(StrictMode::AcceptNew),
            "no" | "off" | "false" => Some(StrictMode::No),
            _ => None,
        }
    });
    cfg.host_key_algorithms_add_ssh_rsa = c.get(Key::HostKeyAlgorithms).is_some_and(|o| {
        o.first().strip_prefix('+').is_some_and(|list| {
            list.split(',')
                .any(|a| a.trim().eq_ignore_ascii_case("ssh-rsa"))
        })
    });
    let bool_of = |key: Key| c.get(key).and_then(|o| parse_bool(o.first()));
    cfg.identities_only = bool_of(Key::IdentitiesOnly);
    cfg.hash_known_hosts = bool_of(Key::HashKnownHosts);
    cfg.password_authentication = bool_of(Key::PasswordAuthentication);
    cfg.kbd_interactive_authentication = bool_of(Key::KbdInteractiveAuthentication);
    cfg.pubkey_authentication = c.get(Key::PubkeyAuthentication).and_then(|o| {
        parse_bool(o.first()).or_else(|| {
            matches!(
                o.first().to_ascii_lowercase().as_str(),
                "unbound" | "host-bound"
            )
            .then_some(true)
        })
    });
    cfg.server_alive_interval = c
        .get(Key::ServerAliveInterval)
        .and_then(|o| parse_time(o.first()));
    cfg.connect_timeout = c
        .get(Key::ConnectTimeout)
        .and_then(|o| parse_time(o.first()));
    cfg.server_alive_count_max = c
        .get(Key::ServerAliveCountMax)
        .and_then(|o| o.first().parse().ok());
    cfg.preferred_authentications = c.get(Key::PreferredAuthentications).map(list_arg);
    cfg.add_keys_to_agent = c.get(Key::AddKeysToAgent).and_then(|o| match o.scope {
        super::ConfigScope::User => Some(matches!(
            o.first().to_ascii_lowercase().as_str(),
            "yes" | "true"
        )),
        super::ConfigScope::System => None,
    });
    cfg.warnings = warnings;
    Ok(Resolution {
        config: cfg,
        proxy_command_loc,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use crate::sshconfig::testutil::Fixture;
    use pretty_assertions::assert_eq;

    fn q(f: &Fixture, alias: &str) -> HostConfig {
        f.load().query(alias, &f.env).unwrap()
    }

    #[test]
    fn global_options_and_first_value_wins() {
        let f = Fixture::new();
        f.user_config(
            "User globaluser\nPort 2000\n\nHost web\n  User webuser\n  Port 3000\n  HostName web.example.com\n",
        );
        let c = q(&f, "web");
        assert_eq!(
            c.user.as_deref(),
            Some("globaluser"),
            "options before the first Host apply first"
        );
        assert_eq!(c.port, Some(2000));
        assert_eq!(c.hostname.as_deref(), Some("web.example.com"));
    }

    #[test]
    fn host_star_after_specific_hosts_supplies_defaults() {
        let f = Fixture::new();
        f.user_config("Host db\n  User dbadmin\nHost *\n  User fallback\n  Port 2222\n");
        let db = q(&f, "db");
        assert_eq!(db.user.as_deref(), Some("dbadmin"));
        assert_eq!(db.port, Some(2222));
        assert_eq!(q(&f, "other").user.as_deref(), Some("fallback"));
    }

    /// Keywords are case-insensitive; `Host` patterns are case-sensitive against the alias as
    /// typed, as OpenSSH (`ssh -G`: `Host Lab` applies to `Lab`, not `lab`/`LAB`).
    #[test]
    fn negation_question_mark_and_case_rules() {
        let f = Fixture::new();
        f.user_config(
            "Host web? !web3\n  Port 8022\nHOST Bastion\n  user jump\nHost Lab\n  Port 1001\n\
             Host lower\n  Port 1002\nHost *.EXAMPLE.com\n  Port 1003\n",
        );
        assert_eq!(q(&f, "web1").port, Some(8022));
        assert_eq!(q(&f, "web3").port, None);
        assert_eq!(q(&f, "web10").port, None);
        assert_eq!(q(&f, "WEB2").port, None);
        assert_eq!(q(&f, "Bastion").user.as_deref(), Some("jump"));
        assert_eq!(q(&f, "bastion").user, None);
        assert_eq!(q(&f, "Lab").port, Some(1001));
        assert_eq!(q(&f, "lab").port, None);
        assert_eq!(q(&f, "LAB").port, None);
        assert_eq!(q(&f, "lower").port, Some(1002));
        assert_eq!(q(&f, "LOWER").port, None);
        assert_eq!(q(&f, "a.EXAMPLE.com").port, Some(1003));
        assert_eq!(q(&f, "a.example.com").port, None);
    }

    /// `Match host`/`originalhost` compare ASCII case-insensitively, `Match user`/`localuser`
    /// case-sensitively (OpenSSH `match_hostname` vs `match_pattern_list(…, 0)`; checked with
    /// `ssh -G` on Win32 9.5p2 and MSYS 10.2p1).
    #[test]
    fn match_criteria_case_rules() {
        let mut f = Fixture::new();
        f.env.local_user = "smith".to_owned();
        f.user_config(concat!(
            "Host hn1\n  HostName MixHost.example.org\n",
            "Match host mixhost.example.org\n  Port 1015\n",
            "Match originalhost OrigCase\n  ConnectTimeout 4\n",
            "Match user Smith\n  ServerAliveInterval 11\n",
            "Match localuser Smith\n  ServerAliveCountMax 21\n",
            "Match user smith\n  IdentitiesOnly yes\n",
            "Match localuser smith\n  HashKnownHosts yes\n",
        ));
        let hn1 = q(&f, "hn1");
        assert_eq!(hn1.port, Some(1015), "Match host ignores case");
        assert_eq!(q(&f, "MIXHOST.Example.ORG").port, Some(1015));
        assert_eq!(q(&f, "origcase").connect_timeout, Some(4));
        assert_eq!(q(&f, "ORIGCASE").connect_timeout, Some(4));
        // Implicit user `smith` (Win32-OpenSSH lowercases the account): `Smith` does not match.
        assert_eq!(hn1.server_alive_interval, None);
        assert_eq!(hn1.server_alive_count_max, None);
        assert_eq!(hn1.identities_only, Some(true));
        assert_eq!(hn1.hash_known_hosts, Some(true));
        // `Smith@host`: the connection's user as given.
        let p = f.load();
        let own = p
            .query_with(
                "x",
                &f.env,
                QueryOverrides {
                    user: Some("Smith"),
                    port: None,
                    jump: None,
                },
            )
            .unwrap();
        assert_eq!(own.server_alive_interval, Some(11));
        assert_eq!(own.identities_only, None);
    }

    #[test]
    fn key_value_quotes_tabs_trailing_comments() {
        let f = Fixture::new();
        f.user_config(
            "Host=q\n\tHostName=q.example.com # the real name\n\tUser = \"alice example\"\n\tIdentityFile \"~/.ssh/my key\" # quoted\n",
        );
        let c = q(&f, "q");
        assert_eq!(c.hostname.as_deref(), Some("q.example.com"));
        assert_eq!(c.user.as_deref(), Some("alice example"));
        assert_eq!(
            c.identity_files[0].path,
            f.home().join(".ssh/my key").display().to_string()
        );
    }

    #[test]
    fn include_inside_host_is_conditional() {
        let f = Fixture::new();
        f.user_file("inc.conf", "Port 4444\nHost inner\n  User inneruser\n");
        f.user_config("Host outer\n  Include inc.conf\nHost *\n  Port 22\n");
        assert_eq!(q(&f, "outer").port, Some(4444));
        assert_eq!(
            q(&f, "other").port,
            Some(22),
            "included file never matches when the enclosing Host does not"
        );
        assert_eq!(
            q(&f, "inner").user,
            None,
            "Host lines inside an inactive include never match"
        );
    }

    #[test]
    fn match_in_include_does_not_clobber_parent_state() {
        let f = Fixture::new();
        f.user_file("inc.conf", "Host nomatch\n");
        f.user_config("Host a\n  Include inc.conf\n  Port 1111\n");
        assert_eq!(q(&f, "a").port, Some(1111));
    }

    #[test]
    fn match_subset() {
        let f = Fixture::new();
        f.user_config(
            "Host *.int\n  HostName %h.corp.example.com\n\
             Match host *.corp.example.com\n  User corp\n\
             Match originalhost db\n  Port 5432\n\
             Match user corp localuser alice\n  IdentitiesOnly yes\n\
             Match !localuser alice\n  Port 1\n\
             Match all\n  ConnectTimeout 7\n",
        );
        let c = q(&f, "x.int");
        assert_eq!(c.hostname.as_deref(), Some("x.int.corp.example.com"));
        assert_eq!(c.user.as_deref(), Some("corp"));
        assert_eq!(c.identities_only, Some(true));
        assert_eq!(c.port, None);
        assert_eq!(c.connect_timeout, Some(7));
        let db = q(&f, "db");
        assert_eq!(db.port, Some(5432));
        assert_eq!(db.user, None);
        assert_eq!(
            db.identities_only, None,
            "Match user uses the local user when no User is set"
        );
    }

    #[test]
    fn unsupported_match_never_matches_and_warns_per_host() {
        let f = Fixture::new();
        f.user_config("Match host db exec \"touch /tmp/pwned\"\n  Port 1\nMatch final\n  Port 2\n");
        let p = f.load();
        let db = p.query("db", &f.env).unwrap();
        assert_eq!(db.port, None);
        assert_eq!(db.warnings.len(), 2, "{:?}", db.warnings);
        let web = p.query("web", &f.env).unwrap();
        assert_eq!(web.port, None);
        assert_eq!(
            web.warnings.len(),
            1,
            "the exec block's host criterion does not match web"
        );
    }

    /// (L, #7 verifier) A block skipped only for unsupported criteria (`Match exec …`) that
    /// would set `User` (→ `%r`/`%C`) or a known_hosts option while it is still unset makes the
    /// hop ask (OpenSSH may apply it and read other files); it never does when the option was
    /// already set, when the block's supported criteria fail, or for unrelated options.
    #[test]
    fn unsupported_match_setting_user_makes_known_hosts_uncertain() {
        let f = with_host(Fixture::new());
        f.user_file("inc.conf", "UserKnownHostsFile ~/.ssh/kh/inc\n");
        f.user_config(concat!(
            "Host t-matchex\n  UserKnownHostsFile ~/.ssh/kh/base-%C\n",
            "Match exec \"echo hi\" host t-matchex\n  User EXECUSER\n",
            "Host t-set\n  User fixed\n",
            "Match exec \"echo hi\" host t-set\n  User other\n  ServerAliveInterval 5\n",
            "Match exec \"echo hi\" host t-other\n  ServerAliveInterval 5\n",
            "Match exec \"echo hi\" host t-inc\n  Include inc.conf\n",
            "Match exec \"echo hi\" host nomatch\n  User EXECUSER\n",
        ));
        let m = q(&f, "t-matchex");
        assert!(
            m.known_hosts_unresolved,
            "User from an exec block could change %C"
        );
        assert_eq!(m.user, None, "the block itself is not applied");
        assert!(
            !q(&f, "t-set").known_hosts_unresolved,
            "User was already set: first value wins"
        );
        assert!(!q(&f, "t-other").known_hosts_unresolved);
        assert!(
            q(&f, "t-inc").known_hosts_unresolved,
            "an Include inside the block counts too"
        );
        assert!(!q(&f, "plain").known_hosts_unresolved);
    }

    #[test]
    fn identity_files_accumulate_and_none_disables() {
        let f = Fixture::new();
        f.user_config("Host a\n  IdentityFile ~/.ssh/a\nHost *\n  IdentityFile ~/.ssh/default\nHost b\n  IdentityFile none\n");
        let a = q(&f, "a");
        let paths: Vec<_> = a.identity_files.iter().map(|i| i.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                f.home().join(".ssh/a").display().to_string(),
                f.home().join(".ssh/default").display().to_string()
            ]
        );
        assert!(a.identity_files.iter().all(|i| i.explicit));
        assert!(q(&f, "b").identity_files.is_empty());
    }

    #[test]
    fn proxy_jump_none_and_proxy_command() {
        let f = Fixture::new();
        f.user_config(
            "Host direct\n  ProxyJump none\nHost viacmd\n  ProxyCommand ssh -W %h:%p gw\n\
             Host jumped\n  ProxyJump alice@gw:2222,bastion\n  ProxyCommand nc %h %p\n\
             Host *\n  ProxyJump defaultgw\n",
        );
        assert_eq!(q(&f, "direct").proxy_jump, Some(ProxyJumpOpt::Disabled));
        let c = q(&f, "viacmd");
        assert_eq!(c.proxy_command.as_deref(), Some("ssh -W %h:%p gw"));
        assert_eq!(
            c.proxy_jump, None,
            "ProxyCommand came first, ProxyJump is ignored"
        );
        let j = q(&f, "jumped");
        assert_eq!(
            j.proxy_jump,
            Some(ProxyJumpOpt::Hops(vec![
                "alice@gw:2222".into(),
                "bastion".into()
            ]))
        );
        assert_eq!(j.proxy_command, None);
        assert_eq!(
            q(&f, "other").proxy_jump,
            Some(ProxyJumpOpt::Hops(vec!["defaultgw".into()]))
        );
    }

    #[test]
    fn tokens_in_file_options() {
        let f = Fixture::with_env(&[("KEYDIR", "/opt/keys")]);
        f.user_config(
            "Host t\n  HostName t.example.com\n  User deploy\n  Port 2200\n\
             IdentityFile ~/.ssh/%h-%p-%r-%n-%u\n  IdentityFile ${KEYDIR}/id\n  IdentityFile %d/k%%\n\
             CertificateFile ~/.ssh/%n-cert.pub\n",
        );
        let c = q(&f, "t");
        let paths: Vec<_> = c.identity_files.iter().map(|i| i.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                f.home()
                    .join(".ssh/t.example.com-2200-deploy-t-alice")
                    .display()
                    .to_string(),
                "/opt/keys/id".to_owned(),
                format!("{}/k%", f.home().display()),
            ]
        );
        assert_eq!(
            c.certificate_files,
            vec![f.home().join(".ssh/t-cert.pub").display().to_string()]
        );
        assert!(c.warnings.is_empty());
    }

    #[test]
    fn home_env_var_and_unknown_tokens() {
        let f = Fixture::with_env(&[("HOME", "/home/alice")]);
        f.user_config("Host t\n  IdentityFile ${HOME}/.ssh/id_t\n  IdentityFile ${NOPE}/x\n  IdentityFile ~/.ssh/%Z\n");
        let c = q(&f, "t");
        assert_eq!(c.identity_files.len(), 1);
        assert_eq!(c.identity_files[0].path, "/home/alice/.ssh/id_t");
        let tokens: Vec<_> = c
            .warnings
            .iter()
            .map(|w| match &w.kind {
                ConfigWarningCode::UnknownToken { token } => token.clone(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(tokens, vec!["${NOPE}".to_owned(), "%Z".to_owned()]);
    }

    #[test]
    fn known_hosts_disabled_forms() {
        let f = Fixture::new();
        f.user_config(
            "Host a\n  UserKnownHostsFile none\nHost b\n  UserKnownHostsFile /dev/null\n\
             Host c\n  UserKnownHostsFile nul\n  GlobalKnownHostsFile NUL\n\
             Host d\n  UserKnownHostsFile ~/.ssh/kh1 ~/.ssh/kh_%h\n",
        );
        for alias in ["a", "b", "c"] {
            assert_eq!(
                q(&f, alias).user_known_hosts,
                Some(vec![KnownHostsFileOpt::Disabled]),
                "{alias}"
            );
        }
        assert_eq!(
            q(&f, "c").global_known_hosts,
            Some(vec![KnownHostsFileOpt::Disabled])
        );
        assert_eq!(
            q(&f, "d").user_known_hosts,
            Some(vec![
                KnownHostsFileOpt::Path(f.home().join(".ssh/kh1").display().to_string()),
                KnownHostsFileOpt::Path(f.home().join(".ssh/kh_d").display().to_string()),
            ])
        );
    }

    /// `%k` is supported; an argument that cannot be expanded (here `%C`/`%l` without a local
    /// host name) never leaves an empty list (no files read, nothing learned): the option falls
    /// back to the defaults, warns, and marks the hop so `accept-new`/`no` ask.
    #[test]
    fn known_hosts_tokens_and_unexpandable_arguments() {
        let f = Fixture::new();
        f.user_config(
            "Host k\n  HostKeyAlias gw-key\n  UserKnownHostsFile ~/.ssh/known_hosts.d/%k\n\
             Host n\n  UserKnownHostsFile ~/.ssh/known_hosts.d/%k\n\
             Host c\n  StrictHostKeyChecking accept-new\n  UserKnownHostsFile ~/.ssh/kh ~/.ssh/known_hosts.d/%C\n\
             Host g\n  GlobalKnownHostsFile /etc/ssh/%l\n",
        );
        let k = q(&f, "k");
        assert_eq!(
            k.user_known_hosts,
            Some(vec![KnownHostsFileOpt::Path(
                f.home()
                    .join(".ssh/known_hosts.d/gw-key")
                    .display()
                    .to_string()
            )])
        );
        assert!(!k.known_hosts_unresolved);
        assert_eq!(
            q(&f, "n").user_known_hosts,
            Some(vec![KnownHostsFileOpt::Path(
                f.home().join(".ssh/known_hosts.d/n").display().to_string()
            )])
        );
        let c = q(&f, "c");
        assert_eq!(c.user_known_hosts, None, "defaults, not an empty list");
        assert!(c.known_hosts_unresolved);
        assert!(
            matches!(
                &c.warnings[..],
                [ConfigWarning { kind: ConfigWarningCode::UnavailableToken { token }, .. }] if token == "%C"
            ),
            "an accurate warning, not `unknown token`: {:?}",
            c.warnings
        );
        let g = q(&f, "g");
        assert_eq!(g.global_known_hosts, None);
        assert!(g.known_hosts_unresolved);
    }

    /// `ProxyJump a, b` (and `PreferredAuthentications x, y`) keep every hop/method.
    #[test]
    fn list_options_with_spaces_after_commas() {
        let f = Fixture::new();
        f.user_config(
            "Host t\n  ProxyJump gw1, gw2 ,gw3\n  PreferredAuthentications publickey, password\n",
        );
        let c = q(&f, "t");
        assert_eq!(
            c.proxy_jump,
            Some(ProxyJumpOpt::Hops(vec![
                "gw1".into(),
                "gw2".into(),
                "gw3".into()
            ]))
        );
        assert_eq!(
            c.preferred_authentications,
            Some(vec!["publickey".into(), "password".into()])
        );
    }

    /// The connection's own user and port feed `Match user`, `%r` and `%p` first.
    #[test]
    fn connection_user_and_port_override_the_config_for_matching_and_tokens() {
        let f = Fixture::new();
        f.user_config(
            "Match user bob\n  IdentityFile ~/.ssh/bob_key\n\
             Host h\n  User alice\n  Port 2200\n  IdentityFile ~/.ssh/id_%r_%p\n",
        );
        let p = f.load();
        let plain = p.query("h", &f.env).unwrap();
        let paths = |c: &HostConfig| -> Vec<String> {
            c.identity_files.iter().map(|i| i.path.clone()).collect()
        };
        assert_eq!(
            paths(&plain),
            vec![f.home().join(".ssh/id_alice_2200").display().to_string()]
        );
        let bob = p
            .query_with(
                "h",
                &f.env,
                QueryOverrides {
                    user: Some("bob"),
                    port: Some(22),
                    jump: None,
                },
            )
            .unwrap();
        assert_eq!(
            paths(&bob),
            vec![
                f.home().join(".ssh/bob_key").display().to_string(),
                f.home().join(".ssh/id_bob_22").display().to_string(),
            ]
        );
        assert_eq!(
            bob.user.as_deref(),
            Some("alice"),
            "the config value itself"
        );
    }

    #[test]
    fn identity_agent_forms() {
        let f = Fixture::new();
        // argv_split turns `\\` into `\` (OpenSSH semantics), so a pipe path is written with
        // doubled backslashes (or quoted the same way) in the file.
        f.user_config(concat!(
            "Host a\n  IdentityAgent SSH_AUTH_SOCK\nHost b\n  IdentityAgent $MY_SOCK\nHost c\n  IdentityAgent ${OTHER_SOCK}\n",
            "Host d\n  IdentityAgent none\nHost e\n  IdentityAgent ~/.agent.sock\n",
            r"Host f",
            "\n  ",
            r"IdentityAgent \\\\.\\pipe\\pageant.alice.abc",
            "\nHost g\n  IdentityAgent $1bad\n",
        ));
        assert_eq!(
            q(&f, "a").identity_agent,
            Some(AgentSetting::EnvVar("SSH_AUTH_SOCK".into()))
        );
        assert_eq!(
            q(&f, "b").identity_agent,
            Some(AgentSetting::EnvVar("MY_SOCK".into()))
        );
        assert_eq!(
            q(&f, "c").identity_agent,
            Some(AgentSetting::EnvVar("OTHER_SOCK".into()))
        );
        assert_eq!(q(&f, "d").identity_agent, Some(AgentSetting::Disabled));
        assert_eq!(
            q(&f, "e").identity_agent,
            Some(AgentSetting::Path(
                f.home().join(".agent.sock").display().to_string()
            ))
        );
        assert_eq!(
            q(&f, "f").identity_agent,
            Some(AgentSetting::Path(r"\\.\pipe\pageant.alice.abc".into()))
        );
        let g = q(&f, "g");
        assert_eq!(g.identity_agent, None);
        assert_eq!(g.warnings.len(), 1);
    }

    #[test]
    fn invalid_hostname_and_host_key_alias() {
        let f = Fixture::new();
        f.user_config(
            "Host bad\n  HostName ../../etc/passwd\nHost badalias\n  HostKeyAlias a/b\n\
             Host tok\n  HostName %d.example.com\nHost okalias\n  HostKeyAlias gateway-ha\n",
        );
        let p = f.load();
        assert_eq!(
            p.query("bad", &f.env).unwrap_err(),
            QueryError::InvalidHostname {
                value: "../../etc/passwd".into()
            }
        );
        assert_eq!(
            p.query("badalias", &f.env).unwrap_err(),
            QueryError::InvalidHostname {
                value: "a/b".into()
            }
        );
        // An unknown token in HostName is an error (OpenSSH aborts), never a fallback
        // to the bare alias.
        assert_eq!(
            p.query("tok", &f.env).unwrap_err(),
            QueryError::InvalidHostname {
                value: "%d.example.com".into()
            }
        );
        assert_eq!(
            p.query("okalias", &f.env)
                .unwrap()
                .host_key_alias
                .as_deref(),
            Some("gateway-ha")
        );
    }

    #[test]
    fn hostname_expansion_before_validation() {
        // §10.1 ssh::target case: `Host *.int` + `HostName %h.corp.example.com`.
        let f = Fixture::new();
        f.user_config("Host *.int\n  HostName %h.corp.example.com\n  IdentityFile ~/.ssh/id_%h\n");
        let c = q(&f, "x.int");
        assert_eq!(c.hostname.as_deref(), Some("x.int.corp.example.com"));
        assert_eq!(
            c.identity_files[0].path,
            f.home()
                .join(".ssh/id_x.int.corp.example.com")
                .display()
                .to_string()
        );
    }

    #[test]
    fn user_file_wins_over_system_file_and_add_keys_scope() {
        let f = Fixture::new();
        f.user_config("Host a\n  Port 1\n");
        f.system_config("Host *\n  Port 2\n  User sysuser\n  AddKeysToAgent yes\n");
        let a = q(&f, "a");
        assert_eq!(a.port, Some(1));
        assert_eq!(a.user.as_deref(), Some("sysuser"));
        assert_eq!(
            a.add_keys_to_agent, None,
            "AddKeysToAgent from the system file is not honoured"
        );

        let f = Fixture::new();
        f.user_config("Host a\n  AddKeysToAgent yes\nHost b\n  AddKeysToAgent confirm\n");
        assert_eq!(q(&f, "a").add_keys_to_agent, Some(true));
        assert_eq!(q(&f, "b").add_keys_to_agent, Some(false));
    }

    #[test]
    fn misc_options() {
        let f = Fixture::new();
        f.user_config(
            "Host m\n  StrictHostKeyChecking accept-new\n  HostKeyAlgorithms +ssh-rsa\n  HashKnownHosts yes\n\
             ServerAliveInterval 1m\n  ServerAliveCountMax 4\n  PreferredAuthentications publickey,password\n\
             PubkeyAuthentication unbound\n  PasswordAuthentication no\n  ChallengeResponseAuthentication no\n\
             KbdInteractiveAuthentication yes\n  IdentitiesOnly yes\n\
             Host n\n  StrictHostKeyChecking off\n  HostKeyAlgorithms ssh-ed25519,ssh-rsa\n",
        );
        let m = q(&f, "m");
        assert_eq!(m.strict_host_key_checking, Some(StrictMode::AcceptNew));
        assert!(m.host_key_algorithms_add_ssh_rsa);
        assert_eq!(m.hash_known_hosts, Some(true));
        assert_eq!(m.server_alive_interval, Some(60));
        assert_eq!(m.server_alive_count_max, Some(4));
        assert_eq!(
            m.preferred_authentications,
            Some(vec!["publickey".into(), "password".into()])
        );
        assert_eq!(m.pubkey_authentication, Some(true));
        assert_eq!(m.password_authentication, Some(false));
        assert_eq!(
            m.kbd_interactive_authentication,
            Some(false),
            "first of the two aliases wins"
        );
        assert_eq!(m.identities_only, Some(true));
        let n = q(&f, "n");
        assert_eq!(n.strict_host_key_checking, Some(StrictMode::No));
        assert!(
            !n.host_key_algorithms_add_ssh_rsa,
            "only +lists are honoured"
        );
    }

    /// A fixture with a local host name and uid (as `service::production_env` sets them).
    fn with_host(mut f: Fixture) -> Fixture {
        f.env.local_host = Some("laptop.example.org".to_owned());
        f.env.uid = Some("1000".to_owned());
        f
    }

    fn kh(c: &HostConfig) -> Vec<String> {
        c.user_known_hosts
            .iter()
            .flatten()
            .map(|o| match o {
                KnownHostsFileOpt::Path(p) => p.clone(),
                KnownHostsFileOpt::Disabled => "none".to_owned(),
            })
            .collect()
    }

    fn id_paths(c: &HostConfig) -> Vec<String> {
        c.identity_files.iter().map(|i| i.path.clone()).collect()
    }

    fn invalid_user(c: &HostConfig) -> Option<String> {
        c.warnings.iter().find_map(|w| match &w.kind {
            ConfigWarningCode::InvalidUser { value } => Some(value.clone()),
            _ => None,
        })
    }

    fn unknown_tokens(c: &HostConfig) -> Vec<String> {
        c.warnings
            .iter()
            .filter_map(|w| match &w.kind {
                ConfigWarningCode::UnknownToken { token } => Some(token.clone()),
                _ => None,
            })
            .collect()
    }

    fn unavailable_tokens(c: &HostConfig) -> Vec<String> {
        c.warnings
            .iter()
            .filter_map(|w| match &w.kind {
                ConfigWarningCode::UnavailableToken { token } => Some(token.clone()),
                _ => None,
            })
            .collect()
    }

    /// #6: `User` takes the OpenSSH ≥ 10.0 tokens and `${ENV}`.
    #[test]
    fn user_tokens_are_expanded() {
        let f = with_host(Fixture::with_env(&[
            ("DEPLOY_USER", "deploy"),
            ("PCT", "%h"),
        ]));
        f.user_config(concat!(
            "Host u1\n  User %u@%L\n",
            "Host u2\n  User %%u\n",
            "Host u3\n  User ops-%n-%p-%i\n  Port 2200\n",
            "Host u4\n  HostName DB.example.com\n  User %h\n",
            "Host u5\n  User ${DEPLOY_USER}\n",
            "Host u6\n  ProxyJump alice@gw1:2222, gw2.example.com:2200\n  User via-%j\n",
            "Host u7\n  User ${PCT}\n",
            "Host u8\n  HostKeyAlias db-key\n  User k-%k-%l\n",
            "Host u9\n  User plain-%j\n",
        ));
        let user = |alias: &str| {
            let c = q(&f, alias);
            assert!(c.warnings.is_empty(), "{alias}: {:?}", c.warnings);
            assert_eq!(c.user_unresolved, None, "{alias}");
            c.user.unwrap()
        };
        assert_eq!(user("u1"), "alice@laptop");
        assert_eq!(user("u2"), "%u", "%% is a literal %");
        assert_eq!(user("u3"), "ops-u3-2200-1000");
        assert_eq!(
            user("u4"),
            "db.example.com",
            "%h is lowercased like OpenSSH (10.2p1 `ssh -G`: `User x-%h` → `x-db.example.com`)"
        );
        assert_eq!(user("u5"), "deploy");
        assert_eq!(
            user("u6"),
            "via-gw2.example.com",
            "%j = host of the last ProxyJump entry"
        );
        assert_eq!(
            user("u7"),
            "%h",
            "a variable's value is never expanded again"
        );
        assert_eq!(user("u8"), "k-db-key-laptop.example.org");
        assert_eq!(user("u9"), "plain-", "%j is empty without ProxyJump");
    }

    /// (hostile input) #6: an unexpandable `User`, or one whose expansion is not a safe user
    /// name, is dropped with an `invalidUser` warning carrying the config text (never the
    /// expanded value); the host stays listed.
    #[test]
    fn invalid_users_are_refused_with_a_warning() {
        let esc = format!("{}[31mred", char::from(0x1b));
        let rtl = format!("adm{}nimda", '\u{202e}');
        let zw = format!("ali{}ce", '\u{200b}');
        let vars = [
            ("SEMI", "a;b"),
            ("OPT", "-oProxyCommand=x"),
            ("NL", "a\nb"),
            ("ESC", esc.as_str()),
            ("RTL", rtl.as_str()),
            ("ZW", zw.as_str()),
            ("UP", "../x"),
            ("SLASH", "a/b"),
            ("SPACE", "a -b"),
            ("DOLLAR", "a$b"),
            ("BACKTICK", "a`id`"),
            ("EMPTY", ""),
        ];
        let f = with_host(Fixture::with_env(&vars));
        let raws = [
            "%r",
            "x%C",
            "a%",
            "%Z",
            "${UNSET}",
            "${X",
            "%d",
            "x%d",
            "${SEMI}",
            "${OPT}",
            "${NL}",
            "${ESC}",
            "${RTL}",
            "${ZW}",
            "${UP}",
            "${SLASH}",
            "${SPACE}",
            "${DOLLAR}",
            "${BACKTICK}",
            "${EMPTY}",
        ];
        let mut cfg = String::new();
        for (i, raw) in raws.iter().enumerate() {
            cfg.push_str(&format!("Host b{i}\n  User {raw}\n"));
        }
        f.user_config(&cfg);
        for (i, raw) in raws.iter().enumerate() {
            let c = q(&f, &format!("b{i}"));
            assert_eq!(c.user, None, "{raw}");
            assert_eq!(c.user_unresolved.as_deref(), Some(*raw), "{raw}");
            assert_eq!(invalid_user(&c).as_deref(), Some(*raw), "{raw}");
        }
        let snap = f.load().snapshot(0);
        assert_eq!(snap.hosts.len(), raws.len(), "every host stays listed");
        assert!(
            snap.hosts
                .iter()
                .all(|h| h.user.is_none() && h.has_warnings)
        );

        // A 2 MiB line of `%d` is bounded and refused; the warning text is shortened.
        let f = with_host(Fixture::new());
        f.user_config(&format!("Host big\n  User {}\n", "%d".repeat(1024 * 1024)));
        let c = q(&f, "big");
        assert_eq!(c.user, None);
        let value = c.user_unresolved.unwrap();
        assert_eq!(value.chars().count(), MAX_USER_CHARS + 1);
        assert!(value.ends_with('…'));
    }

    /// #6: with the connection's own user the config `User` is neither expanded nor checked
    /// (a broken one does not matter); `%C` hashes the connection's user as given; a user that
    /// is not path-safe never reaches `%r`.
    #[test]
    fn connection_user_suppresses_user_expansion() {
        let f = with_host(Fixture::new());
        f.user_config(
            "Host h\n  User ${UNSET}\n  IdentityFile ~/.ssh/id_%r\n  UserKnownHostsFile ~/.ssh/kh/%C\n",
        );
        let p = f.load();
        let with = |user| {
            p.query_with(
                "h",
                &f.env,
                QueryOverrides {
                    user: Some(user),
                    port: None,
                    jump: None,
                },
            )
            .unwrap()
        };
        let khd = |user: &str| {
            let hash = connection_hash("laptop.example.org", "h", 22, user, "");
            f.home()
                .join(format!(".ssh/kh/{hash}"))
                .display()
                .to_string()
        };
        let bob = with("bob");
        assert_eq!(invalid_user(&bob), None);
        assert_eq!(bob.user_unresolved, None);
        assert_eq!(
            bob.user.as_deref(),
            Some("${UNSET}"),
            "the config text, unused"
        );
        assert_eq!(
            id_paths(&bob),
            [f.home().join(".ssh/id_bob").display().to_string()]
        );
        assert_eq!(kh(&bob), [khd("bob")]);

        let domain = with(r"DOMAIN\alice");
        assert!(
            domain.identity_files.is_empty(),
            "%r is unavailable for a user with a path separator"
        );
        assert_eq!(unavailable_tokens(&domain), ["%r"]);
        assert!(unknown_tokens(&domain).is_empty());
        assert_eq!(
            kh(&domain),
            [khd(r"DOMAIN\alice")],
            "%C hashes the raw user"
        );
        assert!(!domain.known_hosts_unresolved);

        // Without a user of its own the broken User is refused, and %r/%C are unknown.
        let plain = p.query("h", &f.env).unwrap();
        assert_eq!(plain.user, None);
        assert_eq!(plain.user_unresolved.as_deref(), Some("${UNSET}"));
        assert!(plain.identity_files.is_empty());
        assert!(plain.known_hosts_unresolved);
    }

    /// #6: `Match user` compares the raw `User` text (OpenSSH); `%r` is the expanded user.
    #[test]
    fn match_user_sees_the_raw_user() {
        let f = with_host(Fixture::new());
        f.user_config(
            "Host h\n  User ops-%u\n  IdentityFile ~/.ssh/id_%r\n\
             Match user ops-alice\n  Port 2222\nHost l\n  User %u\nMatch user alice\n  Port 3333\n",
        );
        let h = q(&f, "h");
        assert_eq!(h.user.as_deref(), Some("ops-alice"));
        assert_eq!(h.port, None, "Match user saw `ops-%u`");
        assert_eq!(
            id_paths(&h),
            [f.home().join(".ssh/id_ops-alice").display().to_string()]
        );
        let l = q(&f, "l");
        assert_eq!(l.user.as_deref(), Some("alice"));
        assert_eq!(l.port, None, "`User %u` does not match `Match user alice`");
    }

    /// MEDIUM (#7 review): `Match user` sees the connection's user as given, also one that is
    /// not path-safe (`corp\alice`); it must not fall back to the config `User` or the local
    /// user (`alice`) and apply blocks OpenSSH does not apply.
    #[test]
    fn match_user_sees_the_raw_connection_user() {
        let f = Fixture::new();
        f.user_config(
            "Match user alice\n  Port 2222\nMatch user corp*\n  IdentityFile ~/.ssh/id_corp\n",
        );
        let p = f.load();
        let c = p
            .query_with(
                "h",
                &f.env,
                QueryOverrides {
                    user: Some(r"corp\alice"),
                    port: None,
                    jump: None,
                },
            )
            .unwrap();
        assert_eq!(
            c.port, None,
            "the local user `alice` is not the connection's user"
        );
        assert_eq!(
            id_paths(&c),
            [f.home().join(".ssh/id_corp").display().to_string()],
            "`corp\\alice` matches `corp*`"
        );
        let plain = p.query("h", &f.env).unwrap();
        assert_eq!(plain.port, Some(2222), "without one, the local user");
        assert!(plain.identity_files.is_empty());
    }

    /// HIGH (#7 review), Windows: the implicit user is the lowercased local account
    /// (`platform::openssh_windows_user`: `Alice` → `alice`, as Win32-OpenSSH), so `%u`, `%r`
    /// and `%C` name the files OpenSSH uses. Fixed vector: SHA-1("laptop.example.org"
    /// "db.example.com" "22" "alice" "") = 1227bff8….
    #[test]
    fn implicit_windows_user_is_hashed_lowercase() {
        let mut f = with_host(Fixture::new());
        f.env.local_user =
            crate::platform::openssh_windows_user("Alice", "LAPTOP", "LAPTOP").unwrap();
        f.user_config(
            "Host db\n  HostName DB.Example.com\n  UserKnownHostsFile ~/.ssh/kh/%C\n  IdentityFile ~/.ssh/id_%u_%r\n",
        );
        let c = q(&f, "db");
        assert_eq!(
            kh(&c),
            [f.home()
                .join(".ssh/kh/1227bff8334e0ddbb57c2fb46f0c25b8ecac5c7f")
                .display()
                .to_string()]
        );
        assert_eq!(
            id_paths(&c),
            [f.home().join(".ssh/id_alice_alice").display().to_string()]
        );
        // A user given for the connection is used (and hashed) as given, not lowercased.
        let p = f.load();
        let own = p
            .query_with(
                "db",
                &f.env,
                QueryOverrides {
                    user: Some("Alice"),
                    port: None,
                    jump: None,
                },
            )
            .unwrap();
        assert_eq!(
            kh(&own),
            [f.home()
                .join(".ssh/kh/a16aec8bf093a7469ba77018b2190c6ada9f2bec")
                .display()
                .to_string()]
        );
    }

    /// HIGH (#7 review): when the local account name is not exactly OpenSSH's (a Windows
    /// domain account, a non-ASCII name), `%u` and the implicit user's `%r`/`%C` are
    /// unavailable: the known_hosts option falls back and the hop asks; a config `User` or a
    /// connection user still makes `%r`/`%C` work.
    #[test]
    fn inexact_local_user_makes_user_tokens_unavailable() {
        let mut f = with_host(Fixture::new());
        f.env.local_user_exact = false;
        f.user_config(concat!(
            "Host imp\n  StrictHostKeyChecking accept-new\n  UserKnownHostsFile ~/.ssh/kh/%C\n  IdentityFile ~/.ssh/id_%r\n",
            "Host u\n  UserKnownHostsFile ~/.ssh/kh/%u\n",
            "Host cu\n  User %u\n",
            "Host set\n  User deploy\n  UserKnownHostsFile ~/.ssh/kh/%C\n",
        ));
        let imp = q(&f, "imp");
        assert_eq!(imp.user_known_hosts, None);
        assert!(imp.known_hosts_unresolved);
        assert!(imp.identity_files.is_empty());
        assert_eq!(unavailable_tokens(&imp), ["%r", "%C"]);
        let u = q(&f, "u");
        assert!(u.known_hosts_unresolved);
        assert_eq!(unavailable_tokens(&u), ["%u"]);
        let cu = q(&f, "cu");
        assert_eq!(cu.user, None);
        assert_eq!(cu.user_unresolved.as_deref(), Some("%u"));
        let set = q(&f, "set");
        assert_eq!(
            kh(&set),
            [f.home()
                .join(format!(
                    ".ssh/kh/{}",
                    connection_hash("laptop.example.org", "set", 22, "deploy", "")
                ))
                .display()
                .to_string()]
        );
        assert!(!set.known_hosts_unresolved);
    }

    /// (L2, #7 re-review) With `CanonicalizeHostname` on, OpenSSH replaces the host by the
    /// canonicalised name before expanding `User` and the file options: `%h` is unavailable
    /// there too (not only `%C`); `CanonicalizeHostname no` changes nothing.
    #[test]
    fn canonicalize_hostname_makes_host_token_unavailable() {
        let f = with_host(Fixture::new());
        f.user_config(concat!(
            "Host c1\n  CanonicalizeHostname yes\n  HostName db.example.com\n  User x-%h\n",
            "Host c2\n  CanonicalizeHostname always\n  HostName db.example.com\n",
            "  StrictHostKeyChecking accept-new\n  UserKnownHostsFile ~/.ssh/kh/%h\n  IdentityFile ~/.ssh/id_%h\n",
            "Host c3\n  CanonicalizeHostname no\n  HostName db.example.com\n  User x-%h\n  IdentityFile ~/.ssh/id_%h\n",
        ));
        let c1 = q(&f, "c1");
        assert_eq!(c1.user, None);
        assert_eq!(c1.user_unresolved.as_deref(), Some("x-%h"));
        let c2 = q(&f, "c2");
        assert_eq!(c2.user_known_hosts, None);
        assert!(c2.known_hosts_unresolved);
        assert!(c2.identity_files.is_empty());
        assert_eq!(unavailable_tokens(&c2), ["%h", "%h"]);
        let c3 = q(&f, "c3");
        assert_eq!(c3.user.as_deref(), Some("x-db.example.com"));
        assert_eq!(
            id_paths(&c3),
            [f.home()
                .join(".ssh/id_db.example.com")
                .display()
                .to_string()]
        );
    }

    /// Decision (b): `%h` is lowercased / normalised in file options too, as OpenSSH
    /// (`HostName DB.Example.COM` → `~/.ssh/kh/db.example.com`); a non-canonical numeric IPv4
    /// (`127.1`, which OpenSSH rewrites to `127.0.0.1`) makes `%h` and `%C` unavailable.
    #[test]
    fn host_token_is_canonical_in_file_options() {
        let f = with_host(Fixture::new());
        let mut cfg = String::from(
            "Host up\n  HostName DB.Example.COM\n  UserKnownHostsFile ~/.ssh/kh/%h\n\
             Host v6\n  HostName 2001:DB8:0::1\n  UserKnownHostsFile ~/.ssh/kh/%h\n",
        );
        let numeric = ["127.1", "192.168.001.010", "0x7f.1", "2130706433"];
        for (i, n) in numeric.iter().enumerate() {
            cfg.push_str(&format!(
                "Host n{i}\n  HostName {n}\n  StrictHostKeyChecking accept-new\n  UserKnownHostsFile ~/.ssh/kh/%C\n  IdentityFile ~/.ssh/id_%h\n"
            ));
        }
        f.user_config(&cfg);
        assert_eq!(
            kh(&q(&f, "up")),
            [f.home()
                .join(".ssh/kh/db.example.com")
                .display()
                .to_string()]
        );
        assert_eq!(
            kh(&q(&f, "v6")),
            [f.home().join(".ssh/kh/2001:db8::1").display().to_string()]
        );
        // (M1, #7 re-review) IPv6 forms whose spelling differs between C libraries.
        let f6 = with_host(Fixture::new());
        let v6 = ["::7f00:1", "::ffff:0:1", "0:0:0:0:0:ffff::"];
        let mut cfg6 = String::new();
        for (i, h) in v6.iter().enumerate() {
            cfg6.push_str(&format!(
                "Host m{i}\n  HostName {h}\n  UserKnownHostsFile ~/.ssh/kh/%h ~/.ssh/kh/%C\n"
            ));
        }
        f6.user_config(&cfg6);
        for (i, h) in v6.iter().enumerate() {
            let c = q(&f6, &format!("m{i}"));
            assert_eq!(c.user_known_hosts, None, "{h}");
            assert!(c.known_hosts_unresolved, "{h}");
            assert_eq!(unavailable_tokens(&c), ["%h", "%C"], "{h}");
        }
        for (i, n) in numeric.iter().enumerate() {
            let c = q(&f, &format!("n{i}"));
            assert_eq!(c.hostname.as_deref(), Some(*n), "connects as written");
            assert_eq!(c.user_known_hosts, None, "{n}");
            assert!(c.known_hosts_unresolved, "{n}");
            assert!(c.identity_files.is_empty(), "{n}");
            assert_eq!(unavailable_tokens(&c), ["%h", "%C"], "{n}");
        }
    }

    /// #7: `%C` = SHA-1(`%l%h%p%r%j`) as OpenSSH ≥ 9.6; `%l`/`%L`/`%i`/`%C` work in every file
    /// option.
    #[test]
    fn connection_hash_and_local_tokens_in_file_options() {
        let f = with_host(Fixture::with_env(&[("PCT", "%C")]));
        f.user_config(concat!(
            "Host db\n  HostName db.example.com\n  Port 2222\n  User deploy\n  UserKnownHostsFile ~/.ssh/known_hosts.d/%C\n",
            "Host dbj\n  HostName db.example.com\n  Port 2222\n  User deploy\n  ProxyJump bob@bastion:2022\n  UserKnownHostsFile ~/.ssh/known_hosts.d/%C\n",
            "Host upper\n  HostName DB.Example.COM\n  Port 2222\n  User deploy\n  UserKnownHostsFile ~/.ssh/known_hosts.d/%C\n",
            "Host noport\n  HostName db.example.com\n  User deploy\n  UserKnownHostsFile ~/.ssh/known_hosts.d/%C\n",
            "Host v6\n  HostName 2001:DB8::1\n  UserKnownHostsFile ~/.ssh/known_hosts.d/%C\n",
            "Host lit\n  UserKnownHostsFile ~/.ssh/kh%%C ~/.ssh/kh${PCT}\n",
            "Host canon\n  CanonicalizeHostname yes\n  StrictHostKeyChecking accept-new\n  UserKnownHostsFile ~/.ssh/known_hosts.d/%C\n",
            "Host mixed\n  UserKnownHostsFile ~/.ssh/kh ~/.ssh/kh_%Z\n",
            "Host other\n  GlobalKnownHostsFile /etc/ssh/kh-%l-%L-%i\n  IdentityFile ~/.ssh/id_%C\n  CertificateFile ~/.ssh/c_%L\n  IdentityAgent ~/.agent-%L\n",
        ));
        let khd = |hash: &str| {
            f.home()
                .join(format!(".ssh/known_hosts.d/{hash}"))
                .display()
                .to_string()
        };
        let db = q(&f, "db");
        assert_eq!(kh(&db), [khd("abeddbd31e78b12b84bb72178f9da77a87609068")]);
        assert!(!db.known_hosts_unresolved && db.warnings.is_empty());
        assert_eq!(
            kh(&q(&f, "dbj")),
            [khd("afe40bf82d1a691f8e0b53bfb83376182ea83392")]
        );
        assert_eq!(
            kh(&q(&f, "upper")),
            [khd("abeddbd31e78b12b84bb72178f9da77a87609068")],
            "the host is lowercased inside the hash"
        );
        let p = f.load();
        let with_port = p
            .query_with(
                "noport",
                &f.env,
                QueryOverrides {
                    user: None,
                    port: Some(2222),
                    jump: None,
                },
            )
            .unwrap();
        assert_eq!(
            kh(&with_port),
            [khd("abeddbd31e78b12b84bb72178f9da77a87609068")],
            "the connection's port"
        );
        assert_eq!(
            kh(&q(&f, "v6")),
            [khd(&connection_hash(
                "laptop.example.org",
                "2001:db8::1",
                22,
                "alice",
                ""
            ))],
            "IP literals are normalised"
        );
        assert_eq!(
            kh(&q(&f, "lit")),
            [
                f.home().join(".ssh/kh%C").display().to_string(),
                f.home().join(".ssh/kh%C").display().to_string(),
            ],
            "%%C and a variable holding %C stay literal"
        );
        let canon = q(&f, "canon");
        assert_eq!(
            canon.user_known_hosts, None,
            "CanonicalizeHostname: %C unresolved"
        );
        assert!(canon.known_hosts_unresolved);
        assert_eq!(unavailable_tokens(&canon), ["%C"]);
        let mixed = q(&f, "mixed");
        assert_eq!(
            mixed.user_known_hosts, None,
            "one bad argument drops the option"
        );
        assert!(mixed.known_hosts_unresolved);

        let other = q(&f, "other");
        assert_eq!(
            other.global_known_hosts,
            Some(vec![KnownHostsFileOpt::Path(
                "/etc/ssh/kh-laptop.example.org-laptop-1000".into()
            )])
        );
        let hash = connection_hash("laptop.example.org", "other", 22, "alice", "");
        assert_eq!(
            id_paths(&other),
            [f.home()
                .join(format!(".ssh/id_{hash}"))
                .display()
                .to_string()]
        );
        assert_eq!(
            other.certificate_files,
            [f.home().join(".ssh/c_laptop").display().to_string()]
        );
        assert_eq!(
            other.identity_agent,
            Some(AgentSetting::Path(
                f.home().join(".agent-laptop").display().to_string()
            ))
        );
        assert!(other.warnings.is_empty(), "{:?}", other.warnings);
    }

    /// (hostile input) #7: a local host name that is not a plain name is never substituted:
    /// `%l`/`%L`/`%C` are unavailable, the defaults are read and the hop is marked to ask; an
    /// expansion longer than a path is dropped.
    #[test]
    fn hostile_local_host_names_and_overlong_paths() {
        let long = "a".repeat(300);
        let nul = format!("lap{}top", char::from(0));
        let rtl = format!("lap{}top", '\u{202e}');
        for name in [
            "a/b",
            "..",
            r"a\b",
            "%h",
            "-x",
            "",
            long.as_str(),
            "ноутбук",
            nul.as_str(),
            rtl.as_str(),
        ] {
            let mut f = Fixture::new();
            f.env.local_host = Some(name.to_owned());
            f.user_config(
                "Host h\n  UserKnownHostsFile ~/.ssh/kh.d/%C\n  GlobalKnownHostsFile /etc/ssh/%l\n  IdentityFile ~/.ssh/%L\n",
            );
            let c = q(&f, "h");
            assert_eq!(c.user_known_hosts, None, "{name:?}");
            assert_eq!(c.global_known_hosts, None, "{name:?}");
            assert!(c.known_hosts_unresolved, "{name:?}");
            assert!(c.identity_files.is_empty(), "{name:?}");
            assert_eq!(unavailable_tokens(&c), ["%L", "%C", "%l"], "{name:?}");
            assert!(unknown_tokens(&c).is_empty(), "{name:?}");
        }

        let f = with_host(Fixture::new());
        f.user_config(&format!(
            "Host h\n  IdentityFile ~/{}\n  UserKnownHostsFile ~/.ssh/{}\n",
            "%d".repeat(5000),
            "%l".repeat(5000)
        ));
        let c = q(&f, "h");
        assert!(c.identity_files.is_empty());
        assert_eq!(c.user_known_hosts, None);
        assert!(c.known_hosts_unresolved);
        let bad: Vec<&str> = c
            .warnings
            .iter()
            .filter_map(|w| match &w.kind {
                ConfigWarningCode::ExpansionTooLong { option } => Some(option.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(bad, ["IdentityFile", "UserKnownHostsFile"]);
        assert!(
            !c.warnings
                .iter()
                .any(|w| matches!(w.kind, ConfigWarningCode::BadLine { .. })),
            "a clear message, not `could not be parsed`"
        );
    }

    #[test]
    fn missing_main_file_yields_empty_config() {
        let f = Fixture::new();
        let p = f.load();
        assert!(p.files_read().is_empty());
        assert!(p.load_warnings().is_empty());
        assert_eq!(p.query("anything", &f.env).unwrap(), HostConfig::default());
    }
}
