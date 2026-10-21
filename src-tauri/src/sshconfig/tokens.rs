//! Token expansion (§7.4): `HostName` supports only `%h` and `%%`; file-valued options support
//! `~`, `%d`, `%u`, `%h`, `%p`, `%r`, `%n`, `%%` and `${ENV}`. Expansion is single-pass, so text
//! produced by a token is never expanded again. An unknown token or an unset variable is an
//! error carrying the offending token (→ `unknownToken` warning, value dropped).

use std::path::Path;

/// Values for the file-option tokens.
pub(crate) struct TokenCtx<'a> {
    /// `%d` and `~`.
    pub home: &'a Path,
    /// `%u`.
    pub local_user: &'a str,
    /// `%h` — the resolved (validated) HostName, or the alias.
    pub host: &'a str,
    /// `%p`.
    pub port: u16,
    /// `%r`.
    pub remote_user: &'a str,
    /// `%n` — the alias as typed.
    pub alias: &'a str,
    /// `%k` — `HostKeyAlias` if set, else the alias as typed (OpenSSH; common in
    /// `UserKnownHostsFile ~/.ssh/known_hosts.d/%k`).
    pub key_alias: &'a str,
    /// `${VAR}` lookup.
    pub env: &'a (dyn Fn(&str) -> Option<String> + Send + Sync),
}

/// `HostName` expansion: `%h` → alias, `%%` → `%`. Must run **before** `valid_hostname`.
pub(crate) fn expand_hostname(value: &str, alias: &str) -> Result<String, String> {
    let mut out = String::with_capacity(value.len());
    let mut it = value.chars();
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('h') => out.push_str(alias),
            Some('%') => out.push('%'),
            Some(other) => return Err(format!("%{other}")),
            None => return Err("%".to_owned()),
        }
    }
    Ok(out)
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
pub(crate) fn expand_path(value: &str, ctx: &TokenCtx<'_>) -> Result<String, String> {
    let value = expand_tilde(value, ctx.home);
    let mut out = String::with_capacity(value.len());
    let mut it = value.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '%' => match it.next() {
                Some('d') => out.push_str(&ctx.home.display().to_string()),
                Some('u') => out.push_str(ctx.local_user),
                Some('h') => out.push_str(ctx.host),
                Some('p') => out.push_str(&ctx.port.to_string()),
                Some('r') => out.push_str(ctx.remote_user),
                Some('n') => out.push_str(ctx.alias),
                Some('k') => out.push_str(ctx.key_alias),
                Some('%') => out.push('%'),
                Some(other) => return Err(format!("%{other}")),
                None => return Err("%".to_owned()),
            },
            '$' if it.peek() == Some(&'{') => {
                it.next();
                let mut name = String::new();
                let mut closed = false;
                for n in it.by_ref() {
                    if n == '}' {
                        closed = true;
                        break;
                    }
                    name.push(n);
                }
                let token = if closed {
                    format!("${{{name}}}")
                } else {
                    format!("${{{name}")
                };
                if !closed || !valid_env_name(&name) {
                    return Err(token);
                }
                match (ctx.env)(&name) {
                    Some(v) => out.push_str(&v),
                    None => return Err(token),
                }
            }
            _ => out.push(c),
        }
    }
    Ok(out)
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
            local_user: "alice",
            host: "db.example.com",
            port: 2222,
            remote_user: "deploy",
            alias: "db",
            key_alias: "db-key",
            env: &env,
        }
    }

    #[test]
    fn hostname_tokens() {
        assert_eq!(
            expand_hostname("%h.corp.example.com", "x.int").unwrap(),
            "x.int.corp.example.com"
        );
        assert_eq!(expand_hostname("a%%b", "x").unwrap(), "a%b");
        assert_eq!(expand_hostname("%d", "x").unwrap_err(), "%d");
        assert_eq!(expand_hostname("x%", "x").unwrap_err(), "%");
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
        assert_eq!(expand_path("${UNSET}/x", &c).unwrap_err(), "${UNSET}");
        assert_eq!(expand_path("${KEYS", &c).unwrap_err(), "${KEYS");
        assert_eq!(expand_path("%C", &c).unwrap_err(), "%C");
        assert_eq!(expand_path("%l", &c).unwrap_err(), "%l");
        assert_eq!(
            expand_path("~/.ssh/known_hosts.d/%k", &c).unwrap(),
            home.join(".ssh/known_hosts.d/db-key").display().to_string()
        );
        assert_eq!(expand_path("~", &c).unwrap(), home.display().to_string());
        assert_eq!(expand_path("~bob/x", &c).unwrap(), "~bob/x");
    }

    #[test]
    fn token_output_is_not_reexpanded() {
        fn env2(_: &str) -> Option<String> {
            Some("%h".to_owned())
        }
        let home = PathBuf::from("/h");
        let mut c = ctx(&home);
        c.env = &env2;
        assert_eq!(expand_path("${X}", &c).unwrap(), "%h");
    }
}
