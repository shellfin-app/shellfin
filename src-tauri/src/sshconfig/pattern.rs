//! Host pattern matching (`match.c`): `*` and `?` wildcards, `!` negation, comma lists.
//! Matching is case-insensitive (§7.4).

/// `true` if `text` matches the wildcard `pattern` (`*` any run, `?` one char), ignoring
/// ASCII/Unicode case.
pub(crate) fn wildcard_match(text: &str, pattern: &str) -> bool {
    let t: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let p: Vec<char> = pattern.chars().flat_map(char::to_lowercase).collect();
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

pub(crate) fn match_list(text: &str, list: &str) -> ListMatch {
    let mut result = ListMatch::NoMatch;
    for pat in list.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        match pat.strip_prefix('!') {
            Some(neg) => {
                if wildcard_match(text, neg) {
                    return ListMatch::Negated;
                }
            }
            None => {
                if wildcard_match(text, pat) {
                    result = ListMatch::Positive;
                }
            }
        }
    }
    result
}

/// `Host` line semantics (`readconf.c` `oHost`): the block applies if some pattern matches and
/// no negated pattern matches. Each whitespace-separated argument is a single pattern.
pub(crate) fn host_block_matches(alias: &str, patterns: &[String]) -> bool {
    let mut active = false;
    for pat in patterns {
        match pat.strip_prefix('!') {
            Some(neg) => {
                if wildcard_match(alias, neg) {
                    return false;
                }
            }
            None => {
                if wildcard_match(alias, pat) {
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

    #[test]
    fn wildcards() {
        assert!(wildcard_match("web01", "web*"));
        assert!(wildcard_match("web01", "web??"));
        assert!(!wildcard_match("web1", "web??"));
        assert!(wildcard_match("anything", "*"));
        assert!(wildcard_match("", "*"));
        assert!(!wildcard_match("", "?"));
        assert!(wildcard_match("a.int", "*.int"));
        assert!(!wildcard_match("a.int.x", "*.int"));
        assert!(wildcard_match("abcabd", "*abd"));
        assert!(wildcard_match("x.int.y", "*.int.*"));
        assert!(wildcard_match("aXb", "a*b"));
        assert!(!wildcard_match("aXc", "a*b"));
    }

    #[test]
    fn case_insensitive() {
        assert!(wildcard_match("WEB01.Example.COM", "web*.example.com"));
        assert!(wildcard_match("nas", "NAS"));
    }

    #[test]
    fn lists_and_negation() {
        assert_eq!(match_list("db1", "db*,web*"), ListMatch::Positive);
        assert_eq!(match_list("db1", "web*"), ListMatch::NoMatch);
        assert_eq!(match_list("db1", "db*,!db1"), ListMatch::Negated);
        assert_eq!(match_list("db2", "db*,!db1"), ListMatch::Positive);
        assert_eq!(match_list("x", "!y"), ListMatch::NoMatch);
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
    }

    #[test]
    fn concrete() {
        assert!(is_concrete("nas"));
        assert!(!is_concrete("*.int"));
        assert!(!is_concrete("web?"));
        assert!(!is_concrete("!bad"));
    }
}
