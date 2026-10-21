//! Per-alias resolution (§7.4) with OpenSSH semantics:
//! - files in order user config → system config; options before the first `Host` apply to all;
//! - `Host` patterns are matched case-insensitively against the alias as typed; a matching
//!   negated pattern excludes the block;
//! - `Match` subset (`all`, `host` = HostName so far, `originalhost`, `user` = User so far or
//!   the local user, `localuser`), AND-combined, `!`-negatable; unsupported criteria ⇒ the block
//!   never matches (warning);
//! - `Include` inside an inactive block makes the included file never match
//!   (`SSHCONF_NEVERMATCH`); a `Host`/`Match` inside an included file does not change the
//!   including file's state;
//! - first obtained value wins; `IdentityFile`/`CertificateFile` accumulate; `ProxyJump` and
//!   `ProxyCommand` exclude each other (whichever comes first wins);
//! - expansion order: `HostName`'s own `%h`/`%%` → `valid_hostname` → file-option tokens.

use std::collections::HashMap;

use crate::ipc::dto::{ConfigWarning, ConfigWarningCode, SourceLocation};

use super::parser::{CritKind, Item, Key, OptLine, parse_bool, parse_time, warning};
use super::pattern::{ListMatch, host_block_matches, match_list};
use super::tokens::{TokenCtx, expand_hostname, expand_path, valid_env_name};
use super::validate::valid_hostname;
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
    pub user: Option<String>,
    /// Empty if `IdentityFile none` appeared anywhere in the applicable blocks.
    pub identity_files: Vec<IdentityFileOpt>,
    pub identities_only: Option<bool>,
    pub identity_agent: Option<AgentSetting>,
    pub proxy_jump: Option<ProxyJumpOpt>,
    /// Set only if `ProxyCommand` won over `ProxyJump` and is not `none` (⇒ `proxyCommandUnsupported`).
    pub proxy_command: Option<String>,
    pub user_known_hosts: Option<Vec<KnownHostsFileOpt>>,
    pub global_known_hosts: Option<Vec<KnownHostsFileOpt>>,
    /// A `UserKnownHostsFile`/`GlobalKnownHostsFile` argument could not be expanded (unsupported
    /// token such as `%C`/`%l`): that option falls back to the defaults (`unknownToken` warning)
    /// and `accept-new`/`no` must ask instead (the files the user meant are not read).
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
}

/// Per-connection values that take precedence over the config (OpenSSH applies the
/// command-line user and port before reading the config): `bob@host` makes `Match user`
/// and `%r` see `bob`, a bookmark port makes `%p` see that port.
#[derive(Debug, Clone, Copy, Default)]
pub struct QueryOverrides<'a> {
    pub user: Option<&'a str>,
    pub port: Option<u16>,
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

    fn walk<'a>(&self, items: &'a [Item], never_match: bool, c: &mut Collector<'a>) {
        let mut active = !never_match;
        for item in items {
            match item {
                Item::Host { patterns, .. } => {
                    active = !never_match && host_block_matches(self.alias, patterns)
                }
                Item::Match {
                    criteria,
                    unsupported,
                    loc,
                } => {
                    if never_match {
                        active = false;
                        continue;
                    }
                    let supported_ok = criteria.iter().all(|cr| {
                        let ok = match cr.kind {
                            CritKind::All => true,
                            CritKind::Host => {
                                match_list(&self.host_so_far(c), &cr.list) == ListMatch::Positive
                            }
                            CritKind::OriginalHost => {
                                match_list(self.alias, &cr.list) == ListMatch::Positive
                            }
                            CritKind::User => {
                                match_list(&self.user_so_far(c), &cr.list) == ListMatch::Positive
                            }
                            CritKind::LocalUser => {
                                match_list(&self.env.local_user, &cr.list) == ListMatch::Positive
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
                }
                Item::Opt(o) => {
                    if active {
                        c.apply(o);
                    }
                }
                Item::Include { files } => {
                    let child_never = never_match || !active;
                    for f in files {
                        self.walk(&f.items, child_never, c);
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
        user_override: overrides.user,
    };
    let mut c = Collector::default();
    for file in [&parsed.user, &parsed.system].into_iter().flatten() {
        walker.walk(&file.items, false, &mut c);
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
    cfg.user = c.get(Key::User).map(|o| o.first().to_owned());

    let host_for_tokens = cfg.hostname.clone().unwrap_or_else(|| alias.to_owned());
    let remote_user = overrides
        .user
        .map(str::to_owned)
        .or_else(|| cfg.user.clone())
        .unwrap_or_else(|| env.local_user.clone());
    let key_alias = cfg
        .host_key_alias
        .clone()
        .unwrap_or_else(|| alias.to_owned());
    let ctx = TokenCtx {
        home: &env.home,
        local_user: &env.local_user,
        host: &host_for_tokens,
        port: overrides.port.or(cfg.port).unwrap_or(22),
        remote_user: &remote_user,
        alias,
        key_alias: &key_alias,
        env: env.env.as_ref(),
    };
    let expand =
        |value: &str, loc: &SourceLocation, warnings: &mut Vec<ConfigWarning>| -> Option<String> {
            match expand_path(value, &ctx) {
                Ok(v) => Some(v),
                Err(token) => {
                    unknown_token(warnings, loc, token);
                    None
                }
            }
        };

    if !c.identity_none {
        for o in &c.identity_files {
            if let Some(path) = expand(o.first(), &o.loc, &mut warnings) {
                cfg.identity_files.push(IdentityFileOpt {
                    path,
                    explicit: true,
                });
            }
        }
    }
    for o in &c.certificate_files {
        if let Some(path) = expand(o.first(), &o.loc, &mut warnings) {
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
            expand(v, &o.loc, &mut warnings).map(AgentSetting::Path)
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
                } else if let Some(p) = expand(arg, &o.loc, &mut warnings) {
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

    #[test]
    fn negation_question_mark_and_case_insensitivity() {
        let f = Fixture::new();
        f.user_config("Host web? !web3\n  Port 8022\nHOST Bastion\n  user jump\n");
        assert_eq!(q(&f, "web1").port, Some(8022));
        assert_eq!(q(&f, "web3").port, None);
        assert_eq!(q(&f, "web10").port, None);
        assert_eq!(q(&f, "WEB2").port, Some(8022));
        assert_eq!(q(&f, "bastion").user.as_deref(), Some("jump"));
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
        f.user_config("Host t\n  IdentityFile ${HOME}/.ssh/id_t\n  IdentityFile ${NOPE}/x\n  IdentityFile ~/.ssh/%C\n");
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
        assert_eq!(tokens, vec!["${NOPE}".to_owned(), "%C".to_owned()]);
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

    /// `%k` is supported; an argument that cannot be expanded never leaves an empty list
    /// (no files read, nothing learned): the option falls back to the defaults, warns, and marks
    /// the hop so `accept-new`/`no` ask.
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
        assert!(matches!(
            &c.warnings[..],
            [ConfigWarning { kind: ConfigWarningCode::UnknownToken { token }, .. }] if token == "%C"
        ));
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

    #[test]
    fn missing_main_file_yields_empty_config() {
        let f = Fixture::new();
        let p = f.load();
        assert!(p.files_read().is_empty());
        assert!(p.load_warnings().is_empty());
        assert_eq!(p.query("anything", &f.env).unwrap(), HostConfig::default());
    }
}
