//! Host pattern matching (`match.c`): `*` and `?` wildcards, `!` negation, comma lists.
//! Case follows OpenSSH (§7.4), checked with `ssh -G` on Win32-OpenSSH 9.5p2 and MSYS 10.2p1:
//! - `Host` patterns: case-sensitive, against the alias as typed (`Host Lab` applies to `Lab`,
//!   not to `lab` or `LAB`; `match_pattern`);
//! - `Match host` / `Match originalhost`: ASCII case-insensitive (`match_hostname` lowercases
//!   both the name and the pattern);
//! - `Match user` / `Match localuser`: case-sensitive (`match_pattern_list(…, 0)`).

/// How [`wildcard_match`] compares characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Case {
    /// Exact (`Host`, `Match user`, `Match localuser`).
    Sensitive,
    /// ASCII letters folded, like OpenSSH's `lowercase()`/`tolower` (`Match host`,
    /// `Match originalhost`); other characters compare exactly.
    AsciiInsensitive,
}

/// `true` if `text` matches the wildcard `pattern` (`*` any run, `?` one char).
pub(crate) fn wildcard_match(text: &str, pattern: &str, case: Case) -> bool {
    let fold = |c: char| match case {
        Case::Sensitive => c,
        Case::AsciiInsensitive => c.to_ascii_lowercase(),
    };
    let t: Vec<char> = text.chars().map(fold).collect();
    let p: Vec<char> = pattern.chars().map(fold).collect();
    // Iterative glob with single-star backtracking (linear in practice, no recursion).
    let (mut ti, mut pi) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        match p.get(pi) {
            Some('*') => {
                star = Some((pi, ti));
                pi += 1;
            }
            Some(&pc) if pc == '?' || Some(&pc) == t.get(ti) => {
                pi += 1;
                ti += 1;
            }
            _ => match star {
                Some((sp, st)) => {
                    pi = sp + 1;
                    ti = st + 1;
                    star = Some((sp, st + 1));
                }
                None => return false,
            },
        }
    }
    p.get(pi..)
        .is_some_and(|rest| rest.iter().all(|c| *c == '*'))
}

/// Result of matching against a comma-separated pattern list (`match_pattern_list`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ListMatch {
    /// A positive pattern matched and no negated one did.
    Positive,
    /// A negated pattern (`!pat`) matched — overrides everything.
    Negated,
    NoMatch,
}

pub(crate) fn match_list(text: &str, list: &str, case: Case) -> ListMatch {
    let mut result = ListMatch::NoMatch;
    for pat in list.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        match pat.strip_prefix('!') {
            Some(neg) => {
                if wildcard_match(text, neg, case) {
                    return ListMatch::Negated;
                }
            }
            None => {
                if wildcard_match(text, pat, case) {
                    result = ListMatch::Positive;
                }
            }
        }
    }
    result
}

/// `Host` line semantics (`readconf.c` `oHost`): the block applies if some pattern matches and
/// no negated pattern matches. Each whitespace-separated argument is a single pattern.
/// Case-sensitive against the alias as typed (OpenSSH).
pub(crate) fn host_block_matches(alias: &str, patterns: &[String]) -> bool {
    let mut active = false;
    for pat in patterns {
        match pat.strip_prefix('!') {
            Some(neg) => {
                if wildcard_match(alias, neg, Case::Sensitive) {
                    return false;
                }
            }
            None => {
                if wildcard_match(alias, pat, Case::Sensitive) {
                    active = true;
                }
            }
        }
    }
    active
}

/// A pattern naming exactly one host (no wildcards, not negated) — listed in the sidebar.
pub(crate) fn is_concrete(pattern: &str) -> bool {
    !pattern.is_empty() && !pattern.starts_with('!') && !pattern.contains(['*', '?'])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wm(text: &str, pattern: &str) -> bool {
        wildcard_match(text, pattern, Case::Sensitive)
    }

    #[test]
    fn wildcards() {
        assert!(wm("web01", "web*"));
        assert!(wm("web01", "web??"));
        assert!(!wm("web1", "web??"));
        assert!(wm("anything", "*"));
        assert!(wm("", "*"));
        assert!(!wm("", "?"));
        assert!(wm("a.int", "*.int"));
        assert!(!wm("a.int.x", "*.int"));
        assert!(wm("abcabd", "*abd"));
        assert!(wm("x.int.y", "*.int.*"));
        assert!(wm("aXb", "a*b"));
        assert!(!wm("aXc", "a*b"));
    }

    /// OpenSSH case rules (`ssh -G`, Win32 9.5p2 and MSYS 10.2p1).
    #[test]
    fn case_rules() {
        // Sensitive: `Host`, `Match user`, `Match localuser`.
        assert!(!wm("WEB01.Example.COM", "web*.example.com"));
        assert!(!wm("nas", "NAS"));
        assert!(wm("a.EXAMPLE.com", "*.EXAMPLE.com"));
        assert!(!wm("a.example.com", "*.EXAMPLE.com"));
        // ASCII-insensitive: `Match host`, `Match originalhost`.
        let ci = |t: &str, p: &str| wildcard_match(t, p, Case::AsciiInsensitive);
        assert!(ci("WEB01.Example.COM", "web*.example.com"));
        assert!(ci("mixhost.example.org", "MIXHOST.EXAMPLE.ORG"));
        assert!(ci("origcase", "OrigCase"));
        // Only ASCII is folded (OpenSSH `tolower` on bytes).
        assert!(!ci("ÄB", "äb"));
        assert!(ci("Äb", "ÄB"));
    }

    #[test]
    fn lists_and_negation() {
        let ml = |t: &str, l: &str| match_list(t, l, Case::Sensitive);
        assert_eq!(ml("db1", "db*,web*"), ListMatch::Positive);
        assert_eq!(ml("db1", "web*"), ListMatch::NoMatch);
        assert_eq!(ml("db1", "db*,!db1"), ListMatch::Negated);
        assert_eq!(ml("db2", "db*,!db1"), ListMatch::Positive);
        assert_eq!(ml("x", "!y"), ListMatch::NoMatch);
        assert_eq!(ml("Smith", "smith"), ListMatch::NoMatch);
        assert_eq!(
            match_list("DB1", "db*,!db1", Case::AsciiInsensitive),
            ListMatch::Negated
        );
    }

    #[test]
    fn host_blocks() {
        let p = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert!(host_block_matches("web1", &p(&["web*", "!web2"])));
        assert!(!host_block_matches("web2", &p(&["web*", "!web2"])));
        assert!(!host_block_matches("web2", &p(&["!web2", "web*"])));
        assert!(!host_block_matches("db", &p(&["!web*"])));
        assert!(
            host_block_matches("a,b", &p(&["a,b"])),
            "commas are literal in Host patterns"
        );
        // Case-sensitive, also negation (`Host !LAB *` applies to `lab`, not to `LAB`).
        assert!(host_block_matches("Lab", &p(&["Lab"])));
        assert!(!host_block_matches("lab", &p(&["Lab"])));
        assert!(!host_block_matches("LAB", &p(&["Lab"])));
        assert!(!host_block_matches("LOWER", &p(&["lower"])));
        assert!(host_block_matches("lab", &p(&["!LAB", "*"])));
        assert!(!host_block_matches("LAB", &p(&["!LAB", "*"])));
    }

    #[test]
    fn concrete() {
        assert!(is_concrete("nas"));
        assert!(!is_concrete("*.int"));
        assert!(!is_concrete("web?"));
        assert!(!is_concrete("!bad"));
    }
}
