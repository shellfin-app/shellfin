//! Tolerant known_hosts parser (§5.3). Unparsable lines are skipped and reported.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ring::hmac;

use super::blob_key_type;

/// Where a known_hosts file comes from (user files may be written; global ones never are).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    User,
    Global,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    None,
    Revoked,
    CertAuthority,
}

/// One entry of a comma-separated host pattern list (stored ASCII-lowercased).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPattern {
    pub negated: bool,
    pub pattern: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostPatterns {
    /// Plain `host1,[host2]:2222,*.example.com,!bad.example.com`.
    List(Vec<HostPattern>),
    /// `|1|base64(salt)|base64(HMAC-SHA1(salt, lookup))`.
    Hashed { salt: Vec<u8>, hash: Vec<u8> },
}

impl HostPatterns {
    /// OpenSSH `match_hostname` semantics: any matching negated pattern excludes the line;
    /// otherwise at least one positive pattern must match. Case-insensitive for plain patterns.
    pub fn matches(&self, lookup: &str) -> bool {
        match self {
            Self::List(_) => self.matches_lowercase(&lookup.to_ascii_lowercase()),
            Self::Hashed { .. } => self.matches_lowercase(lookup),
        }
    }

    /// [`Self::matches`] without lowercasing: plain patterns expect an ASCII-lowercased lookup
    /// (`verify` lowercases once per check instead of once per entry); hashed entries
    /// compare the lookup as given.
    pub fn matches_lowercase(&self, lookup: &str) -> bool {
        match self {
            Self::List(patterns) => {
                let mut positive = false;
                for p in patterns {
                    if wildcard_match(&p.pattern, lookup) {
                        if p.negated {
                            return false;
                        }
                        positive = true;
                    }
                }
                positive
            }
            Self::Hashed { salt, hash } => {
                let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, salt);
                hmac::verify(&key, lookup.as_bytes(), hash).is_ok()
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// 1-based line number.
    pub line: u32,
    pub marker: Marker,
    pub hosts: HostPatterns,
    pub key_type: String,
    pub blob: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct KnownHostsFile {
    pub path: PathBuf,
    /// Path as shown in error params (`knownHostsFile`), e.g. `~`-abbreviated.
    pub display: String,
    pub origin: Origin,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseIssue {
    pub line: u32,
    pub reason: &'static str,
}

/// Parse a known_hosts file's contents (already read through `local_path` by the caller).
pub fn parse(
    path: &Path,
    display: &str,
    origin: Origin,
    contents: &[u8],
) -> (KnownHostsFile, Vec<ParseIssue>) {
    let mut entries = Vec::new();
    let mut issues = Vec::new();
    for (index, raw) in contents.split(|b| *b == b'\n').enumerate() {
        let line = u32::try_from(index).unwrap_or(u32::MAX).saturating_add(1);
        let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
        let Ok(text) = std::str::from_utf8(raw) else {
            issues.push(ParseIssue {
                line,
                reason: "not valid UTF-8",
            });
            continue;
        };
        match parse_line(text) {
            Ok(Some((marker, hosts, key_type, blob))) => {
                entries.push(Entry {
                    line,
                    marker,
                    hosts,
                    key_type,
                    blob,
                });
            }
            Ok(None) => {}
            Err(reason) => issues.push(ParseIssue { line, reason }),
        }
    }
    let file = KnownHostsFile {
        path: path.to_path_buf(),
        display: display.to_owned(),
        origin,
        entries,
    };
    (file, issues)
}

type ParsedLine = (Marker, HostPatterns, String, Vec<u8>);

fn parse_line(text: &str) -> Result<Option<ParsedLine>, &'static str> {
    let text = text.trim_start_matches([' ', '\t']);
    if text.is_empty() || text.starts_with('#') {
        return Ok(None);
    }
    let mut tokens = text.split_ascii_whitespace();
    let mut first = tokens.next().ok_or("empty line")?;
    let marker = if first.starts_with('@') {
        let marker = match first {
            "@revoked" => Marker::Revoked,
            "@cert-authority" => Marker::CertAuthority,
            _ => return Err("unknown marker"),
        };
        first = tokens.next().ok_or("missing host pattern")?;
        marker
    } else {
        Marker::None
    };
    let hosts = parse_hosts(first)?;
    let key_type = tokens.next().ok_or("missing key type")?;
    if key_type.is_empty() || key_type.chars().any(|c| c.is_control()) {
        return Err("invalid key type");
    }
    let b64 = tokens.next().ok_or("missing key")?;
    let blob = STANDARD.decode(b64).map_err(|_| "invalid base64 key")?;
    if blob_key_type(&blob) != Some(key_type) {
        return Err("key type does not match the key blob");
    }
    Ok(Some((marker, hosts, key_type.to_owned(), blob)))
}

const SHA1_LEN: usize = 20;

fn parse_hosts(field: &str) -> Result<HostPatterns, &'static str> {
    if let Some(rest) = field.strip_prefix("|1|") {
        let (salt, hash) = rest.split_once('|').ok_or("invalid hashed host")?;
        let salt = STANDARD
            .decode(salt)
            .map_err(|_| "invalid hashed host salt")?;
        let hash = STANDARD
            .decode(hash)
            .map_err(|_| "invalid hashed host hash")?;
        if salt.len() != SHA1_LEN || hash.len() != SHA1_LEN {
            return Err("invalid hashed host length");
        }
        return Ok(HostPatterns::Hashed { salt, hash });
    }
    if field.starts_with('|') {
        return Err("unsupported hashed host format");
    }
    let mut patterns = Vec::new();
    for part in field.split(',') {
        let (negated, pattern) = match part.strip_prefix('!') {
            Some(p) => (true, p),
            None => (false, part),
        };
        if pattern.is_empty() {
            return Err("empty host pattern");
        }
        patterns.push(HostPattern {
            negated,
            pattern: pattern.to_ascii_lowercase(),
        });
    }
    Ok(HostPatterns::List(patterns))
}

/// Glob match with `*` (any sequence) and `?` (one character) over the whole string; every
/// other character (including `[`, `]`) is literal — OpenSSH `match_pattern`.
///
/// Works on the UTF-8 bytes without allocating; `?` and the `*` backtracking step move
/// by whole characters, so non-ASCII text matches exactly as a `char`-wise match would. Greedy
/// single-star backtracking: O(pattern × text) worst case, never exponential.
pub(crate) fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p = pattern.as_bytes();
    let t = text.as_bytes();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut mark = 0usize;
    while ti < t.len() {
        match p.get(pi) {
            Some(b'?') => {
                pi += utf8_len(p.get(pi).copied());
                ti += utf8_len(t.get(ti).copied());
            }
            Some(b'*') => {
                star = Some(pi);
                pi += 1;
                mark = ti;
            }
            Some(c) if Some(c) == t.get(ti) => {
                pi += 1;
                ti += 1;
            }
            _ => match star {
                Some(s) => {
                    pi = s + 1;
                    mark += utf8_len(t.get(mark).copied());
                    ti = mark;
                }
                None => return false,
            },
        }
    }
    while p.get(pi) == Some(&b'*') {
        pi += 1;
    }
    pi == p.len()
}

/// Length of the UTF-8 sequence starting with `lead` (1 for ASCII, a stray continuation byte or
/// the end of the input).
fn utf8_len(lead: Option<u8>) -> usize {
    match lead {
        Some(b) if b >= 0xF0 => 4,
        Some(b) if b >= 0xE0 => 3,
        Some(b) if b >= 0xC0 => 2,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use crate::known_hosts::test_keys::{b64, ed25519, rsa};

    const HASHED_EXAMPLE_COM: &str = "|1|O33ESRMWPVkMYIwJ1Uw+n877jTo=|nuuC5vEqXlEZ/8BXQR7m619W6Ak=";

    fn parse_str(s: &str) -> (KnownHostsFile, Vec<ParseIssue>) {
        parse(
            Path::new("/tmp/known_hosts"),
            "known_hosts",
            Origin::User,
            s.as_bytes(),
        )
    }

    #[test]
    fn wildcards() {
        assert!(wildcard_match("*.example.com", "a.example.com"));
        assert!(!wildcard_match("*.example.com", "example.com"));
        assert!(wildcard_match("host?", "host1"));
        assert!(!wildcard_match("host?", "host12"));
        assert!(wildcard_match("*", "[h]:2222"));
        assert!(wildcard_match("[h]:22*", "[h]:2222"));
        assert!(wildcard_match("a*b*c", "aXbYbZc"));
        assert!(!wildcard_match("a*b*c", "aXbYbZ"));
        assert!(wildcard_match("", ""));
        assert!(!wildcard_match("", "x"));
    }

    /// The previous `Vec<char>` implementation, as the reference for the byte-wise one.
    fn reference_match(pattern: &str, text: &str) -> bool {
        let p: Vec<char> = pattern.chars().collect();
        let t: Vec<char> = text.chars().collect();
        let (mut pi, mut ti) = (0usize, 0usize);
        let mut star: Option<usize> = None;
        let mut mark = 0usize;
        while ti < t.len() {
            match p.get(pi) {
                Some('?') => {
                    pi += 1;
                    ti += 1;
                }
                Some('*') => {
                    star = Some(pi);
                    pi += 1;
                    mark = ti;
                }
                Some(c) if Some(c) == t.get(ti) => {
                    pi += 1;
                    ti += 1;
                }
                _ => match star {
                    Some(s) => {
                        pi = s + 1;
                        mark += 1;
                        ti = mark;
                    }
                    None => return false,
                },
            }
        }
        while p.get(pi) == Some(&'*') {
            pi += 1;
        }
        pi == p.len()
    }

    /// The byte-wise matcher agrees with the char-wise one, including non-ASCII text
    /// (`?` is one character, not one byte) and characters sharing a UTF-8 lead byte.
    #[test]
    fn wildcards_bytewise_equal_charwise() {
        assert!(wildcard_match("h?st", "hést"));
        assert!(!wildcard_match("h??st", "hést"));
        assert!(wildcard_match("*é", "aaé"));
        assert!(!wildcard_match("*è", "aaé"), "same lead byte, other char");
        assert!(wildcard_match("?", "€"));
        assert!(wildcard_match("?", "😀"));
        let alphabet = ["a", "é", "è", "€", "😀", "*", "?"];
        let mut words = vec![String::new()];
        for _ in 0..3 {
            let mut next = Vec::new();
            for w in &words {
                for c in alphabet {
                    next.push(format!("{w}{c}"));
                }
            }
            words.extend(next);
        }
        let texts: Vec<&String> = words.iter().filter(|w| !w.contains(['*', '?'])).collect();
        for p in &words {
            for t in &texts {
                assert_eq!(
                    wildcard_match(p, t),
                    reference_match(p, t),
                    "{p:?} vs {t:?}"
                );
            }
        }
    }

    /// Hostile patterns: many stars against a long near-miss text stay polynomial.
    #[test]
    fn pathological_wildcards_are_fast() {
        let pattern = format!("{}b", "*a".repeat(500));
        let text = "a".repeat(4000);
        let start = std::time::Instant::now();
        assert!(!wildcard_match(&pattern, &text));
        assert!(wildcard_match(&pattern, &format!("{text}b")));
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn hashed_vector_matches_example_com() {
        let (f, issues) = parse_str(&format!(
            "{HASHED_EXAMPLE_COM} ssh-ed25519 {}\n",
            b64(&ed25519(1))
        ));
        assert!(issues.is_empty(), "{issues:?}");
        let hosts = &f.entries[0].hosts;
        assert!(matches!(hosts, HostPatterns::Hashed { .. }));
        assert!(hosts.matches("example.com"));
        assert!(!hosts.matches("example.org"));
        assert!(!hosts.matches("[example.com]:2222"));
    }

    #[test]
    fn plain_lists_negation_and_ports() {
        let k = b64(&ed25519(1));
        let (f, issues) = parse_str(&format!(
            "# comment\n\n\t  \nHost1,host2.Example.com,*.int,!bad.int ssh-ed25519 {k} comment words\n\
             [h]:2222 ssh-ed25519 {k}\r\n"
        ));
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(f.entries.len(), 2);
        assert_eq!(f.entries[0].line, 4);
        assert_eq!(f.entries[1].line, 5);
        let h = &f.entries[0].hosts;
        assert!(h.matches("host1"));
        assert!(h.matches("HOST2.example.com"));
        assert!(h.matches("db.int"));
        assert!(!h.matches("bad.int"), "negation excludes the line");
        assert!(!h.matches("host3"));
        assert!(f.entries[1].hosts.matches("[h]:2222"));
        assert!(!f.entries[1].hosts.matches("h"));
    }

    #[test]
    fn markers() {
        let k = b64(&ed25519(1));
        let (f, issues) = parse_str(&format!(
            "@revoked * ssh-ed25519 {k}\n@cert-authority *.example.com ssh-rsa {}\n",
            b64(&rsa(2))
        ));
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(f.entries[0].marker, Marker::Revoked);
        assert_eq!(f.entries[1].marker, Marker::CertAuthority);
        assert_eq!(f.entries[1].key_type, "ssh-rsa");
    }

    #[test]
    fn malformed_lines_are_skipped_and_reported() {
        let k = b64(&ed25519(1));
        let text = format!(
            "host\n\
             host ssh-ed25519\n\
             host ssh-ed25519 !!!notbase64\n\
             host ssh-rsa {k}\n\
             @bogus host ssh-ed25519 {k}\n\
             @revoked\n\
             |1|abc ssh-ed25519 {k}\n\
             |1|AAAA|AAAA ssh-ed25519 {k}\n\
             |2|x|y ssh-ed25519 {k}\n\
             a,,b ssh-ed25519 {k}\n\
             host 1024 35 1234567\n\
             good ssh-ed25519 {k}\n"
        );
        let (f, issues) = parse_str(&text);
        assert_eq!(f.entries.len(), 1);
        assert_eq!(f.entries[0].line, 12);
        assert_eq!(issues.len(), 11, "{issues:?}");
        // invalid UTF-8 line
        let mut bytes = b"\xff\xfe ssh-ed25519 x\n".to_vec();
        bytes.extend_from_slice(format!("ok ssh-ed25519 {k}").as_bytes());
        let (f, issues) = parse(Path::new("/x"), "x", Origin::Global, &bytes);
        assert_eq!(f.entries.len(), 1);
        assert_eq!(
            issues,
            vec![ParseIssue {
                line: 1,
                reason: "not valid UTF-8"
            }]
        );
    }
}
