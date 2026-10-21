//! `valid_hostname` (§5.1 rule 5): OpenSSH-like validation for bookmark `host`, quick specs,
//! ProxyJump specs, config `HostName` (after its own `%h`/`%%` expansion) and `HostKeyAlias`.
//! It runs **before** a name is used in `%h` expansions inside file options, so it rejects every
//! character that could turn into path syntax (`/`, `\`, `%`) or shell/pattern syntax.

/// Characters never allowed in a host name (besides whitespace, C0 controls and DEL).
const FORBIDDEN: &[char] = &[
    ',', '*', '?', '!', '|', '#', '@', '"', '\'', '\\', '/', '%', '$', ';', '&', '<', '>', '(',
    ')', '[', ']', '{', '}', '`',
];

pub const MAX_HOSTNAME_LEN: usize = 253;

/// `true` if `name` is acceptable as a host name or `HostKeyAlias`. IPv6 addresses are passed
/// here **without** brackets (spec parsers strip `[…]`).
pub fn valid_hostname(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_HOSTNAME_LEN
        && !name.starts_with('-')
        // A whole-name `.`/`..` would act as a path component in `%h` expansions.
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| !c.is_whitespace() && !c.is_control() && !FORBIDDEN.contains(&c))
}

/// A parsed `[user@]host[:port]` spec (quick connect, ProxyJump entries, `JumpRef::Spec`).
/// IPv6 literals must be bracketed: `[2001:db8::1]:2222`, `user@[::1]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostSpec {
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
}

/// Parse `[user@]host[:port]` (§5.1 rule 3). The host part must pass [`valid_hostname`]; the
/// user must be non-empty without whitespace/controls/`@`/`:`/`/`; the port 1–65535.
/// Returns `None` for anything else (callers map that to `invalidHostSpec` / `invalidHostname`).
pub fn parse_host_spec(spec: &str) -> Option<HostSpec> {
    let spec = spec.trim();
    let (user, rest) = match spec.rsplit_once('@') {
        Some((u, r)) => (Some(u), r),
        None => (None, spec),
    };
    // The host part follows the *last* `@`, so UPN users (`alice@corp.example.com@host`) and
    // `DOMAIN\alice` work (drafts with such users are recorded as quick specs).
    if let Some(u) = user
        && (u.is_empty()
            || u.chars().count() > 255
            || u.starts_with('@')
            || u.chars()
                .any(|c| c.is_whitespace() || c.is_control() || matches!(c, ':' | '/')))
    {
        return None;
    }
    let (host, port) = if let Some(after_bracket) = rest.strip_prefix('[') {
        let (addr, tail) = after_bracket.split_once(']')?;
        if !addr.contains(':') {
            return None; // brackets are for IPv6 literals only
        }
        let port = match tail {
            "" => None,
            t => Some(parse_port(t.strip_prefix(':')?)?),
        };
        (addr, port)
    } else {
        match rest.split_once(':') {
            None => (rest, None),
            Some((h, p)) => {
                if p.contains(':') {
                    return None; // bare IPv6 is ambiguous; must be bracketed
                }
                (h, Some(parse_port(p)?))
            }
        }
    };
    if !valid_hostname(host) {
        return None;
    }
    Some(HostSpec {
        user: user.map(str::to_owned),
        host: host.to_owned(),
        port,
    })
}

fn parse_port(s: &str) -> Option<u16> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<u16>().ok().filter(|p| *p != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(user: Option<&str>, host: &str, port: Option<u16>) -> Option<HostSpec> {
        Some(HostSpec {
            user: user.map(str::to_owned),
            host: host.to_owned(),
            port,
        })
    }

    #[test]
    fn host_specs() {
        assert_eq!(
            parse_host_spec("example.com"),
            spec(None, "example.com", None)
        );
        assert_eq!(
            parse_host_spec("alice@example.com:2222"),
            spec(Some("alice"), "example.com", Some(2222))
        );
        assert_eq!(
            parse_host_spec(" alice@nas "),
            spec(Some("alice"), "nas", None)
        );
        assert_eq!(
            parse_host_spec("[2001:db8::1]:22"),
            spec(None, "2001:db8::1", Some(22))
        );
        assert_eq!(parse_host_spec("bob@[::1]"), spec(Some("bob"), "::1", None));
        assert_eq!(
            parse_host_spec(r"CORP\alice@host"),
            spec(Some(r"CORP\alice"), "host", None)
        );
        assert_eq!(
            parse_host_spec("alice@corp.example.com@host:2222"),
            spec(Some("alice@corp.example.com"), "host", Some(2222))
        );
        for bad in [
            "",
            "@host",
            "host:",
            "host:0",
            "host:65536",
            "host:22x",
            "::1",
            "2001:db8::1:22",
            "[example.com]:22",
            "[::1",
            "[::1]x",
            "a b@host",
            "host/x",
            "-oProxyCommand=x",
            "al:ice@host",
            "a/b@host",
            "@@host",
            "%h",
        ] {
            assert_eq!(parse_host_spec(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn accepts_normal_names() {
        for ok in [
            "example.com",
            "nas",
            "host-01.int.example.com",
            "192.0.2.10",
            "::1",
            "fe80::1",
            "x.int.corp.example.com",
            "xn--bcher-kva.example",
        ] {
            assert!(valid_hostname(ok), "{ok}");
        }
        assert!(valid_hostname(&"a".repeat(MAX_HOSTNAME_LEN)));
    }

    #[test]
    fn rejects_dangerous_names() {
        for bad in [
            "",
            "-oProxyCommand=x",
            "a b",
            "a\tb",
            "a\nb",
            "a\u{7f}",
            "a\u{0}",
            "x,*",
            "host/../../etc",
            "..\\..\\x",
            "%d",
            "a%h",
            "$HOME",
            "a;b",
            "a&b",
            "a|b",
            "a`b`",
            "user@host",
            "[::1]",
            "a(b)",
            "{x}",
            "a\"b",
            "a'b",
            "a#b",
            "a!b",
            "a?b",
            "<x>",
            ".",
            "..",
        ] {
            assert!(!valid_hostname(bad), "{bad:?}");
        }
        assert!(!valid_hostname(&"a".repeat(MAX_HOSTNAME_LEN + 1)));
    }
}
