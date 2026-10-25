//! Token expansion (§7.4). One value table ([`TokenCtx::value`]) and one allowed set per option
//! ([`TokenSet`]), as OpenSSH:
//! - `HostName`: `%h` (the alias) and `%%`;
//! - `User` (OpenSSH ≥ 10.0): `%L %i %k %l %n %p %d %h %u %j`, `%%` and `${ENV}`, no `~`
//!   (`%r` would be itself, `%C` hashes the user);
//! - file options (`IdentityFile`, `CertificateFile`, `IdentityAgent`, `UserKnownHostsFile`,
//!   `GlobalKnownHostsFile`): `~`, everything `User` has, plus `%r` and `%C`.
//!
//! Expansion is single-pass, so text produced by a token is never expanded again. An unknown
//! token, a trailing `%` or an unset variable is [`ExpandError::Unknown`]; a known token whose
//! value Shellfin cannot compute exactly like OpenSSH here is [`ExpandError::Unavailable`]
//! (→ `unknownToken` / `unavailableToken` / `invalidUser` warning, value dropped). The output is
//! capped ([`TokenSet::max_bytes`]): a 4 MiB line of `%d%d…` must not expand to hundreds of MB.

use std::borrow::Cow;
use std::fmt::Write as _;
use std::net::IpAddr;
use std::path::Path;

use super::validate::MAX_HOSTNAME_LEN;

/// Longest expanded path (bytes), like `PATH_MAX`.
pub(crate) const MAX_PATH_BYTES: usize = 4096;
/// Longest expanded `User` (characters; validated by `resolve::valid_expanded_user`).
pub(crate) const MAX_USER_CHARS: usize = 255;

/// Which option is expanded; selects the allowed tokens and the output cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TokenSet {
    HostName,
    User,
    File,
}

impl TokenSet {
    /// The `%x` tokens this option accepts (besides `%%`).
    fn allowed(self) -> &'static [char] {
        match self {
            Self::HostName => &['h'],
            Self::User => &['L', 'i', 'k', 'l', 'n', 'p', 'd', 'h', 'u', 'j'],
            Self::File => &['L', 'i', 'k', 'l', 'n', 'p', 'd', 'h', 'u', 'j', 'C', 'r'],
        }
    }

    fn env_vars(self) -> bool {
        !matches!(self, Self::HostName)
    }

    /// Output cap in bytes. `User` is validated to 255 characters afterwards; up to 4 bytes per
    /// character are allowed here so the validator, not the cap, decides.
    fn max_bytes(self) -> usize {
        match self {
            Self::HostName => MAX_HOSTNAME_LEN,
            Self::User => MAX_USER_CHARS * 4,
            Self::File => MAX_PATH_BYTES,
        }
    }
}

/// Why a value could not be expanded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExpandError {
    /// Unknown token or one this option does not accept (`%Z`, `%C` in `User`, `%` at the end),
    /// or an unset or malformed `${VAR}`; carries the token as written (never a variable's
    /// value).
    Unknown(String),
    /// A token this option accepts whose value is not known exactly (`%C` without a usable
    /// local host name or user, `%h` of a non-canonical numeric IPv4, `%u` when the local
    /// account name is not OpenSSH's, …).
    Unavailable(String),
    /// The result would exceed [`TokenSet::max_bytes`].
    TooLong,
}

/// Values for the tokens. `None` = unavailable here (the token is an error).
pub(crate) struct TokenCtx<'a> {
    /// `%d` and `~`.
    pub home: &'a Path,
    /// `%u`. `None` when the local account name is not exactly OpenSSH's
    /// (`ConfigEnv::local_user_exact`).
    pub local_user: Option<&'a str>,
    /// `%h` — [`canonical_host`] of the resolved (validated) HostName, or of the alias: ASCII
    /// lowercase, IP literals in canonical form, as OpenSSH (which lowercases the host name
    /// before expanding `User` and the file options, and normalises address literals).
    /// `None` for a non-canonical numeric IPv4 (`127.1`).
    pub host: Option<&'a str>,
    /// `%p` — the connection's port, else the config `Port`, else 22.
    pub port: u16,
    /// `%r` — the final remote user. `None` while `User` itself is expanded, or when the
    /// user is unusable in paths.
    pub remote_user: Option<&'a str>,
    /// `%n` — the alias as typed.
    pub alias: &'a str,
    /// `%k` — `HostKeyAlias` if set, else the alias as typed (OpenSSH; common in
    /// `UserKnownHostsFile ~/.ssh/known_hosts.d/%k`).
    pub key_alias: &'a str,
    /// `%l` (full) / `%L` (up to the first `.`): the local host name.
    pub local_host: Option<&'a str>,
    /// `%i`: the local numeric user id.
    pub uid: Option<&'a str>,
    /// `%j`: host part of the last ProxyJump hop of this connection (`""` without one; `None`
    /// if the config's last ProxyJump entry is malformed).
    pub jump_host: Option<&'a str>,
    /// `%C`: [`connection_hash`]. `None` while `User` is expanded or when an input is unknown.
    pub conn_hash: Option<&'a str>,
    /// `${VAR}` lookup.
    pub env: &'a EnvFn,
}

impl TokenCtx<'_> {
    /// The value table: `%t` in an option of kind `set`.
    fn value(&self, t: char, set: TokenSet) -> Result<Cow<'_, str>, ExpandError> {
        if !set.allowed().contains(&t) {
            return Err(ExpandError::Unknown(format!("%{t}")));
        }
        let v = match t {
            'd' => Some(Cow::Owned(self.home.display().to_string())),
            'u' => self.local_user.map(Cow::Borrowed),
            'h' => self.host.map(Cow::Borrowed),
            'p' => Some(Cow::Owned(self.port.to_string())),
            'n' => Some(Cow::Borrowed(self.alias)),
            'k' => Some(Cow::Borrowed(self.key_alias)),
            'l' => self.local_host.map(Cow::Borrowed),
            'L' => self
                .local_host
                .map(|full| Cow::Borrowed(full.split('.').next().unwrap_or(full))),
            'i' => self.uid.map(Cow::Borrowed),
            'j' => self.jump_host.map(Cow::Borrowed),
            'r' => self.remote_user.map(Cow::Borrowed),
            'C' => self.conn_hash.map(Cow::Borrowed),
            _ => return Err(ExpandError::Unknown(format!("%{t}"))),
        };
        v.ok_or_else(|| ExpandError::Unavailable(format!("%{t}")))
    }
}

/// `${VAR}` lookup.
type EnvFn = dyn Fn(&str) -> Option<String> + Send + Sync;

/// Longest `${NAME}` looked up (longer names are an error, and are not collected).
const MAX_ENV_NAME_BYTES: usize = 256;

/// The single-pass engine: `%%` → `%`, `%t` → `token(t)`, `${NAME}` → env (if `env` is given).
fn expand_with<'v>(
    value: &str,
    max_bytes: usize,
    env: Option<&EnvFn>,
    token: impl Fn(char) -> Result<Cow<'v, str>, ExpandError>,
) -> Result<String, ExpandError> {
    let mut out = String::with_capacity(value.len().min(max_bytes));
    let push = |out: &mut String, s: &str| -> Result<(), ExpandError> {
        if out.len().saturating_add(s.len()) > max_bytes {
            return Err(ExpandError::TooLong);
        }
        out.push_str(s);
        Ok(())
    };
    let mut it = value.chars().peekable();
    let mut ch = [0u8; 4];
    while let Some(c) = it.next() {
        match c {
            '%' => match it.next() {
                Some('%') => push(&mut out, "%")?,
                Some(t) => push(&mut out, &token(t)?)?,
                None => return Err(ExpandError::Unknown("%".to_owned())),
            },
            '$' if env.is_some() && it.peek() == Some(&'{') => {
                it.next();
                let mut name = String::new();
                let mut closed = false;
                let mut overlong = false;
                for n in it.by_ref() {
                    if n == '}' {
                        closed = true;
                        break;
                    }
                    // A name is short; do not collect megabytes of an unterminated `${…`.
                    if name.len() < MAX_ENV_NAME_BYTES {
                        name.push(n);
                    } else {
                        overlong = true;
                    }
                }
                let token = if closed && !overlong {
                    format!("${{{name}}}")
                } else {
                    format!("${{{name}")
                };
                if !closed || overlong || !valid_env_name(&name) {
                    return Err(ExpandError::Unknown(token));
                }
                match env.and_then(|lookup| lookup(&name)) {
                    Some(v) => push(&mut out, &v)?,
                    None => return Err(ExpandError::Unknown(token)),
                }
            }
            _ => push(&mut out, c.encode_utf8(&mut ch))?,
        }
    }
    Ok(out)
}

/// `HostName` expansion: `%h` → alias, `%%` → `%`. Must run **before** `valid_hostname`.
pub(crate) fn expand_hostname(value: &str, alias: &str) -> Result<String, ExpandError> {
    let set = TokenSet::HostName;
    let env = set.env_vars().then_some(NO_ENV);
    expand_with(value, set.max_bytes(), env, |t| {
        if set.allowed().contains(&t) && t == 'h' {
            Ok(Cow::Borrowed(alias))
        } else {
            Err(ExpandError::Unknown(format!("%{t}")))
        }
    })
}

/// Placeholder lookup for sets without variables (never called: `env_vars()` is false).
const NO_ENV: &EnvFn = &|_| None;

/// `User` expansion (no `~`). The caller validates the result if it differs from the input.
pub(crate) fn expand_user(value: &str, ctx: &TokenCtx<'_>) -> Result<String, ExpandError> {
    let set = TokenSet::User;
    let env = set.env_vars().then_some(ctx.env);
    expand_with(value, set.max_bytes(), env, |t| ctx.value(t, set))
}

/// Leading `~` / `~/…` → home directory. `~user` forms are left untouched.
pub(crate) fn expand_tilde(value: &str, home: &Path) -> String {
    if value == "~" {
        return home.display().to_string();
    }
    match value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("~\\"))
    {
        Some(rest) => home.join(rest).display().to_string(),
        None => value.to_owned(),
    }
}

/// File-option expansion: `~`, then `%` tokens and `${VAR}` in one pass.
pub(crate) fn expand_path(value: &str, ctx: &TokenCtx<'_>) -> Result<String, ExpandError> {
    let set = TokenSet::File;
    let value = expand_tilde(value, ctx.home);
    let env = set.env_vars().then_some(ctx.env);
    expand_with(&value, set.max_bytes(), env, |t| ctx.value(t, set))
}

/// `%h` as OpenSSH uses it after reading the config (`ssh.c` lowercases the host name and
/// normalises an address literal through `getaddrinfo(AI_NUMERICHOST)`/`getnameinfo`): an IP
/// literal in canonical form (`2001:DB8:0::1` → `2001:db8::1`), anything else ASCII-lowercased.
/// `None` for a numeric IPv4 that is not dotted-quad canonical (`127.1`, `192.168.001.010`,
/// `0x7f.1`, `2130706433`): the C resolver would turn it into another address string
/// (`127.0.0.1`) and Shellfin does not emulate `inet_aton`, so `%h` and `%C` are unavailable.
///
/// Also `None` for an IPv6 address whose first five groups are zero and whose sixth is `0` or
/// `ffff` (IPv4-compatible and IPv4-mapped forms), except `::` and `::1`: the C libraries
/// print these differently from Rust and from each other (`ssh -G`, Win32-OpenSSH 9.5p2 and
/// MSYS 10.2p1: `::7f00:1` → `::127.0.0.1`, `::ffff:0:1` → `::ffff:0:1`,
/// `0:0:0:0:0:ffff::` → `::ffff:0:0`; glibc may differ again). This deliberately also covers
/// forms that happen to match today (`::2`, `::ffff:1.2.3.4`, `::ffff:7f00:1`): failing
/// towards "unavailable" (the hop asks) is safe, a wrong `%C` is not.
pub(crate) fn canonical_host(host: &str) -> Option<String> {
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V6(v6)) if ambiguous_ipv6_text(v6) => return None,
        Ok(ip) => return Some(ip.to_string()),
        Err(_) => {}
    }
    if numeric_ipv4_like(host) {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

/// IPv6 text forms whose canonical spelling differs between C libraries (see
/// [`canonical_host`]): `0:0:0:0:0:{0|ffff}:x:y`, except `::` and `::1`.
fn ambiguous_ipv6_text(ip: std::net::Ipv6Addr) -> bool {
    let s = ip.segments();
    let zero_prefix = s.iter().take(5).all(|&g| g == 0);
    let sixth = s.get(5).copied().unwrap_or(1);
    zero_prefix && matches!(sixth, 0 | 0xffff) && !ip.is_unspecified() && !ip.is_loopback()
}

/// One to four dot-separated numbers in `inet_aton` syntax (decimal, `0`-octal, `0x` hex).
fn numeric_ipv4_like(host: &str) -> bool {
    let number = |p: &str| match p.strip_prefix("0x").or_else(|| p.strip_prefix("0X")) {
        Some(hex) => hex.bytes().all(|b| b.is_ascii_hexdigit()),
        None => !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()),
    };
    host.split('.').count() <= 4 && host.split('.').all(number)
}

/// `%C` (OpenSSH ≥ 9.6): lowercase hex SHA-1 of `%l%h%p%r%j`, plain concatenation. `host` must
/// already be [`canonical_host`] (OpenSSH hashes the lowercased / normalised name).
pub(crate) fn connection_hash(
    local_host: &str,
    host: &str,
    port: u16,
    user: &str,
    jump_host: &str,
) -> String {
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY);
    for part in [local_host, host, &port.to_string(), user, jump_host] {
        ctx.update(part.as_bytes());
    }
    let digest = ctx.finish();
    let mut hex = String::with_capacity(digest.as_ref().len() * 2);
    for b in digest.as_ref() {
        let _ = write!(hex, "{b:02x}");
    }
    hex
}

/// `[A-Za-z_][A-Za-z0-9_]*`.
pub(crate) fn valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use std::path::PathBuf;

    fn env(name: &str) -> Option<String> {
        match name {
            "HOME" => Some("/home/alice".to_owned()),
            "KEYS" => Some("/opt/keys".to_owned()),
            _ => None,
        }
    }

    fn ctx(home: &Path) -> TokenCtx<'_> {
        TokenCtx {
            home,
            local_user: Some("alice"),
            host: Some("db.example.com"),
            port: 2222,
            remote_user: Some("deploy"),
            alias: "db",
            key_alias: "db-key",
            local_host: Some("laptop.example.org"),
            uid: Some("1000"),
            jump_host: Some("bastion"),
            conn_hash: Some("0123abcd"),
            env: &env,
        }
    }

    fn tok(t: &str) -> ExpandError {
        ExpandError::Unknown(t.to_owned())
    }

    fn unavailable(t: &str) -> ExpandError {
        ExpandError::Unavailable(t.to_owned())
    }

    #[test]
    fn hostname_tokens() {
        assert_eq!(
            expand_hostname("%h.corp.example.com", "x.int").unwrap(),
            "x.int.corp.example.com"
        );
        assert_eq!(expand_hostname("a%%b", "x").unwrap(), "a%b");
        assert_eq!(expand_hostname("%d", "x").unwrap_err(), tok("%d"));
        assert_eq!(expand_hostname("%l", "x").unwrap_err(), tok("%l"));
        assert_eq!(expand_hostname("x%", "x").unwrap_err(), tok("%"));
        assert_eq!(
            expand_hostname("${KEYS}", "x").unwrap(),
            "${KEYS}",
            "no variables in HostName (valid_hostname rejects `$`)"
        );
        // (hostile input) a 4 MiB `%h%h…` is bounded, not 1 GB.
        let flood = "%h".repeat(2 * 1024 * 1024);
        assert_eq!(
            expand_hostname(&flood, &"a".repeat(253)).unwrap_err(),
            ExpandError::TooLong
        );
    }

    #[test]
    fn path_tokens() {
        let home = PathBuf::from("/home/alice");
        let c = ctx(&home);
        assert_eq!(
            expand_path("~/.ssh/id_%h_%p_%r_%n_%u", &c).unwrap(),
            home.join(".ssh/id_db.example.com_2222_deploy_db_alice")
                .display()
                .to_string()
        );
        assert_eq!(
            expand_path("%d/k", &c).unwrap(),
            format!("{}/k", home.display())
        );
        assert_eq!(expand_path("100%%", &c).unwrap(), "100%");
        assert_eq!(expand_path("${KEYS}/id", &c).unwrap(), "/opt/keys/id");
        assert_eq!(
            expand_path("${HOME}/.ssh/id", &c).unwrap(),
            "/home/alice/.ssh/id"
        );
        assert_eq!(
            expand_path("$KEYS/id", &c).unwrap(),
            "$KEYS/id",
            "only ${{}} is expanded in paths"
        );
        assert_eq!(expand_path("${UNSET}/x", &c).unwrap_err(), tok("${UNSET}"));
        assert_eq!(expand_path("${KEYS", &c).unwrap_err(), tok("${KEYS"));
        assert_eq!(expand_path("${1X}", &c).unwrap_err(), tok("${1X}"));
        assert_eq!(expand_path("%Z", &c).unwrap_err(), tok("%Z"));
        assert_eq!(expand_path("x%", &c).unwrap_err(), tok("%"));
        assert_eq!(
            expand_path("/kh/%C|%l|%L|%i|%j", &c).unwrap(),
            "/kh/0123abcd|laptop.example.org|laptop|1000|bastion"
        );
        assert_eq!(
            expand_path("%%C", &c).unwrap(),
            "%C",
            "an escaped token stays literal"
        );
        assert_eq!(
            expand_path("~/.ssh/known_hosts.d/%k", &c).unwrap(),
            home.join(".ssh/known_hosts.d/db-key").display().to_string()
        );
        assert_eq!(expand_path("~", &c).unwrap(), home.display().to_string());
        assert_eq!(expand_path("~bob/x", &c).unwrap(), "~bob/x");
    }

    /// Unavailable values make their tokens errors; nothing is guessed.
    #[test]
    fn unavailable_values_are_errors() {
        let home = PathBuf::from("/h");
        let mut c = ctx(&home);
        c.local_host = None;
        c.uid = None;
        c.conn_hash = None;
        c.remote_user = None;
        c.local_user = None;
        c.host = None;
        for t in ["%l", "%L", "%i", "%C", "%r", "%u", "%h"] {
            assert_eq!(expand_path(t, &c).unwrap_err(), unavailable(t), "{t}");
        }
        for t in ["%u", "%h", "%l"] {
            assert_eq!(expand_user(t, &c).unwrap_err(), unavailable(t), "{t}");
        }
        // Not accepted in User at all: unknown, whatever their value.
        assert_eq!(expand_user("%C", &c).unwrap_err(), tok("%C"));
        c.jump_host = Some("");
        assert_eq!(
            expand_path("/x%j", &c).unwrap(),
            "/x",
            "%j is empty without a jump"
        );
        c.jump_host = None;
        assert_eq!(expand_path("/x%j", &c).unwrap_err(), unavailable("%j"));
    }

    #[test]
    fn user_tokens() {
        let home = PathBuf::from("/home/alice");
        let c = ctx(&home);
        assert_eq!(
            expand_user("%u@%L-%l-%i-%k-%n-%p-%h-%j", &c).unwrap(),
            "alice@laptop-laptop.example.org-1000-db-key-db-2222-db.example.com-bastion"
        );
        assert_eq!(expand_user("%%u", &c).unwrap(), "%u");
        assert_eq!(expand_user("ops-${KEYS}", &c).unwrap(), "ops-/opt/keys");
        assert_eq!(expand_user("~", &c).unwrap(), "~", "no tilde in User");
        // %r is User itself, %C hashes it.
        assert_eq!(expand_user("%r", &c).unwrap_err(), tok("%r"));
        assert_eq!(expand_user("x%C", &c).unwrap_err(), tok("%C"));
        assert_eq!(expand_user("a%", &c).unwrap_err(), tok("%"));
        assert_eq!(expand_user("%Z", &c).unwrap_err(), tok("%Z"));
        assert_eq!(expand_user("${UNSET}", &c).unwrap_err(), tok("${UNSET}"));
        assert_eq!(expand_user("${X", &c).unwrap_err(), tok("${X"));
    }

    #[test]
    fn token_output_is_not_reexpanded() {
        fn env2(_: &str) -> Option<String> {
            Some("%h".to_owned())
        }
        fn env3(_: &str) -> Option<String> {
            Some("%C".to_owned())
        }
        let home = PathBuf::from("/h");
        let mut c = ctx(&home);
        c.env = &env2;
        assert_eq!(expand_path("${X}", &c).unwrap(), "%h");
        assert_eq!(expand_user("${X}", &c).unwrap(), "%h");
        c.env = &env3;
        assert_eq!(expand_path("/kh/${X}", &c).unwrap(), "/kh/%C");
    }

    /// (hostile input) a 4 MiB line of tokens expands to at most the cap, then fails.
    #[test]
    fn output_is_capped() {
        let home = PathBuf::from("/home/alice/a/fairly/long/home/directory");
        let c = ctx(&home);
        let flood = "%d".repeat(2 * 1024 * 1024);
        assert_eq!(expand_path(&flood, &c).unwrap_err(), ExpandError::TooLong);
        assert_eq!(expand_user(&flood, &c).unwrap_err(), ExpandError::TooLong);
        let unterminated = format!("${{{}", "A".repeat(4 * 1024 * 1024));
        assert!(matches!(
            expand_path(&unterminated, &c).unwrap_err(),
            ExpandError::Unknown(t) if t.len() < 300
        ));
        // An over-long (but closed) name is an error, never a lookup of its truncated prefix.
        let overlong = format!("${{{}}}", "A".repeat(300));
        assert!(matches!(
            expand_path(&overlong, &c).unwrap_err(),
            ExpandError::Unknown(t) if t.len() < 300
        ));
        let at_cap = "a".repeat(MAX_PATH_BYTES);
        assert_eq!(expand_path(&at_cap, &c).unwrap(), at_cap);
        assert_eq!(
            expand_path(&format!("{at_cap}b"), &c).unwrap_err(),
            ExpandError::TooLong
        );
    }

    /// Fixed vectors (OpenSSH ≥ 9.6 `%C` = SHA-1 of `%l%h%p%r%j`).
    #[test]
    fn connection_hash_vectors() {
        assert_eq!(
            connection_hash("laptop.example.org", "db.example.com", 2222, "deploy", ""),
            "abeddbd31e78b12b84bb72178f9da77a87609068"
        );
        assert_eq!(
            connection_hash(
                "laptop.example.org",
                "db.example.com",
                2222,
                "deploy",
                "bastion"
            ),
            "afe40bf82d1a691f8e0b53bfb83376182ea83392"
        );
        // Windows, implicit user: Win32-OpenSSH hashes the lowercased account name (`Alice` →
        // `alice`): SHA-1("laptop.example.org" "db.example.com" "22" "alice" "").
        assert_eq!(
            connection_hash("laptop.example.org", "db.example.com", 22, "alice", ""),
            "1227bff8334e0ddbb57c2fb46f0c25b8ecac5c7f"
        );
        assert_eq!(
            connection_hash("laptop.example.org", "db.example.com", 22, "Alice", ""),
            "a16aec8bf093a7469ba77018b2190c6ada9f2bec",
            "the case of the user changes the file"
        );
        // The host goes in canonical (lowercase / normalised IP) form.
        assert_eq!(
            connection_hash(
                "laptop.example.org",
                &canonical_host("DB.Example.COM").unwrap(),
                2222,
                "deploy",
                ""
            ),
            "abeddbd31e78b12b84bb72178f9da77a87609068"
        );
        // The user is hashed as given (path-unsafe characters are fine: the output is hex).
        let h = connection_hash("l", "h", 22, r"DOMAIN\alice", "");
        assert_eq!(h.len(), 40);
        assert!(
            h.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
        assert_ne!(h, connection_hash("l", "h", 22, "alice", ""));
    }

    /// (hostile input) `%h` as OpenSSH sees it: lowercase, canonical IP literals, and no value
    /// for numeric IPv4 forms the C resolver would rewrite.
    #[test]
    fn canonical_host_forms() {
        for (input, want) in [
            ("DB.Example.COM", "db.example.com"),
            ("db", "db"),
            ("2001:DB8:0:0::1", "2001:db8::1"),
            ("2001:DB8:0::1", "2001:db8::1"),
            ("::", "::"),
            ("::1", "::1"),
            ("fe80::1:0:0:1", "fe80::1:0:0:1"),
            ("64:ff9b::1.2.3.4", "64:ff9b::102:304"),
            ("127.0.0.1", "127.0.0.1"),
            ("10.0.0.255", "10.0.0.255"),
            ("1e100.example", "1e100.example"),
            ("1.2.3.4.5", "1.2.3.4.5"),
            ("1.2.3.4.", "1.2.3.4."),
            ("0x7f.example", "0x7f.example"),
            ("ÄÖ.example", "ÄÖ.example"),
        ] {
            assert_eq!(canonical_host(input).as_deref(), Some(want), "{input}");
        }
        for numeric in [
            "127.1",
            "192.168.001.010",
            "0x7f.1",
            "0X7F.0.0.1",
            "2130706433",
            "017700000001",
            "127.0.1",
            "0x",
            "0",
        ] {
            assert_eq!(canonical_host(numeric), None, "{numeric}");
        }
        // (M1, #7 re-review) IPv4-compatible / -mapped IPv6: OpenSSH (inet_ntop/getnameinfo)
        // spells these differently from Rust: `::7f00:1` → `::127.0.0.1`, `::ffff:0:1` →
        // `::ffff:0:1`, `0:0:0:0:0:ffff::` → `::ffff:0:0`. Unavailable, never guessed; forms
        // that happen to match (`::2`, `::ffff:1.2.3.4`, `::FFFF:7f00:1`) are covered too.
        for v6 in [
            "::7f00:1",
            "::ffff:0:1",
            "0:0:0:0:0:ffff::",
            "::2",
            "::ffff:1.2.3.4",
            "::FFFF:7f00:1",
            "::FFFF:127.0.0.1",
            "::1.2.3.4",
        ] {
            assert_eq!(canonical_host(v6), None, "{v6}");
        }
    }
}
