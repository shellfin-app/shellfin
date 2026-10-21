//! Target resolution (§5.1): `Target` → `ResolvedTarget { label, recent, hops }` (jumps first,
//! destination last).
//!
//! 1. `sshConfig{alias}` → `query(alias)`; defaults hostname = alias, user = local user, port 22.
//! 2. `bookmark{id}` → bookmark fields win; with `useSshConfig` the config (queried with the
//!    bookmark host) fills empty fields. `auth`: auto | key | agent | password.
//! 3. `quick{spec}` → `[user@]host[:port]`, then like `sshConfig` (the spec's user/port win).
//! 4. `draft{bookmark, password}` → as `bookmark`, in memory; recorded in recents as `quick`.
//! 5. Every host name passes `valid_hostname` (config `HostName`/`HostKeyAlias` are validated by
//!    the sshconfig resolver after their own `%h` expansion).
//! 6. ProxyJump: from the bookmark, else the config (`none` = no jump). For `a,b,c`: `a` is
//!    resolved recursively (its own ProxyJump honoured), `b`, `c` without their own ProxyJump
//!    (OpenSSH `ssh -J`). Cycle → `proxyJumpCycle`; more than 8 jumps → `proxyJumpTooDeep`;
//!    `ProxyCommand` without ProxyJump → `proxyCommandUnsupported`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::{AppError, EntityKind, ErrorCode};
use crate::ipc::dto::{
    AuthConfig, AuthInput, Bookmark, BookmarkInput, JumpRef, RecentTarget, Settings, Target,
};
use crate::ipc::secret::Secret;
use crate::secrets;
use crate::sftp::throttle::LimitOverrides;
use crate::ssh::auth::{AuthPlan, KeychainBinding, KeychainRef};
use crate::ssh::keys::IdentityFile;
use crate::sshconfig::validate::{parse_host_spec, valid_hostname};
use crate::sshconfig::{
    AgentSetting, ConfigEnv, HostConfig, KnownHostsFileOpt, ParsedConfig, ProxyJumpOpt, QueryError,
    QueryOverrides, StrictMode,
};
use crate::storage::bookmarks::PasswordBinding;

/// More jump hops than this ⇒ `proxyJumpTooDeep`.
pub const MAX_JUMPS: usize = 8;

/// Which agent endpoint to use for a hop (§5.5). `Default` = env `SSH_AUTH_SOCK`, then the
/// Windows OpenSSH pipe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentChoice {
    Default,
    Disabled,
    /// An expanded endpoint (pipe name or socket path) from `IdentityAgent`.
    Endpoint(String),
}

/// A known_hosts file option after defaults were applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KnownHostsFile {
    Path(PathBuf),
    /// `none`, `/dev/null`, `NUL`: neither read nor written.
    Disabled,
}

#[derive(Debug, Clone)]
pub struct ResolvedHop {
    /// Display label: config alias, bookmark name or spec host.
    pub label: String,
    /// Alias used for config lookups (`Host` matching).
    pub config_alias: String,
    pub hostname: String,
    pub port: u16,
    pub user: String,
    pub auth: AuthPlan,
    /// Explicit identity files (config `IdentityFile` / bookmark key); empty ⇒ defaults.
    pub identity_files: Vec<IdentityFile>,
    pub identities_only: bool,
    pub identity_agent: AgentChoice,
    pub add_keys_to_agent: bool,
    pub host_key_alias: Option<String>,
    pub user_known_hosts: Vec<KnownHostsFile>,
    pub global_known_hosts: Vec<KnownHostsFile>,
    pub strict: StrictMode,
    pub hash_known_hosts: bool,
    pub host_key_algorithms_add_ssh_rsa: bool,
    pub keepalive_interval: Duration,
    pub keepalive_max: usize,
    pub connect_timeout: Duration,
    pub keychain: Option<KeychainRef>,
    pub pubkey_enabled: bool,
    pub password_enabled: bool,
    pub kbd_interactive_enabled: bool,
}

impl ResolvedHop {
    /// The keychain binding of this hop (§5.6).
    pub fn binding(&self) -> PasswordBinding {
        PasswordBinding {
            hostname: self.hostname.clone(),
            port: self.port,
            user: self.user.clone(),
        }
    }

    /// `user@host[:port]` for recents/labels; an IPv6 literal is bracketed so the text is also
    /// a valid quick spec (drafts are recorded as `quick{spec}`).
    pub fn user_at_host(&self) -> String {
        let host = if self.hostname.contains(':') {
            format!("[{}]", self.hostname)
        } else {
            self.hostname.clone()
        };
        if self.port == 22 {
            format!("{}@{host}", self.user)
        } else {
            format!("{}@{host}:{}", self.user, self.port)
        }
    }
}

#[derive(Debug)]
pub struct ResolvedTarget {
    /// Session label (destination label).
    pub label: String,
    /// Secret-free target for recents (§7.3).
    pub recent: RecentTarget,
    /// Jumps first, destination last (never empty).
    pub hops: Vec<ResolvedHop>,
    /// Bookmark `startDir` (validated by `stat` after SFTP init, §6.1).
    pub start_dir: Option<String>,
    /// Bookmark id when the destination is a stored bookmark (password store updates).
    pub bookmark_id: Option<String>,
    /// The stored bookmark as it was when the session was opened (live-edit baseline).
    pub bookmark: Option<Bookmark>,
    /// Draft password for the destination hop (`SessionSecrets`, §5.1 rule 4).
    pub draft_password: Option<Secret<String>>,
    /// Per-bookmark bandwidth overrides (KiB/s; `None` = global setting).
    pub limits: crate::sftp::throttle::LimitOverrides,
}

impl ResolvedTarget {
    /// "via" text for recents: the jump labels, comma-separated.
    pub fn via(&self) -> Option<String> {
        let jumps: Vec<&str> = self
            .hops
            .iter()
            .rev()
            .skip(1)
            .rev()
            .map(|h| h.label.as_str())
            .collect();
        (!jumps.is_empty()).then(|| jumps.join(", "))
    }

    pub fn destination(&self) -> Option<&ResolvedHop> {
        self.hops.last()
    }
}

/// Lookups the resolver needs from the bookmark store.
pub trait BookmarkLookup {
    fn bookmark(&self, id: &str) -> Option<Bookmark>;
    fn stored_binding(&self, id: &str) -> Option<PasswordBinding>;
}

impl BookmarkLookup for crate::storage::bookmarks::BookmarkStore {
    fn bookmark(&self, id: &str) -> Option<Bookmark> {
        self.get(id)
    }
    fn stored_binding(&self, id: &str) -> Option<PasswordBinding> {
        self.password_binding(id)
    }
}

/// Everything resolution reads (all in memory; the caller loads them).
pub struct ResolveInput<'a> {
    pub config: &'a ParsedConfig,
    pub env: &'a ConfigEnv,
    pub bookmarks: &'a dyn BookmarkLookup,
    pub settings: &'a Settings,
    /// `secrets::availability().is_ok()` (evaluated once per process).
    pub keychain_available: bool,
}

/// A bookmark-like description (stored bookmark or in-memory draft).
struct BookmarkView {
    id: Option<String>,
    name: String,
    host: String,
    port: Option<u16>,
    user: Option<String>,
    auth: AuthPlan,
    proxy_jump: Option<JumpRef>,
    start_dir: Option<String>,
    use_ssh_config: bool,
    upload_limit_kibps: Option<u32>,
    download_limit_kibps: Option<u32>,
}

impl BookmarkView {
    fn stored(b: Bookmark, home: &Path) -> Self {
        let auth = match b.auth {
            AuthConfig::Auto => AuthPlan::Auto,
            AuthConfig::Key { identity_file } => AuthPlan::Key(expand_tilde(&identity_file, home)),
            AuthConfig::Password { .. } => AuthPlan::Password,
            AuthConfig::Agent => AuthPlan::Agent,
        };
        Self {
            id: Some(b.id),
            name: b.name,
            host: b.host,
            port: b.port,
            user: b.user,
            auth,
            proxy_jump: b.proxy_jump,
            start_dir: b.start_dir,
            use_ssh_config: b.use_ssh_config,
            upload_limit_kibps: b.upload_limit_kibps,
            download_limit_kibps: b.download_limit_kibps,
        }
    }

    fn draft(b: BookmarkInput, home: &Path) -> Self {
        let auth = match b.auth {
            AuthInput::Auto => AuthPlan::Auto,
            AuthInput::Key { identity_file } => AuthPlan::Key(expand_tilde(&identity_file, home)),
            AuthInput::Password => AuthPlan::Password,
            AuthInput::Agent => AuthPlan::Agent,
        };
        Self {
            id: None,
            name: b.name,
            host: b.host,
            port: b.port,
            user: b.user,
            auth,
            proxy_jump: b.proxy_jump,
            start_dir: b.start_dir,
            use_ssh_config: b.use_ssh_config,
            upload_limit_kibps: b.upload_limit_kibps,
            download_limit_kibps: b.download_limit_kibps,
        }
    }
}

/// `~` / `~/…` → home (bookmark identity files; config values are expanded by the resolver).
pub fn expand_tilde(path: &str, home: &Path) -> PathBuf {
    if path == "~" {
        return home.to_path_buf();
    }
    match path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        Some(rest) => home.join(rest),
        None => PathBuf::from(path),
    }
}

/// One element of a jump list, before resolution.
#[derive(Debug, Clone)]
enum JumpElem {
    /// Config alias or spec `[user@]host[:port]` (looked up in the config by host).
    Spec {
        host: String,
        user: Option<String>,
        port: Option<u16>,
        label: String,
    },
    Bookmark(String),
}

struct Resolver<'a> {
    input: &'a ResolveInput<'a>,
    /// Cycle detection keys of the chain currently being resolved.
    stack: Vec<(String, String)>, // (key, label)
}

fn invalid_hostname(value: impl Into<String>) -> AppError {
    AppError::new(ErrorCode::InvalidHostname {
        value: value.into(),
    })
}

/// Query with the connection's own user/port applied first (OpenSSH command-line
/// semantics for `Match user`, `%r`, `%p`).
fn query(
    cfg: &ParsedConfig,
    alias: &str,
    env: &ConfigEnv,
    user: Option<&str>,
    port: Option<u16>,
) -> Result<HostConfig, AppError> {
    // `%r` ends up in local file paths (`IdentityFile ~/.ssh/id_%r`); a user with a path
    // separator (`DOMAIN\alice`, or a hostile imported bookmark) is not used for that.
    let user = user.filter(|u| !u.contains(['/', '\\']) && !u.contains(".."));
    cfg.query_with(alias, env, QueryOverrides { user, port })
        .map_err(|e| match e {
            QueryError::InvalidHostname { value } => invalid_hostname(value),
        })
}

impl<'a> Resolver<'a> {
    fn enter(&mut self, key: String, label: &str) -> Result<(), AppError> {
        if self.stack.iter().any(|(k, _)| *k == key) {
            let mut chain: Vec<String> = self.stack.iter().map(|(_, l)| l.clone()).collect();
            chain.push(label.to_owned());
            return Err(AppError::new(ErrorCode::ProxyJumpCycle { chain }));
        }
        // Bound the recursion *before* descending. A chain of thousands of jumps (an
        // imported bookmark file, a generated ssh_config) would otherwise overflow the stack
        // and abort the process; the destination plus MAX_JUMPS hops is the deepest valid chain.
        if self.stack.len() > MAX_JUMPS {
            return Err(AppError::new(ErrorCode::ProxyJumpTooDeep));
        }
        self.stack.push((key, label.to_owned()));
        Ok(())
    }

    fn leave(&mut self) {
        self.stack.pop();
    }

    /// Hops for a config alias / spec host (`honour_jump` = resolve its own ProxyJump).
    fn resolve_spec(
        &mut self,
        host: &str,
        user: Option<&str>,
        port: Option<u16>,
        label: &str,
        honour_jump: bool,
    ) -> Result<Vec<ResolvedHop>, AppError> {
        if !valid_hostname(host) {
            return Err(invalid_hostname(host));
        }
        self.enter(format!("host:{}", host.to_ascii_lowercase()), label)?;
        let cfg = query(self.input.config, host, self.input.env, user, port)?;
        let mut hops = Vec::new();
        if honour_jump {
            hops = self.jumps_from_config(&cfg, host)?;
        }
        let hop = self.make_hop(
            label,
            host,
            &cfg,
            Some(host),
            user,
            port,
            AuthPlan::Auto,
            None,
        )?;
        hops.push(hop);
        self.leave();
        Ok(hops)
    }

    fn jumps_from_config(
        &mut self,
        cfg: &HostConfig,
        alias: &str,
    ) -> Result<Vec<ResolvedHop>, AppError> {
        match &cfg.proxy_jump {
            Some(ProxyJumpOpt::Hops(list)) => {
                let elems = list
                    .iter()
                    .flat_map(|s| s.split(','))
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(spec_elem)
                    .collect::<Result<Vec<_>, _>>()?;
                self.resolve_jump_list(elems)
            }
            Some(ProxyJumpOpt::Disabled) => Ok(Vec::new()),
            None if cfg.proxy_command.is_some() => {
                Err(AppError::new(ErrorCode::ProxyCommandUnsupported {
                    alias: alias.to_owned(),
                }))
            }
            None => Ok(Vec::new()),
        }
    }

    /// `a,b,c`: `a` with its own ProxyJump, `b`/`c` without.
    fn resolve_jump_list(&mut self, elems: Vec<JumpElem>) -> Result<Vec<ResolvedHop>, AppError> {
        let mut hops = Vec::new();
        for (i, elem) in elems.into_iter().enumerate() {
            let honour = i == 0;
            let mut part = match elem {
                JumpElem::Spec {
                    host,
                    user,
                    port,
                    label,
                } => self.resolve_spec(&host, user.as_deref(), port, &label, honour)?,
                JumpElem::Bookmark(id) => self.resolve_bookmark_id(&id, honour)?,
            };
            hops.append(&mut part);
            if hops.len() > MAX_JUMPS {
                return Err(AppError::new(ErrorCode::ProxyJumpTooDeep));
            }
        }
        Ok(hops)
    }

    /// `connection.readSshConfig = false`: an ssh-config alias (target or jump)
    /// is `notFound` instead of silently becoming a plain hostname.
    fn require_config_alias(&self, alias: &str) -> Result<(), AppError> {
        if self.input.settings.connection.read_ssh_config {
            return Ok(());
        }
        Err(AppError::new(ErrorCode::NotFound {
            what: EntityKind::SshConfigHost,
            id: alias.to_owned(),
        }))
    }

    fn jump_ref_elems(&self, r: &JumpRef) -> Result<Vec<JumpElem>, AppError> {
        match r {
            JumpRef::SshConfig { alias } => self.require_config_alias(alias).map(|()| {
                vec![JumpElem::Spec {
                    host: alias.clone(),
                    user: None,
                    port: None,
                    label: alias.clone(),
                }]
            }),
            JumpRef::Bookmark { id } => Ok(vec![JumpElem::Bookmark(id.clone())]),
            JumpRef::Spec { spec } => spec
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(spec_elem)
                .collect(),
        }
    }

    fn resolve_bookmark_id(
        &mut self,
        id: &str,
        honour_jump: bool,
    ) -> Result<Vec<ResolvedHop>, AppError> {
        let b = self.input.bookmarks.bookmark(id).ok_or_else(|| {
            AppError::new(ErrorCode::NotFound {
                what: EntityKind::Bookmark,
                id: id.to_owned(),
            })
        })?;
        let view = BookmarkView::stored(b, &self.input.env.home);
        self.resolve_bookmark(view, honour_jump)
    }

    fn resolve_bookmark(
        &mut self,
        b: BookmarkView,
        honour_jump: bool,
    ) -> Result<Vec<ResolvedHop>, AppError> {
        if !valid_hostname(&b.host) {
            return Err(invalid_hostname(b.host));
        }
        let key = match &b.id {
            Some(id) => format!("bookmark:{id}"),
            None => "draft".to_owned(),
        };
        self.enter(key, &b.name)?;
        let cfg = if b.use_ssh_config {
            query(
                self.input.config,
                &b.host,
                self.input.env,
                b.user.as_deref(),
                b.port,
            )?
        } else {
            HostConfig::default()
        };
        let mut hops = Vec::new();
        if honour_jump {
            match &b.proxy_jump {
                Some(r) => {
                    let elems = self.jump_ref_elems(r)?;
                    hops = self.resolve_jump_list(elems)?;
                }
                None => hops = self.jumps_from_config(&cfg, &b.host)?,
            }
        }
        // Only `password` bookmarks can carry a saved password (`passwordSaved`, §7.2).
        let keychain = match &b.id {
            Some(id) if self.input.keychain_available && matches!(b.auth, AuthPlan::Password) => {
                Some(KeychainRef {
                    account: secrets::bookmark_account(id),
                    // No stored binding ⇒ `Missing` (a mismatch; still writable).
                    binding: self
                        .input
                        .bookmarks
                        .stored_binding(id)
                        .map_or(KeychainBinding::Missing, KeychainBinding::Stored),
                })
            }
            _ => None,
        };
        let config_alias = if b.use_ssh_config {
            Some(b.host.as_str())
        } else {
            None
        };
        // Bookmark host is the alias for config lookups; without useSshConfig it is the hostname.
        let hop = self.make_hop(
            &b.name,
            &b.host,
            &cfg,
            config_alias,
            b.user.as_deref(),
            b.port,
            b.auth,
            Some(keychain),
        )?;
        hops.push(hop);
        self.leave();
        Ok(hops)
    }

    /// Build one hop. `explicit_user`/`explicit_port` win over the config.
    /// `bookmark_keychain`: `Some(..)` for bookmark hops (possibly `Some(None)` = no keychain),
    /// `None` for config/spec hops (keychain account `sshconfig:<user>@<hostname>:<port>`).
    #[allow(clippy::too_many_arguments)]
    fn make_hop(
        &self,
        label: &str,
        host: &str,
        cfg: &HostConfig,
        config_alias: Option<&str>,
        explicit_user: Option<&str>,
        explicit_port: Option<u16>,
        plan: AuthPlan,
        bookmark_keychain: Option<Option<KeychainRef>>,
    ) -> Result<ResolvedHop, AppError> {
        let input = self.input;
        let env = input.env;
        let settings = &input.settings.connection;
        let hostname = cfg.hostname.clone().unwrap_or_else(|| host.to_owned());
        if !valid_hostname(&hostname) {
            return Err(invalid_hostname(hostname));
        }
        let user = explicit_user
            .map(str::to_owned)
            .or_else(|| cfg.user.clone())
            .unwrap_or_else(|| env.local_user.clone());
        let port = explicit_port.or(cfg.port).unwrap_or(22);

        let mut identity_files: Vec<IdentityFile> = Vec::new();
        if let AuthPlan::Key(path) = &plan {
            identity_files.push(IdentityFile {
                path: path.clone(),
                explicit: true,
            });
        } else {
            identity_files.extend(cfg.identity_files.iter().map(|f| IdentityFile {
                path: PathBuf::from(&f.path),
                explicit: f.explicit,
            }));
        }

        let identity_agent = match &cfg.identity_agent {
            None => AgentChoice::Default,
            Some(AgentSetting::Disabled) => AgentChoice::Disabled,
            Some(AgentSetting::EnvVar(name)) => match (env.env)(name) {
                Some(v) if !v.is_empty() => AgentChoice::Endpoint(v),
                _ => AgentChoice::Disabled,
            },
            Some(AgentSetting::Path(p)) => AgentChoice::Endpoint(p.clone()),
        };

        let to_files = |opts: &Option<Vec<KnownHostsFileOpt>>,
                        defaults: Vec<PathBuf>|
         -> Vec<KnownHostsFile> {
            match opts {
                Some(list) => list
                    .iter()
                    .map(|o| match o {
                        KnownHostsFileOpt::Disabled => KnownHostsFile::Disabled,
                        KnownHostsFileOpt::Path(p) => KnownHostsFile::Path(PathBuf::from(p)),
                    })
                    .collect(),
                None => defaults.into_iter().map(KnownHostsFile::Path).collect(),
            }
        };
        let user_known_hosts = to_files(
            &cfg.user_known_hosts,
            vec![
                env.user_ssh_dir.join("known_hosts"),
                env.user_ssh_dir.join("known_hosts2"),
            ],
        );
        let global_known_hosts = to_files(
            &cfg.global_known_hosts,
            vec![
                env.system_ssh_dir.join("ssh_known_hosts"),
                env.system_ssh_dir.join("ssh_known_hosts2"),
            ],
        );

        let keychain = match bookmark_keychain {
            Some(k) => k,
            None if input.keychain_available => Some(KeychainRef {
                account: secrets::ssh_config_account(&user, &hostname, port),
                binding: KeychainBinding::ImpliedByAccount,
            }),
            None => None,
        };

        Ok(ResolvedHop {
            label: label.to_owned(),
            config_alias: config_alias.unwrap_or(host).to_owned(),
            hostname,
            port,
            user,
            auth: plan,
            identity_files,
            identities_only: cfg.identities_only.unwrap_or(false),
            identity_agent,
            add_keys_to_agent: cfg.add_keys_to_agent.unwrap_or(false),
            host_key_alias: cfg.host_key_alias.clone(),
            user_known_hosts,
            global_known_hosts,
            // The known_hosts files the user configured could not be expanded, so the
            // defaults are read instead; never auto-accept on that basis.
            strict: match cfg.strict_host_key_checking.unwrap_or(StrictMode::Ask) {
                StrictMode::AcceptNew | StrictMode::No if cfg.known_hosts_unresolved => {
                    StrictMode::Ask
                }
                s => s,
            },
            hash_known_hosts: cfg.hash_known_hosts.unwrap_or(false),
            host_key_algorithms_add_ssh_rsa: cfg.host_key_algorithms_add_ssh_rsa,
            keepalive_interval: Duration::from_secs(u64::from(
                cfg.server_alive_interval
                    .filter(|v| *v > 0)
                    .unwrap_or(settings.keepalive_interval_secs),
            )),
            keepalive_max: usize::try_from(
                cfg.server_alive_count_max
                    .unwrap_or(settings.keepalive_count_max),
            )
            .unwrap_or(3),
            connect_timeout: Duration::from_secs(u64::from(
                cfg.connect_timeout
                    .filter(|v| *v > 0)
                    .unwrap_or(settings.connect_timeout_secs),
            )),
            keychain,
            pubkey_enabled: cfg.pubkey_authentication.unwrap_or(true),
            password_enabled: cfg.password_authentication.unwrap_or(true),
            kbd_interactive_enabled: cfg.kbd_interactive_authentication.unwrap_or(true),
        })
    }
}

fn spec_elem(s: &str) -> Result<JumpElem, AppError> {
    let spec = parse_host_spec(s)
        .ok_or_else(|| AppError::new(ErrorCode::InvalidHostSpec { spec: s.to_owned() }))?;
    Ok(JumpElem::Spec {
        label: spec.host.clone(),
        host: spec.host,
        user: spec.user,
        port: spec.port,
    })
}

fn finish(hops: Vec<ResolvedHop>) -> Result<Vec<ResolvedHop>, AppError> {
    if hops.len() > MAX_JUMPS + 1 {
        return Err(AppError::new(ErrorCode::ProxyJumpTooDeep));
    }
    if hops.is_empty() {
        return Err(AppError::internal("resolution produced no hops"));
    }
    Ok(hops)
}

/// Resolve a session target (§5.1). Errors are command errors of `session_open`.
pub fn resolve(target: Target, input: &ResolveInput<'_>) -> Result<ResolvedTarget, AppError> {
    let mut r = Resolver {
        input,
        stack: Vec::new(),
    };
    match target {
        Target::SshConfig { alias } => {
            r.require_config_alias(&alias)?;
            let hops = finish(r.resolve_spec(&alias, None, None, &alias, true)?)?;
            Ok(ResolvedTarget {
                label: alias.clone(),
                recent: RecentTarget::SshConfig { alias },
                hops,
                start_dir: None,
                bookmark_id: None,
                bookmark: None,
                draft_password: None,
                limits: LimitOverrides::default(),
            })
        }
        Target::Quick { spec } => {
            let parsed = parse_host_spec(&spec)
                .ok_or_else(|| AppError::new(ErrorCode::InvalidHostSpec { spec: spec.clone() }))?;
            let hops = finish(r.resolve_spec(
                &parsed.host,
                parsed.user.as_deref(),
                parsed.port,
                &parsed.host,
                true,
            )?)?;
            Ok(ResolvedTarget {
                label: parsed.host.clone(),
                recent: RecentTarget::Quick {
                    spec: spec.trim().to_owned(),
                },
                hops,
                start_dir: None,
                bookmark_id: None,
                bookmark: None,
                draft_password: None,
                limits: LimitOverrides::default(),
            })
        }
        Target::Bookmark { id } => {
            let b = input.bookmarks.bookmark(&id).ok_or_else(|| {
                AppError::new(ErrorCode::NotFound {
                    what: EntityKind::Bookmark,
                    id: id.clone(),
                })
            })?;
            let snapshot = b.clone();
            let view = BookmarkView::stored(b, &input.env.home);
            let label = view.name.clone();
            let start_dir = view.start_dir.clone();
            let (upload_kibps, download_kibps) =
                (view.upload_limit_kibps, view.download_limit_kibps);
            let hops = finish(r.resolve_bookmark(view, true)?)?;
            Ok(ResolvedTarget {
                label,
                recent: RecentTarget::Bookmark { id: id.clone() },
                hops,
                start_dir,
                bookmark_id: Some(id),
                bookmark: Some(snapshot),
                draft_password: None,
                limits: LimitOverrides {
                    upload_kibps,
                    download_kibps,
                },
            })
        }
        Target::Draft { bookmark, password } => {
            let view = BookmarkView::draft(*bookmark, &input.env.home);
            let label = if view.name.trim().is_empty() {
                view.host.clone()
            } else {
                view.name.clone()
            };
            let start_dir = view.start_dir.clone();
            let (upload_kibps, download_kibps) =
                (view.upload_limit_kibps, view.download_limit_kibps);
            let hops = finish(r.resolve_bookmark(view, true)?)?;
            let dest = hops
                .last()
                .map(ResolvedHop::user_at_host)
                .unwrap_or_default();
            Ok(ResolvedTarget {
                label,
                recent: RecentTarget::Quick { spec: dest },
                hops,
                start_dir,
                bookmark_id: None,
                bookmark: None,
                draft_password: password,
                limits: LimitOverrides {
                    upload_kibps,
                    download_kibps,
                },
            })
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use std::collections::HashMap;

    use super::*;
    use crate::sshconfig::{StdFsSource, load};
    use crate::storage::settings::default_settings;

    struct Books(HashMap<String, Bookmark>);
    impl BookmarkLookup for Books {
        fn bookmark(&self, id: &str) -> Option<Bookmark> {
            self.0.get(id).cloned()
        }
        fn stored_binding(&self, _id: &str) -> Option<PasswordBinding> {
            None
        }
    }

    fn bm(id: &str, host: &str, jump: Option<JumpRef>, use_cfg: bool) -> Bookmark {
        Bookmark {
            id: id.into(),
            name: format!("bm-{id}"),
            host: host.into(),
            port: None,
            user: None,
            auth: AuthConfig::Auto,
            proxy_jump: jump,
            group_id: None,
            tags: vec![],
            start_dir: Some("/srv".into()),
            use_ssh_config: use_cfg,
            color: None,
            order: 0,
            upload_limit_kibps: None,
            download_limit_kibps: None,
            confirm_connect: false,
            startup_command: None,
            notes: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    struct Fx {
        _dir: tempfile::TempDir,
        env: ConfigEnv,
        cfg: ParsedConfig,
    }

    fn fx(config: &str) -> Fx {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_path_buf();
        std::fs::create_dir(home.join(".ssh")).unwrap();
        std::fs::write(home.join(".ssh/config"), config).unwrap();
        let mut env = ConfigEnv::new(
            home.clone(),
            "alice".into(),
            home.join("etc"),
            home.join("etc/ssh_config"),
        );
        env.env = std::sync::Arc::new(|name| {
            (name == "AGENT_SOCK").then(|| "/run/agent.sock".to_owned())
        });
        let cfg = load(&StdFsSource, &env);
        Fx {
            _dir: dir,
            env,
            cfg,
        }
    }

    fn run(f: &Fx, books: &Books, target: Target) -> Result<ResolvedTarget, AppError> {
        let settings = default_settings();
        let input = ResolveInput {
            config: &f.cfg,
            env: &f.env,
            bookmarks: books,
            settings: &settings,
            keychain_available: true,
        };
        resolve(target, &input)
    }

    fn no_books() -> Books {
        Books(HashMap::new())
    }

    #[test]
    fn config_alias_defaults_and_precedence() {
        let f = fx(
            "Host web\n  HostName web.example.com\n  Port 2200\n  ServerAliveInterval 30\n\nHost *\n  User bob\n",
        );
        let t = run(
            &f,
            &no_books(),
            Target::SshConfig {
                alias: "web".into(),
            },
        )
        .unwrap();
        let h = &t.hops[0];
        assert_eq!(
            (h.hostname.as_str(), h.port, h.user.as_str()),
            ("web.example.com", 2200, "bob")
        );
        assert_eq!(h.keepalive_interval, Duration::from_secs(30));
        assert_eq!(h.keepalive_max, 3);
        assert_eq!(h.strict, StrictMode::Ask);
        assert_eq!(
            h.keychain.as_ref().unwrap().account,
            "sshconfig:bob@web.example.com:2200"
        );
        assert_eq!(
            t.recent,
            RecentTarget::SshConfig {
                alias: "web".into()
            }
        );
        // Unknown alias: hostname = alias, local user, port 22.
        let t = run(
            &f,
            &no_books(),
            Target::SshConfig {
                alias: "plain".into(),
            },
        )
        .unwrap();
        assert_eq!(
            (
                t.hops[0].hostname.as_str(),
                t.hops[0].port,
                t.hops[0].user.as_str()
            ),
            ("plain", 22, "bob")
        );
    }

    #[test]
    fn config_aliases_are_not_found_when_reading_ssh_config_is_off() {
        // With `readSshConfig = false` the (empty) config must not turn an alias into a
        // plain hostname, neither as the target nor as a jump.
        let f = fx("");
        let mut settings = default_settings();
        settings.connection.read_ssh_config = false;
        let input = ResolveInput {
            config: &f.cfg,
            env: &f.env,
            bookmarks: &no_books(),
            settings: &settings,
            keychain_available: true,
        };
        let not_found = |err: AppError| {
            assert!(
                matches!(
                    err.code(),
                    ErrorCode::NotFound {
                        what: EntityKind::SshConfigHost,
                        id,
                    } if id == "web"
                ),
                "{err:?}"
            );
        };
        not_found(
            resolve(
                Target::SshConfig {
                    alias: "web".into(),
                },
                &input,
            )
            .unwrap_err(),
        );
        let r = Resolver {
            input: &input,
            stack: Vec::new(),
        };
        not_found(
            r.jump_ref_elems(&JumpRef::SshConfig {
                alias: "web".into(),
            })
            .unwrap_err(),
        );
        // Plain specs are unaffected.
        assert!(
            r.jump_ref_elems(&JumpRef::Spec { spec: "web".into() })
                .is_ok()
        );
    }

    #[test]
    fn quick_spec_including_ipv6() {
        let f = fx("Host db\n  User admin\n  Port 2222\n");
        let t = run(
            &f,
            &no_books(),
            Target::Quick {
                spec: "root@db:22".into(),
            },
        )
        .unwrap();
        assert_eq!((t.hops[0].user.as_str(), t.hops[0].port), ("root", 22));
        let t = run(
            &f,
            &no_books(),
            Target::Quick {
                spec: "[2001:db8::1]:2022".into(),
            },
        )
        .unwrap();
        assert_eq!(
            (
                t.hops[0].hostname.as_str(),
                t.hops[0].port,
                t.hops[0].user.as_str()
            ),
            ("2001:db8::1", 2022, "alice")
        );
        for bad in ["", "a b", "host:99999", "2001:db8::1"] {
            let err = run(&f, &no_books(), Target::Quick { spec: bad.into() }).unwrap_err();
            assert!(
                matches!(err.kind, ErrorCode::InvalidHostSpec { .. }),
                "{bad:?}: {err:?}"
            );
        }
    }

    #[test]
    fn bookmark_fields_win_and_config_fills_gaps() {
        let f = fx(
            "Host nas\n  HostName nas.example.com\n  User admin\n  Port 2222\n  IdentityFile ~/.ssh/nas_key\n",
        );
        let mut b = bm("b1", "nas", None, true);
        b.user = Some("carol".into());
        let books = Books(HashMap::from([("b1".into(), b)]));
        let t = run(&f, &books, Target::Bookmark { id: "b1".into() }).unwrap();
        let h = &t.hops[0];
        assert_eq!(
            (h.hostname.as_str(), h.port, h.user.as_str()),
            ("nas.example.com", 2222, "carol")
        );
        assert_eq!(h.identity_files.len(), 1);
        assert!(h.identity_files[0].path.ends_with("nas_key"));
        // `auto` bookmarks never use the keychain; `password` bookmarks do.
        assert!(h.keychain.is_none());
        assert_eq!(t.start_dir.as_deref(), Some("/srv"));
        let mut pw = bm("b2", "nas", None, true);
        pw.auth = AuthConfig::Password {
            password_saved: true,
        };
        let books_pw = Books(HashMap::from([("b2".into(), pw)]));
        let t = run(&f, &books_pw, Target::Bookmark { id: "b2".into() }).unwrap();
        assert_eq!(t.hops[0].keychain.as_ref().unwrap().account, "bookmark:b2");
        // Without useSshConfig the config is ignored entirely.
        let books = Books(HashMap::from([("b1".into(), bm("b1", "nas", None, false))]));
        let t = run(&f, &books, Target::Bookmark { id: "b1".into() }).unwrap();
        assert_eq!(
            (
                t.hops[0].hostname.as_str(),
                t.hops[0].port,
                t.hops[0].user.as_str()
            ),
            ("nas", 22, "alice")
        );
        let err = run(&f, &books, Target::Bookmark { id: "zz".into() }).unwrap_err();
        assert!(matches!(
            err.kind,
            ErrorCode::NotFound {
                what: EntityKind::Bookmark,
                ..
            }
        ));
    }

    #[test]
    fn proxyjump_rules() {
        let f = fx(concat!(
            "Host target\n  ProxyJump a,b\n",
            "Host a\n  HostName a.example.com\n  ProxyJump c\n",
            "Host b\n  HostName b.example.com\n  ProxyJump d\n",
            "Host c\n  HostName c.example.com\n",
            "Host d\n  HostName d.example.com\n",
            "Host loop1\n  ProxyJump loop2\nHost loop2\n  ProxyJump loop1\n",
            "Host nojump\n  ProxyJump none\n",
            "Host pc\n  ProxyCommand nc %h %p\n",
        ));
        let t = run(
            &f,
            &no_books(),
            Target::SshConfig {
                alias: "target".into(),
            },
        )
        .unwrap();
        let labels: Vec<&str> = t.hops.iter().map(|h| h.label.as_str()).collect();
        // `a` honours its own ProxyJump (c); `b` does not (d is skipped).
        assert_eq!(labels, ["c", "a", "b", "target"]);
        assert_eq!(t.via().as_deref(), Some("c, a, b"));
        let err = run(
            &f,
            &no_books(),
            Target::SshConfig {
                alias: "loop1".into(),
            },
        )
        .unwrap_err();
        assert!(
            matches!(err.kind, ErrorCode::ProxyJumpCycle { ref chain } if chain == &["loop1", "loop2", "loop1"]),
            "{err:?}"
        );
        assert_eq!(
            run(
                &f,
                &no_books(),
                Target::SshConfig {
                    alias: "nojump".into()
                }
            )
            .unwrap()
            .hops
            .len(),
            1
        );
        let err = run(&f, &no_books(), Target::SshConfig { alias: "pc".into() }).unwrap_err();
        assert_eq!(
            err.kind,
            ErrorCode::ProxyCommandUnsupported { alias: "pc".into() }
        );
        // Spec jump overrides user/port of the jump host.
        let t = run(
            &f,
            &no_books(),
            Target::Quick {
                spec: "nojump".into(),
            },
        )
        .unwrap();
        assert_eq!(t.hops.len(), 1);
    }

    /// (hostile input): a 10,000-element jump chain, from ssh_config and from bookmarks,
    /// fails with `proxyJumpTooDeep` instead of recursing until the stack overflows. Run on a
    /// small stack so a regression aborts the test instead of passing on a big one.
    #[test]
    fn very_long_jump_chains_fail_without_deep_recursion() {
        let mut cfg = String::new();
        for i in 0..10_000 {
            cfg.push_str(&format!("Host h{i}\n  ProxyJump h{}\n", i + 1));
        }
        let f = fx(&cfg);
        let mut map = HashMap::new();
        for i in 0..10_000 {
            let jump = Some(JumpRef::Bookmark {
                id: format!("b{}", i + 1),
            });
            map.insert(
                format!("b{i}"),
                bm(&format!("b{i}"), "h.example.com", jump, false),
            );
        }
        let books = Books(map);
        std::thread::scope(|s| {
            std::thread::Builder::new()
                .stack_size(256 * 1024)
                .spawn_scoped(s, || {
                    let err =
                        run(&f, &no_books(), Target::SshConfig { alias: "h0".into() }).unwrap_err();
                    assert_eq!(err.kind, ErrorCode::ProxyJumpTooDeep);
                    let err = run(&f, &books, Target::Bookmark { id: "b0".into() }).unwrap_err();
                    assert_eq!(err.kind, ErrorCode::ProxyJumpTooDeep);
                })
                .unwrap()
                .join()
                .unwrap();
        });
    }

    /// An unexpandable `UserKnownHostsFile` under `accept-new` reads the defaults and
    /// makes the hop ask; `%k` works.
    #[test]
    fn unexpandable_known_hosts_file_makes_accept_new_ask() {
        let f = fx(concat!(
            "Host c\n  StrictHostKeyChecking accept-new\n  UserKnownHostsFile ~/.ssh/known_hosts.d/%C\n",
            "Host k\n  StrictHostKeyChecking accept-new\n  UserKnownHostsFile ~/.ssh/known_hosts.d/%k\n",
            "Host n\n  StrictHostKeyChecking no\n  GlobalKnownHostsFile /etc/%l\n",
        ));
        let c = &run(&f, &no_books(), Target::SshConfig { alias: "c".into() })
            .unwrap()
            .hops[0];
        assert_eq!(c.strict, StrictMode::Ask);
        assert_eq!(
            c.user_known_hosts,
            vec![
                KnownHostsFile::Path(f.env.user_ssh_dir.join("known_hosts")),
                KnownHostsFile::Path(f.env.user_ssh_dir.join("known_hosts2")),
            ],
            "defaults, never an empty list"
        );
        let k = &run(&f, &no_books(), Target::SshConfig { alias: "k".into() })
            .unwrap()
            .hops[0];
        assert_eq!(k.strict, StrictMode::AcceptNew);
        let n = &run(&f, &no_books(), Target::SshConfig { alias: "n".into() })
            .unwrap()
            .hops[0];
        assert_eq!(n.strict, StrictMode::Ask);
    }

    #[test]
    fn too_deep_and_bookmark_jumps() {
        let mut cfg = String::new();
        for i in 0..10 {
            cfg.push_str(&format!("Host h{i}\n  ProxyJump h{}\n", i + 1));
        }
        cfg.push_str("Host h10\n  ProxyJump none\n");
        let f = fx(&cfg);
        let err = run(&f, &no_books(), Target::SshConfig { alias: "h0".into() }).unwrap_err();
        assert_eq!(err.kind, ErrorCode::ProxyJumpTooDeep);
        let ok = run(&f, &no_books(), Target::SshConfig { alias: "h2".into() }).unwrap();
        assert_eq!(ok.hops.len(), 9); // 8 jumps + destination is the maximum

        let books = Books(HashMap::from([
            ("j".into(), bm("j", "jump.example.com", None, false)),
            (
                "d".into(),
                bm(
                    "d",
                    "dest.example.com",
                    Some(JumpRef::Bookmark { id: "j".into() }),
                    false,
                ),
            ),
            (
                "x".into(),
                bm(
                    "x",
                    "x.example.com",
                    Some(JumpRef::Bookmark { id: "y".into() }),
                    false,
                ),
            ),
            (
                "y".into(),
                bm(
                    "y",
                    "y.example.com",
                    Some(JumpRef::Bookmark { id: "x".into() }),
                    false,
                ),
            ),
            (
                "s".into(),
                bm(
                    "s",
                    "s.example.com",
                    Some(JumpRef::Spec {
                        spec: "bob@jump.example.com:2022".into(),
                    }),
                    false,
                ),
            ),
        ]));
        let t = run(&f, &books, Target::Bookmark { id: "d".into() }).unwrap();
        assert_eq!(
            t.hops
                .iter()
                .map(|h| h.hostname.as_str())
                .collect::<Vec<_>>(),
            ["jump.example.com", "dest.example.com"]
        );
        let err = run(&f, &books, Target::Bookmark { id: "x".into() }).unwrap_err();
        assert!(matches!(err.kind, ErrorCode::ProxyJumpCycle { .. }));
        let t = run(&f, &books, Target::Bookmark { id: "s".into() }).unwrap();
        assert_eq!((t.hops[0].user.as_str(), t.hops[0].port), ("bob", 2022));
    }

    #[test]
    fn hostname_expansion_before_validation() {
        // §10.1: `Host *.int` + `HostName %h.corp.example.com` → x.int.corp.example.com, valid.
        let f = fx("Host *.int\n  HostName %h.corp.example.com\n");
        let t = run(
            &f,
            &no_books(),
            Target::SshConfig {
                alias: "x.int".into(),
            },
        )
        .unwrap();
        assert_eq!(t.hops[0].hostname, "x.int.corp.example.com");
        let f = fx("Host evil\n  HostName ../../etc\n");
        let err = run(
            &f,
            &no_books(),
            Target::SshConfig {
                alias: "evil".into(),
            },
        )
        .unwrap_err();
        assert!(matches!(err.kind, ErrorCode::InvalidHostname { .. }));
    }

    #[test]
    fn agent_and_known_hosts_options() {
        let f = fx(concat!(
            "Host a\n  IdentityAgent $AGENT_SOCK\n  UserKnownHostsFile none\n  StrictHostKeyChecking accept-new\n",
            "Host b\n  IdentityAgent none\n",
            "Host c\n  IdentityAgent SSH_AUTH_SOCK\n",
        ));
        let a = &run(&f, &no_books(), Target::SshConfig { alias: "a".into() })
            .unwrap()
            .hops[0];
        assert_eq!(
            a.identity_agent,
            AgentChoice::Endpoint("/run/agent.sock".into())
        );
        assert_eq!(a.user_known_hosts, vec![KnownHostsFile::Disabled]);
        assert_eq!(a.strict, StrictMode::AcceptNew);
        let b = &run(&f, &no_books(), Target::SshConfig { alias: "b".into() })
            .unwrap()
            .hops[0];
        assert_eq!(b.identity_agent, AgentChoice::Disabled);
        // SSH_AUTH_SOCK unset in this env ⇒ disabled.
        let c = &run(&f, &no_books(), Target::SshConfig { alias: "c".into() })
            .unwrap()
            .hops[0];
        assert_eq!(c.identity_agent, AgentChoice::Disabled);
        assert_eq!(b.user_known_hosts.len(), 2);
    }

    #[test]
    fn draft_is_recorded_as_quick_and_keeps_password() {
        let f = fx("");
        let input = BookmarkInput {
            id: None,
            name: String::new(),
            host: "example.com".into(),
            port: Some(2222),
            user: Some("alice".into()),
            auth: AuthInput::Password,
            proxy_jump: None,
            group_id: None,
            tags: vec![],
            start_dir: None,
            use_ssh_config: false,
            color: None,
            upload_limit_kibps: Some(64),
            download_limit_kibps: None,
            confirm_connect: None,
            startup_command: None,
            notes: None,
        };
        let t = run(
            &f,
            &no_books(),
            Target::Draft {
                bookmark: Box::new(input),
                password: Some(Secret::new("pw".to_owned())),
            },
        )
        .unwrap();
        assert_eq!(
            t.recent,
            RecentTarget::Quick {
                spec: "alice@example.com:2222".into()
            }
        );
        assert!(t.draft_password.is_some());
        assert_eq!(t.limits.upload_kibps, Some(64));
        assert_eq!(t.limits.download_kibps, None);
        assert!(t.hops[0].keychain.is_none()); // drafts are never remembered
        assert!(matches!(t.hops[0].auth, AuthPlan::Password));

        // An IPv6 draft is recorded as a bracketed (parsable) spec.
        let mut input6 = BookmarkInput {
            id: None,
            name: String::new(),
            host: "2001:db8::1".into(),
            port: None,
            user: Some(r"CORP\alice".into()),
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
        };
        let t6 = run(
            &f,
            &no_books(),
            Target::Draft {
                bookmark: Box::new(input6.clone()),
                password: None,
            },
        )
        .unwrap();
        let RecentTarget::Quick { spec } = &t6.recent else {
            panic!("{:?}", t6.recent)
        };
        assert_eq!(spec, r"CORP\alice@[2001:db8::1]");
        assert!(parse_host_spec(spec).is_some());
        input6.port = Some(2222);
        let t6 = run(
            &f,
            &no_books(),
            Target::Draft {
                bookmark: Box::new(input6),
                password: None,
            },
        )
        .unwrap();
        assert_eq!(
            t6.recent,
            RecentTarget::Quick {
                spec: r"CORP\alice@[2001:db8::1]:2222".into()
            }
        );
    }
}
