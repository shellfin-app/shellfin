//! Error reports (§9.6, `app_error_report`): the redacted Markdown report for the error details
//! dialog and the pre-filled GitHub bug form URL. Nothing is sent anywhere: the frontend copies
//! the report and opens the URL through `app_open_external` (native confirm) when the user asks.
//!
//! **Redaction is always on.** Every text that goes into the report (message, params, detail,
//! context, stack, log lines) is redacted before it is bounded and escaped:
//! - known values from the app's own state: bookmark hosts, users, names, identity files and
//!   start directories; ssh config aliases, `HostName`s, users and identity files; recents and
//!   open sessions (with their resolved hops and remote directories); the default user; the
//!   local user and computer name; the home directory (also with `/` and doubled `\`
//!   separators); IDN hosts also in punycode. Case-insensitive, whole tokens only, longest
//!   match first, values shorter than 2 characters are ignored;
//! - patterns: IPv4/IPv6 addresses (loopback and unspecified stay; also right after `word:`),
//!   `user@host` (also `user%40host`, `user@[v6]`), `SHA256:`/`MD5:` fingerprints, values of
//!   host/user/path-like and credential keys (`host: "…"`, `path=…`, `"remotePath":"…"`, string
//!   lists, `password: …`, `?token=…&sig=…`, `Authorization: Bearer …`), removal hints, and
//!   absolute paths in free text (POSIX, `host:/…`, `file:///…`, `X:\…`, UNC, `\\?\…`,
//!   `~name/…`, continued across spaces or to the closing quote);
//! - well-formed ANSI colour/cursor sequences are removed first (a space left between two
//!   values), so they never hide a value;
//! - placeholders: `<host>`, `<ip>`, `<user>`, `~` (home), `<path>`, `<redacted>`.
//!
//! Then control, bidi and invisible characters become visible escapes like the UI's
//! `displayServerText` (`\x1b`, `⟨U+202E⟩`), so nothing invisible reaches the clipboard or the URL.

use std::collections::{BTreeMap, HashMap};
use std::net::{Ipv4Addr, Ipv6Addr};

use crate::external_link::{MAX_URL_CHARS, normalize_external_url};
use crate::ipc::dto::{ErrorReport, ReportEnvironment};

/// The bug form of `.github/ISSUE_TEMPLATE/`.
pub const ISSUE_TEMPLATE: &str = "bug_report.yml";

/// Limits of the report parts (characters, after redaction).
pub const MAX_MESSAGE_CHARS: usize = 2000;
pub const MAX_DETAIL_CHARS: usize = 4000;
pub const MAX_PARAMS_CHARS: usize = 4000;
pub const MAX_STACK_LINES: usize = 100;
pub const MAX_STACK_CHARS: usize = 16_000;
pub const MAX_LINE_CHARS: usize = 2000;
/// The error message in the URL's `actual` field (the full one is in the copied report).
pub const MAX_URL_MESSAGE_CHARS: usize = 300;
/// Raw input read at most per text (cut at whitespace), before redaction.
const MAX_RAW_CHARS: usize = 64 * 1024;

// ---------------------------------------------------------------------------------------------
// redaction
// ---------------------------------------------------------------------------------------------

/// What a redacted value is replaced with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placeholder {
    Host,
    Ip,
    User,
    Home,
    Path,
    Redacted,
}

impl Placeholder {
    pub const fn text(self) -> &'static str {
        match self {
            Self::Host => "<host>",
            Self::Ip => "<ip>",
            Self::User => "<user>",
            Self::Home => "~",
            Self::Path => "<path>",
            Self::Redacted => "<redacted>",
        }
    }
}

/// Sensitive values known from the app's own state.
#[derive(Debug, Default, Clone)]
pub struct KnownValues {
    values: Vec<(String, Placeholder)>,
    homes: Vec<String>,
}

/// Values shorter than this are not matched (they would hit ordinary words).
const MIN_KNOWN_CHARS: usize = 2;

impl KnownValues {
    pub fn add(&mut self, value: &str, kind: Placeholder) {
        let value = value.trim();
        if value.chars().count() < MIN_KNOWN_CHARS || value.chars().all(|c| !c.is_alphanumeric()) {
            return;
        }
        for variant in path_variants(value) {
            self.values.push((variant, kind));
        }
    }

    pub fn add_host(&mut self, host: &str) {
        let host = host.trim();
        // `[2001:db8::1]` is the bracketed form of an IPv6 address.
        let unbracketed = host
            .strip_prefix('[')
            .and_then(|h| h.strip_suffix(']'))
            .filter(|h| parse_ip(h).is_some())
            .unwrap_or(host);
        let kind = if parse_ip(unbracketed).is_some() {
            Placeholder::Ip
        } else {
            Placeholder::Host
        };
        self.add(unbracketed, kind);
        // An internationalised name also appears in its ASCII (punycode) form, e.g. in URLs.
        if kind == Placeholder::Host
            && !unbracketed.is_ascii()
            && let Ok(url::Host::Domain(ascii)) = url::Host::parse(unbracketed)
        {
            self.add(&ascii, Placeholder::Host);
        }
    }

    pub fn add_user(&mut self, user: &str) {
        self.add(user, Placeholder::User);
    }

    pub fn add_path(&mut self, path: &str) {
        self.add(path, Placeholder::Path);
    }

    /// `[user@]host[:port]`, also comma-separated jump lists (`ProxyJump`, quick-connect specs,
    /// `user@host` texts of recents).
    pub fn add_host_spec(&mut self, spec: &str) {
        for part in spec.split(',') {
            let part = part.trim();
            let (user, host) = match part.rsplit_once('@') {
                Some((u, h)) => (Some(u), h),
                None => (None, part),
            };
            if let Some(user) = user {
                self.add_user(user);
            }
            let host = if let Some(rest) = host.strip_prefix('[') {
                rest.split(']').next().unwrap_or(rest)
            } else if host.matches(':').count() == 1 {
                host.split(':').next().unwrap_or(host)
            } else {
                host
            };
            self.add_host(host);
        }
    }

    /// The local home directory: replaced with `~`, never with `<path>`.
    pub fn add_home(&mut self, home: &str) {
        let home = home.trim().trim_end_matches(['/', '\\']);
        if home.chars().count() < MIN_KNOWN_CHARS {
            return;
        }
        for variant in path_variants(home) {
            self.homes.push(variant.clone());
            self.values.push((variant, Placeholder::Home));
        }
    }
}

/// A value plus the spellings it has in logs and JSON: `/` for `\`, and `\\` (Debug and JSON
/// escape backslashes).
fn path_variants(value: &str) -> Vec<String> {
    let mut out = vec![value.to_owned()];
    if value.contains('\\') {
        out.push(value.replace('\\', "/"));
        out.push(value.replace('\\', "\\\\"));
    }
    out
}

fn fold(c: char) -> char {
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(l), None) => l,
        _ => c,
    }
}

/// Characters that continue a token (a match must not be followed or preceded by one).
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

fn parse_ip(s: &str) -> Option<std::net::IpAddr> {
    s.parse::<std::net::IpAddr>().ok()
}

/// The text without well-formed ANSI colour and cursor sequences, so they never hide a value from
/// the patterns: `ESC [` or C1 `CSI`, an optional `?`, parameters `[0-9;:]*`, then `m` (SGR,
/// also bare `\x1b[m`) or, only after at least one parameter, a cursor/erase/mode final
/// (`A`–`H`, `J`, `K`, `S`, `T`, `f`, `h`, `l`). Anything else stays and is escaped later, so a
/// bare `ESC [` never swallows the first character of a value (`\x1b[alice`, `\x1b[C:\…`).
/// Where a removed sequence sat between two non-space characters a space is left, so a value is
/// not glued to its neighbour (`10.1.2.3\x1b[0mabc`), except next to `.` or `:`, which continue
/// an address or host (`10.1.2\x1b[m.3`, `failed:\x1b[31m10.9.8.7`).
fn strip_csi(text: &str) -> Vec<char> {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(chars.len());
    let mut removed = false;
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        let body = match (c, chars.get(i + 1)) {
            ('\u{1b}', Some('[')) => Some(i + 2),
            ('\u{9b}', _) => Some(i + 1),
            _ => None,
        };
        if let Some(body) = body {
            let mut k = body;
            if chars.get(k) == Some(&'?') {
                k += 1;
            }
            let params = k;
            while chars
                .get(k)
                .is_some_and(|&c| c.is_ascii_digit() || c == ';' || c == ':')
            {
                k += 1;
            }
            let has_params = k > params;
            let well_formed = chars.get(k).is_some_and(|&f| {
                f == 'm'
                    || (has_params
                        && matches!(f, 'A'..='H' | 'J' | 'K' | 'S' | 'T' | 'f' | 'h' | 'l'))
            });
            if well_formed {
                removed = true;
                i = k + 1;
                continue;
            }
        }
        if removed
            && let Some(&last) = out.last()
            && !last.is_whitespace()
            && !c.is_whitespace()
            && !matches!(last, '.' | ':')
            && !matches!(c, '.' | ':')
        {
            out.push(' ');
        }
        removed = false;
        out.push(c);
        i += 1;
    }
    out
}

/// A replacement of `text[start..end]` (char indices).
#[derive(Debug, Clone)]
struct Span {
    start: usize,
    end: usize,
    with: String,
}

/// Redacts text with the known values and the patterns (module docs).
pub struct Redactor {
    /// Folded first char → (folded value, placeholder), longest first.
    index: HashMap<char, Vec<(Vec<char>, Placeholder)>>,
    homes: Vec<Vec<char>>,
}

impl Redactor {
    pub fn new(known: &KnownValues) -> Self {
        let mut index: HashMap<char, Vec<(Vec<char>, Placeholder)>> = HashMap::new();
        for (value, kind) in &known.values {
            let folded: Vec<char> = value.chars().map(fold).collect();
            if let Some(&first) = folded.first() {
                index.entry(first).or_default().push((folded, *kind));
            }
        }
        for list in index.values_mut() {
            // Longest first; on equal length the more specific kind (home before path before host).
            list.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then(rank(a.1).cmp(&rank(b.1))));
            list.dedup_by(|a, b| a.0 == b.0);
        }
        let homes = known
            .homes
            .iter()
            .map(|h| h.chars().map(fold).collect())
            .collect();
        Self { index, homes }
    }

    /// Redact one text (any number of lines).
    pub fn redact(&self, text: &str) -> String {
        let chars = strip_csi(text);
        let mut spans = Vec::new();
        ip_spans(&chars, &mut spans);
        user_at_host_spans(&chars, &mut spans);
        fingerprint_spans(&chars, &mut spans);
        self.key_value_spans(&chars, &mut spans);
        self.known_spans(&chars, &mut spans);
        self.path_spans(&chars, &mut spans);
        apply_spans(&chars, spans)
    }

    fn starts_with_home(&self, chars: &[char]) -> bool {
        self.homes.iter().any(|home| {
            chars.len() >= home.len()
                && chars.iter().zip(home).all(|(&a, &b)| fold(a) == b)
                && chars
                    .get(home.len())
                    .is_none_or(|&c| !is_word(c) && c != '.')
        })
    }

    fn known_spans(&self, chars: &[char], spans: &mut Vec<Span>) {
        for i in 0..chars.len() {
            if i > 0 && chars.get(i - 1).is_some_and(|&c| is_word(c)) {
                continue;
            }
            let Some(&c) = chars.get(i) else { break };
            let Some(candidates) = self.index.get(&fold(c)) else {
                continue;
            };
            for (value, kind) in candidates {
                let end = i + value.len();
                let Some(window) = chars.get(i..end) else {
                    continue;
                };
                if window.iter().zip(value).all(|(&a, &b)| fold(a) == b)
                    && right_boundary(chars, end)
                {
                    spans.push(Span {
                        start: i,
                        end,
                        with: kind.text().to_owned(),
                    });
                    break;
                }
            }
        }
    }

    /// `key: "value"`, `key: Some("value")`, `key="value"`, `key=value`, `"key":"value"`.
    fn key_value_spans(&self, chars: &[char], spans: &mut Vec<Span>) {
        let mut i = 0;
        while i < chars.len() {
            let starts_ident = chars
                .get(i)
                .is_some_and(|&c| c.is_ascii_alphabetic() || c == '_')
                && (i == 0
                    || chars
                        .get(i - 1)
                        .is_none_or(|&c| !c.is_alphanumeric() && c != '_'));
            if !starts_ident {
                i += 1;
                continue;
            }
            let key_start = i;
            // `X-Api-Key`, `set-cookie`: a `-` inside a key (followed by a letter or digit).
            while chars.get(i).is_some_and(|&c| {
                c.is_ascii_alphanumeric()
                    || c == '_'
                    || (c == '-' && chars.get(i + 1).is_some_and(char::is_ascii_alphanumeric))
            }) {
                i += 1;
            }
            let normalise = |raw: &[char]| -> String {
                raw.iter()
                    .filter(|&&c| c != '_' && c != '-')
                    .map(|c| c.to_ascii_lowercase())
                    .collect()
            };
            let raw = chars.get(key_start..i).unwrap_or_default();
            let mut key = normalise(raw);
            // `my-host=…` is still a host key: the last `-` segment when the whole has no class.
            if key_class(&key).is_none()
                && let Some(dash) = raw.iter().rposition(|&c| c == '-')
            {
                key = normalise(raw.get(dash + 1..).unwrap_or_default());
            }
            let Some(class) = key_class(&key) else {
                continue;
            };
            let secret = is_secret_key(&key);
            // `?token=abc&sig=zzz`: in a query string `&`, `;` and `#` end a value.
            let query = key_start > 0 && matches!(chars.get(key_start - 1), Some('?' | '&'));
            let mut j = i;
            // JSON: the key's closing quote.
            if key_start > 0 && chars.get(key_start - 1) == Some(&'"') && chars.get(j) == Some(&'"')
            {
                j += 1;
            }
            while chars.get(j) == Some(&' ') {
                j += 1;
            }
            let equals = match chars.get(j) {
                Some(':') => false,
                Some('=') => true,
                _ => continue,
            };
            j += 1;
            while chars.get(j) == Some(&' ') {
                j += 1;
            }
            if chars
                .get(j..j + 5)
                .is_some_and(|s| s == ['S', 'o', 'm', 'e', '('])
            {
                j += 5;
            }
            // A list of strings (`"chain":["a","b"]`, `chain: ["a", "b"]`): each element.
            if chars.get(j) == Some(&'[') {
                let mut k = j + 1;
                loop {
                    while chars.get(k).is_some_and(|&c| c == ' ' || c == ',') {
                        k += 1;
                    }
                    if chars.get(k) != Some(&'"') {
                        break;
                    }
                    let (start, end) = quoted_value(chars, k);
                    if let Some(with) = chars
                        .get(start..end)
                        .and_then(|value| self.key_value_replacement(class, value))
                    {
                        spans.push(Span { start, end, with });
                    }
                    k = end + 1;
                }
                i = k.max(i);
                continue;
            }
            // `Cookie: a=1; b=2`: the whole rest of the line.
            let whole_line = matches!(key.as_str(), "cookie" | "setcookie");
            let ends_value = |c: char| {
                if whole_line {
                    return c == '\n' || c == '\r';
                }
                c.is_whitespace()
                    || matches!(c, ',' | ')' | '}' | ']')
                    || (secret && matches!(c, '"' | '\''))
                    || (query && matches!(c, '&' | ';' | '#'))
            };
            let word_end = |from: usize| {
                let mut k = from;
                while chars.get(k).is_some_and(|&c| !ends_value(c)) {
                    k += 1;
                }
                k
            };
            let (start, end) = if chars.get(j) == Some(&'{') {
                // `"auth": {"method": …}`: an object; its own keys are matched one by one.
                continue;
            } else if chars.get(j) == Some(&'"') {
                quoted_value(chars, j)
            } else if secret && chars.get(j) == Some(&'\'') {
                // `password='a b'`: to the closing quote on the same line (bounded), else the word.
                chars
                    .iter()
                    .enumerate()
                    .skip(j + 1)
                    .take(MAX_QUOTED_PATH_CHARS)
                    .take_while(|&(_, &c)| c != '\n' && c != '\r')
                    .find(|&(_, &c)| c == '\'')
                    .map_or_else(|| (j + 1, word_end(j + 1)), |(k, _)| (j + 1, k))
            } else if secret || (equals && class != Placeholder::Redacted) {
                let mut k = word_end(j);
                // `Authorization: Bearer <token>`: the credentials after the scheme too.
                let scheme: String = chars
                    .get(j..k)
                    .unwrap_or_default()
                    .iter()
                    .map(|c| c.to_ascii_lowercase())
                    .collect();
                if secret
                    && matches!(
                        scheme.as_str(),
                        "bearer" | "basic" | "token" | "digest" | "negotiate" | "ntlm"
                    )
                    && chars.get(k) == Some(&' ')
                    && chars.get(k + 1).is_some_and(|&c| !ends_value(c))
                {
                    k = word_end(k + 1);
                }
                (j, k)
            } else {
                continue;
            };
            let Some(value) = chars.get(start..end) else {
                continue;
            };
            if let Some(with) = self.key_value_replacement(class, value) {
                spans.push(Span { start, end, with });
            }
            i = end.max(i);
        }
    }

    fn key_value_replacement(&self, class: Placeholder, value: &[char]) -> Option<String> {
        if value.is_empty() {
            return None;
        }
        let text: String = value.iter().collect();
        if text.starts_with('<') && text.ends_with('>') {
            return None; // already a placeholder
        }
        if value.iter().all(|&c| c == '*') {
            return None; // already masked (`password=***` in a log line)
        }
        match class {
            // Bare file names (`bookmarks.json`) and relative source paths of panic locations
            // stay; the home becomes `~` in the known-values pass.
            Placeholder::Path => self
                .is_private_path(value)
                .then(|| Placeholder::Path.text().to_owned()),
            // `host:port` keeps the port (`peer=<ip>:22`).
            Placeholder::Host => {
                let (host, port) = split_port(&text);
                let placeholder =
                    if parse_ip(host.trim_start_matches('[').trim_end_matches(']')).is_some() {
                        Placeholder::Ip
                    } else {
                        Placeholder::Host
                    };
                Some(match port {
                    Some(port) => format!("{}:{port}", placeholder.text()),
                    None => placeholder.text().to_owned(),
                })
            }
            other => Some(other.text().to_owned()),
        }
    }

    /// An absolute path (POSIX `/…`, `\…` and UNC `\\server\…`, `X:\…`, `X:/…`, another user's
    /// `~name/…`) that is neither under the home directory nor a toolchain path of a panic
    /// location.
    fn is_private_path(&self, value: &[char]) -> bool {
        let absolute = match value {
            ['/' | '\\', ..] => true,
            [drive, ':', '/' | '\\', ..] => drive.is_ascii_alphabetic(),
            ['~', first, ..] => first.is_alphabetic(),
            _ => false,
        };
        if !absolute || self.starts_with_home(value) {
            return false;
        }
        let text: String = value.iter().collect::<String>().replace('\\', "/");
        !(text.starts_with("/rustc/") || text.contains("/.cargo/registry/"))
    }

    /// Absolute paths in free text (remote paths, local paths outside the home): POSIX `/…`
    /// (also after `host:` as in `scp` specs), Windows `X:\…` / `X:/…`, UNC `\\server\…` and
    /// `~name/…`. A path continues across a space when the next word contains a separator
    /// (`/srv/My Project/notes.txt`). The home directory and toolchain paths stay.
    fn path_spans(&self, chars: &[char], spans: &mut Vec<Span>) {
        let mut i = 0;
        while i < chars.len() {
            if !path_starts_at(chars, i) {
                i += 1;
                continue;
            }
            let start = i;
            let mut end = path_token_end(chars, i);
            // `'/opt/Client Files/a b.txt'`: a quoted path runs to its closing quote on the same
            // line. The search is bounded, and an unclosed quote falls back to the rules below.
            let closing = start
                .checked_sub(1)
                .and_then(|k| chars.get(k).copied())
                .filter(|&q| matches!(q, '\'' | '"' | '`'))
                .and_then(|q| {
                    chars
                        .iter()
                        .enumerate()
                        .skip(end)
                        .take(MAX_QUOTED_PATH_CHARS)
                        .take_while(|&(_, &c)| c != '\n' && c != '\r')
                        .find(|&(_, &c)| c == q)
                        .map(|(k, _)| k)
                });
            if let Some(close) = closing {
                end = close;
            }
            // `/srv/My Project/notes.txt`: one space, then a word with a separator in it that
            // is not a path, URL or `key=value` of its own.
            while closing.is_none() && chars.get(end) == Some(&' ') {
                let next_end = path_token_end(chars, end + 1);
                let word = chars.get(end + 1..next_end).unwrap_or_default();
                let continues = !word.is_empty()
                    && !path_starts_at(chars, end + 1)
                    && word.iter().any(|&c| c == '/' || c == '\\')
                    && !word.contains(&'=')
                    && !word.windows(3).any(|w| w == [':', '/', '/']);
                if !continues {
                    break;
                }
                end = next_end;
            }
            while end > start + 1
                && chars
                    .get(end - 1)
                    .is_some_and(|&c| matches!(c, '.' | ':' | ';'))
            {
                end -= 1;
            }
            let token = chars.get(start..end).unwrap_or_default();
            // `file://server/share/x`: a UNC path without its leading slashes.
            let file_host = after_file_scheme(chars, start)
                && token.first().is_some_and(|&c| c != '/')
                && !self.starts_with_home(token);
            if file_host || self.is_private_path(token) {
                spans.push(Span {
                    start,
                    end,
                    with: Placeholder::Path.text().to_owned(),
                });
            }
            i = end.max(start + 1);
        }
    }
}

/// `host:22` / `[2001:db8::1]:22` → (`host`, `22`); a bare IPv6 address has no port.
fn split_port(text: &str) -> (&str, Option<&str>) {
    if let Some((host, port)) = text.rsplit_once(':')
        && !port.is_empty()
        && port.len() <= 5
        && port.chars().all(|c| c.is_ascii_digit())
        && (!host.contains(':') || (host.starts_with('[') && host.ends_with(']')))
    {
        return (host, Some(port));
    }
    (text, None)
}

/// The value of a Debug/JSON string whose opening quote is at `quote`: `(start, end)` without
/// the quotes; `\"` and `\\` do not end it.
fn quoted_value(chars: &[char], quote: usize) -> (usize, usize) {
    let start = quote + 1;
    let mut k = start;
    while let Some(&c) = chars.get(k) {
        if c == '\\' {
            k += 2;
            continue;
        }
        if c == '"' {
            break;
        }
        k += 1;
    }
    (start, k.min(chars.len()))
}

/// Characters that end a path token in free text.
fn ends_path(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '"' | '\'' | '`' | ',' | ')' | ']' | '}' | '<' | '>' | '”' | '’' | '»'
        )
}

/// `i` follows a `file://` scheme that is not inside a word (case-insensitive).
fn after_file_scheme(chars: &[char], i: usize) -> bool {
    i >= 7
        && chars
            .get(i - 7..i)
            .is_some_and(|s| s.iter().zip("file://".chars()).all(|(&a, b)| fold(a) == b))
        && (i == 7 || chars.get(i - 8).is_none_or(|&c| !is_word(c)))
}

/// The longest user name `~name/…` is recognised with (Linux allows 32, some systems more).
const MAX_TILDE_USER_CHARS: usize = 64;

/// How far a quoted path (`'/opt/Client Files/a b.txt'`) is followed to its closing quote.
const MAX_QUOTED_PATH_CHARS: usize = 4096;

/// The end of the path token starting at `i`.
fn path_token_end(chars: &[char], i: usize) -> usize {
    let mut end = i;
    while chars.get(end).is_some_and(|&c| !ends_path(c)) {
        end += 1;
    }
    end
}

/// A path in free text starts at `i`: `/…` after whitespace or punctuation (also `host:/…`,
/// never `://` or a drive's `C:/`, but the path of `file:///…`), `X:\…` / `X:/…`, `\\server…`
/// (UNC, also with doubled backslashes) and verbatim `\\?\…`, or `~name/…`; never inside a word.
fn path_starts_at(chars: &[char], i: usize) -> bool {
    let at = |k: usize| chars.get(k).copied();
    let prev = i.checked_sub(1).and_then(at);
    let boundary = prev.is_none_or(|c| {
        !c.is_alphanumeric() && !matches!(c, '/' | '.' | '_' | '-' | '\\' | '~' | ':')
    });
    if after_file_scheme(chars, i) {
        // `file:///home/eve/x`, `file:////server/share/x`, `file://server/share/x`: everything
        // after `file://` is the path.
        return match at(i) {
            Some('/') => at(i + 1).is_some_and(|c| !c.is_whitespace() && !ends_path(c)),
            Some(c) => !ends_path(c),
            None => false,
        };
    }
    match at(i) {
        Some('/') => {
            if !at(i + 1).is_some_and(|c| !c.is_whitespace() && c != '/') {
                return false;
            }
            if prev == Some(':') {
                // `C:/…` starts at the drive letter; `prod.internal:/srv/…` starts here.
                let drive = i >= 2
                    && at(i - 2).is_some_and(|c| c.is_ascii_alphabetic())
                    && (i < 3 || at(i - 3).is_none_or(|c| !is_word(c) && c != '.'));
                return !drive && i >= 2;
            }
            boundary
        }
        Some(c) if c.is_ascii_alphabetic() => {
            boundary && at(i + 1) == Some(':') && matches!(at(i + 2), Some('\\' | '/'))
        }
        Some('\\') => {
            // Check the boundary before scanning: inside a run of backslashes every position but
            // the first fails it, so the scan stays linear overall.
            if !boundary {
                return false;
            }
            let mut k = i;
            while at(k) == Some('\\') {
                k += 1;
            }
            // `\\server\…`, or a verbatim `\\?\D:\…` / `\\?\UNC\server\…`.
            k - i >= 2
                && (at(k).is_some_and(is_word) || (at(k) == Some('?') && at(k + 1) == Some('\\')))
        }
        Some('~') => {
            // `~name/`: the `/` must follow within a user name's length, so a `~` never scans
            // to the end of a long token.
            boundary
                && at(i + 1).is_some_and(char::is_alphabetic)
                && chars
                    .iter()
                    .skip(i + 1)
                    .take(MAX_TILDE_USER_CHARS + 1)
                    .take_while(|&&c| !ends_path(c))
                    .any(|&c| c == '/')
        }
        _ => false,
    }
}

const fn rank(p: Placeholder) -> u8 {
    match p {
        Placeholder::Home => 0,
        Placeholder::Ip => 1,
        Placeholder::Path => 2,
        Placeholder::User => 3,
        Placeholder::Host => 4,
        Placeholder::Redacted => 5,
    }
}

/// Classes of keys whose values are redacted as a whole (keys lower-cased, `_` removed).
fn key_class(key: &str) -> Option<Placeholder> {
    match key {
        "host" | "hostname" | "via" | "alias" | "spec" | "value" | "jumphost" | "proxyjump"
        | "server" | "addr" | "address" | "peer" | "remote" | "hostkeyalias" | "label"
        | "chain" => Some(Placeholder::Host),
        "user" | "username" | "localuser" | "remoteuser" | "defaultuser" => Some(Placeholder::User),
        "fingerprint" | "expected" | "actual" | "removehint" | "id" => Some(Placeholder::Redacted),
        k if is_secret_key(k) => Some(Placeholder::Redacted),
        "target" | "from" | "to" | "root" | "cwd" => Some(Placeholder::Path),
        k if k.ends_with("path") || k.ends_with("dir") || k.ends_with("file") => {
            Some(Placeholder::Path)
        }
        _ => None,
    }
}

/// Keys whose values are credentials (lower-cased, `_` removed): also unquoted values after `:`
/// and `=` become `<redacted>`, and the credentials after an `Authorization` scheme.
fn is_secret_key(key: &str) -> bool {
    matches!(
        key,
        "password"
            | "passwd"
            | "pwd"
            | "passphrase"
            | "token"
            | "secret"
            | "apikey"
            | "accesstoken"
            | "authorization"
            | "sig"
            | "signature"
            | "pass"
            | "credentials"
            | "auth"
            | "cookie"
            | "setcookie"
    ) || key.ends_with("token")
        || key.ends_with("apikey")
        || key.ends_with("secret")
        || key.ends_with("password")
        || key.ends_with("passphrase")
}

/// The match ending at `end` is a whole token: not followed by a word character, nor by `.`
/// plus a word character (`alice` in `alice.smith`).
fn right_boundary(chars: &[char], end: usize) -> bool {
    match chars.get(end) {
        None => true,
        Some(&c) if is_word(c) => false,
        Some('.') => !chars.get(end + 1).is_some_and(|&c| is_word(c)),
        Some(_) => true,
    }
}

/// An IPv6-parsable run that reads as a Rust path tail (`ab::cd`, `dead::beef`): every
/// separator is `::` and no group has a digit. Real addresses have single `:` separators
/// (`2001:db8::5`) or digits (`fe80::1234`); a hex-letters-only `x::y` is ambiguous and stays.
fn rust_path_like(run: &str) -> bool {
    let single_colon = run.split("::").any(|part| part.contains(':'));
    !single_colon && !run.chars().any(|c| c.is_ascii_digit())
}

/// IPv4 and IPv6 addresses: maximal runs of `[0-9A-Fa-f:.]` that are not part of a word.
fn ip_spans(chars: &[char], spans: &mut Vec<Span>) {
    let is_run = |c: char| c.is_ascii_hexdigit() || c == ':' || c == '.';
    let mut i = 0;
    while i < chars.len() {
        if !chars.get(i).is_some_and(|&c| is_run(c)) {
            i += 1;
            continue;
        }
        let mut start = i;
        while chars.get(i).is_some_and(|&c| is_run(c)) {
            i += 1;
        }
        let end = i;
        let mut before = start.checked_sub(1).and_then(|k| chars.get(k).copied());
        let after = chars.get(end).copied();
        // `failed:10.9.8.7:22`, `peer:fe80::1234`: the run starts inside a word (`ed:` of
        // `failed`); its first segment belongs to the word, the rest may still be an address.
        // IPv6 there unless it is shaped like a Rust path (`x:ab::cd`, `mod:dead::beef`).
        let mut trimmed = false;
        if before.is_some_and(char::is_alphanumeric)
            && let Some(colon) = chars
                .get(start..end)
                .and_then(|run| run.iter().position(|&c| c == ':'))
        {
            start += colon + 1;
            before = Some(':');
            trimmed = true;
        }
        // IPv4 may touch `_` or `-` (`_10.1.2.3`, `10.1.2.3-22`); IPv6 must not (Rust paths
        // like `x_ab::cd` would parse).
        if before.is_some_and(char::is_alphanumeric) || after.is_some_and(char::is_alphanumeric) {
            continue;
        }
        let word_bounded = !before.is_some_and(is_word) && !after.is_some_and(is_word);
        let run = chars.get(start..end).unwrap_or_default();
        // Trailing sentence punctuation.
        let mut trimmed_end = run.len();
        while trimmed_end > 0 && matches!(run.get(trimmed_end - 1), Some('.' | ':')) {
            trimmed_end -= 1;
        }
        let whole: String = run.get(..trimmed_end).unwrap_or_default().iter().collect();
        if word_bounded
            && !(trimmed && rust_path_like(&whole))
            && whole.matches(':').count() >= 2
            && let Ok(v6) = whole.parse::<Ipv6Addr>()
        {
            if !v6.is_loopback() && !v6.is_unspecified() {
                spans.push(Span {
                    start,
                    end: start + trimmed_end,
                    with: Placeholder::Ip.text().to_owned(),
                });
            }
            continue;
        }
        // IPv4 parts (`10.0.0.1:22`).
        let mut offset = 0;
        for part in whole.split(':') {
            let len = part.chars().count();
            let lead = part.len() - part.trim_start_matches('.').len();
            let core = part.trim_matches('.');
            if core.matches('.').count() == 3
                && let Ok(v4) = core.parse::<Ipv4Addr>()
                && !v4.is_loopback()
                && !v4.is_unspecified()
            {
                spans.push(Span {
                    start: start + offset + lead,
                    end: start + offset + lead + core.chars().count(),
                    with: Placeholder::Ip.text().to_owned(),
                });
            }
            offset += len + 1;
        }
    }
}

/// Key fingerprints in free text: `SHA256:<base64>` and `MD5:<hex:hex…>` keep the prefix, the
/// value becomes `<redacted>` (a host key fingerprint identifies the server).
fn fingerprint_spans(chars: &[char], spans: &mut Vec<Span>) {
    for (prefix, is_value) in [
        (
            &['S', 'H', 'A', '2', '5', '6', ':'][..],
            (|c: char| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '='))
                as fn(char) -> bool,
        ),
        (&['M', 'D', '5', ':'][..], |c: char| {
            c.is_ascii_hexdigit() || c == ':'
        }),
    ] {
        let mut i = 0;
        while i + prefix.len() <= chars.len() {
            let matches_here = chars
                .get(i..i + prefix.len())
                .is_some_and(|w| w.iter().zip(prefix).all(|(a, b)| a.eq_ignore_ascii_case(b)))
                && (i == 0 || chars.get(i - 1).is_none_or(|&c| !c.is_alphanumeric()));
            if !matches_here {
                i += 1;
                continue;
            }
            let start = i + prefix.len();
            let mut end = start;
            while chars.get(end).is_some_and(|&c| is_value(c)) {
                end += 1;
            }
            if end - start >= 16 {
                spans.push(Span {
                    start,
                    end,
                    with: Placeholder::Redacted.text().to_owned(),
                });
            }
            i = end.max(i + 1);
        }
    }
}

/// `user@host` (also e-mail addresses); not `name@scheme://…` (WebKit stack frames) and not
/// `package@1.2.3`.
fn user_at_host_spans(chars: &[char], spans: &mut Vec<Span>) {
    let user_char = |c: char| c.is_alphanumeric() || matches!(c, '.' | '_' | '-' | '+');
    let host_char = |c: char| c.is_alphanumeric() || matches!(c, '.' | '-');
    for (at, &c) in chars.iter().enumerate() {
        // `@`, or percent-encoded `%40` (`carol%40db.internal` in a URL).
        let sep = match c {
            '@' => "@",
            '%' if chars.get(at + 1..at + 3) == Some(&['4', '0'][..]) => "%40",
            _ => continue,
        };
        // The host starts after the separator.
        let h = at + sep.len();
        let mut start = at;
        loop {
            if start > 0 && chars.get(start - 1).is_some_and(|&c| user_char(c)) {
                start -= 1;
            } else if sep == "@"
                && start > 0
                && chars.get(start - 1) == Some(&'%')
                && chars
                    .get(start..start + 2)
                    .is_some_and(|s| s.iter().all(char::is_ascii_hexdigit))
            {
                // Percent-encoded userinfo before a real `@` (`carol%40example.com@host`) is one
                // user. Only for `@`: each `%40` scans its own run, keeping this linear.
                start -= 1;
            } else {
                break;
            }
        }
        // `alice@[2001:db8::3]`: a bracketed IPv6 literal (at most 45 chars plus the brackets).
        if start < at && chars.get(h) == Some(&'[') {
            let literal: String = chars
                .iter()
                .skip(h + 1)
                .take(46)
                .take_while(|&&c| c != ']')
                .collect();
            let close = h + 1 + literal.chars().count();
            if chars.get(close) == Some(&']')
                && let Ok(v6) = literal.parse::<Ipv6Addr>()
            {
                // Loopback and unspecified stay readable, as in `ip_spans`.
                let ip = if v6.is_loopback() || v6.is_unspecified() {
                    literal.as_str()
                } else {
                    Placeholder::Ip.text()
                };
                spans.push(Span {
                    start,
                    end: close + 1,
                    with: format!("{}{sep}[{ip}]", Placeholder::User.text()),
                });
            }
            continue;
        }
        let mut end = h;
        while chars.get(end).is_some_and(|&c| host_char(c)) {
            end += 1;
        }
        while end > h && chars.get(end - 1) == Some(&'.') {
            end -= 1;
        }
        if start == at || end == h {
            continue;
        }
        if chars
            .get(end..end + 3)
            .is_some_and(|s| s == [':', '/', '/'])
        {
            continue;
        }
        let host: String = chars.get(h..end).unwrap_or_default().iter().collect();
        let ip = parse_ip(&host).is_some();
        if !ip && host.chars().all(|c| c.is_ascii_digit() || c == '.') {
            continue; // a version
        }
        let host_placeholder = if ip {
            Placeholder::Ip
        } else {
            Placeholder::Host
        };
        spans.push(Span {
            start,
            end,
            with: format!(
                "{}{sep}{}",
                Placeholder::User.text(),
                host_placeholder.text()
            ),
        });
    }
}

/// Apply the spans: longest first, overlapping shorter ones dropped.
fn apply_spans(chars: &[char], mut spans: Vec<Span>) -> String {
    spans.sort_by(|a, b| {
        (b.end - b.start)
            .cmp(&(a.end - a.start))
            .then(a.start.cmp(&b.start))
    });
    // Taken spans by start. They never overlap, so ordered by start they are ordered by end too:
    // a new span overlaps one of them exactly when it overlaps the last one starting before its
    // end (a linear scan per span would be quadratic in hostile input with many matches).
    let mut taken: BTreeMap<usize, Span> = BTreeMap::new();
    for span in spans {
        if span.start >= span.end {
            continue;
        }
        let overlaps = taken
            .range(..span.end)
            .next_back()
            .is_some_and(|(_, t)| t.end > span.start);
        if !overlaps {
            taken.insert(span.start, span);
        }
    }
    let mut out = String::with_capacity(chars.len());
    let mut pos = 0;
    for span in taken.into_values() {
        out.extend(chars.get(pos..span.start).unwrap_or_default());
        out.push_str(&span.with);
        pos = span.end;
    }
    out.extend(chars.get(pos..).unwrap_or_default());
    out
}

/// What the app knows that must not reach a report: the sessions' labels and targets.
#[derive(Debug, Clone)]
pub struct OpenSession {
    pub label: String,
    pub target: crate::ipc::dto::RecentTarget,
    /// The connected hops as resolved (`HostName`, user), empty while not connected.
    pub hops: Vec<crate::ipc::dto::HopRef>,
    /// The remote home and start directories of the live connection.
    pub dirs: Vec<String>,
}

/// The local machine: user, computer name and home directory.
#[derive(Debug, Clone)]
pub struct LocalIdentity {
    pub user: String,
    pub computer: Option<String>,
    pub home: String,
}

/// Collect the known sensitive values from the app's state (module docs).
pub fn collect_known(
    bookmarks: &crate::ipc::dto::BookmarksDto,
    ssh_hosts: &[crate::ipc::dto::SshConfigHost],
    recents: &[crate::ipc::dto::RecentEntry],
    sessions: &[OpenSession],
    default_user: &str,
    local: &LocalIdentity,
) -> KnownValues {
    use crate::ipc::dto::{AuthConfig, JumpRef, RecentTarget};
    let mut k = KnownValues::default();
    k.add_home(&local.home);
    k.add_user(&local.user);
    // `localhost` says nothing and would hit `tauri.localhost` in every stack trace.
    if let Some(c) = local
        .computer
        .as_ref()
        .filter(|c| !c.to_ascii_lowercase().starts_with("localhost"))
    {
        k.add_host(c);
    }
    k.add_user(default_user);
    let add_target = |k: &mut KnownValues, t: &RecentTarget| match t {
        RecentTarget::SshConfig { alias } => k.add_host(alias),
        RecentTarget::Quick { spec } => k.add_host_spec(spec),
        RecentTarget::Bookmark { .. } => {}
    };
    for b in &bookmarks.bookmarks {
        k.add_host(&b.name);
        k.add_host(&b.host);
        if let Some(u) = &b.user {
            k.add_user(u);
        }
        if let AuthConfig::Key { identity_file } = &b.auth {
            k.add_path(identity_file);
        }
        if let Some(d) = &b.start_dir {
            k.add_path(d);
        }
        match &b.proxy_jump {
            Some(JumpRef::SshConfig { alias }) => k.add_host(alias),
            Some(JumpRef::Spec { spec }) => k.add_host_spec(spec),
            Some(JumpRef::Bookmark { .. }) | None => {}
        }
    }
    for h in ssh_hosts {
        k.add_host(&h.alias);
        k.add_host(&h.hostname);
        if let Some(u) = &h.user {
            k.add_user(u);
        }
        if let Some(j) = &h.proxy_jump {
            k.add_host_spec(j);
        }
        for f in &h.identity_files {
            k.add_path(f);
        }
    }
    for r in recents {
        add_target(&mut k, &r.target);
        k.add_host(&r.label);
        k.add_host_spec(&r.user_at_host);
        if let Some(v) = &r.via {
            k.add_host_spec(v);
        }
        if let Some(d) = &r.last_dir {
            k.add_path(d);
        }
    }
    for s in sessions {
        k.add_host(&s.label);
        add_target(&mut k, &s.target);
        for hop in &s.hops {
            k.add_host(&hop.label);
            k.add_host(&hop.host);
            k.add_user(&hop.user);
        }
        for dir in &s.dirs {
            k.add_path(dir);
        }
    }
    k
}

// ---------------------------------------------------------------------------------------------
// escaping and bounds
// ---------------------------------------------------------------------------------------------

/// The characters `displayServerText` escapes (C0, DEL, C1, bidi, invisible; Rust treats every
/// U+FE00–FE0F as invisible).
fn needs_escape(c: char) -> bool {
    c.is_control() || crate::ssh::shellquote::is_deceptive_char(c)
}

/// Escape one line: `\x1b` for C0/DEL/C1, `⟨U+202E⟩` for the others.
pub fn escape_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    for c in line.chars() {
        if needs_escape(c) {
            let code = u32::from(c);
            if code < 0xA0 {
                out.push_str(&format!("\\x{code:02x}"));
            } else {
                out.push_str(&format!("⟨U+{code:04X}⟩"));
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// At most `max` chars, `…` marking a cut.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Bound raw input before redaction: at most [`MAX_RAW_CHARS`], cut at the last whitespace so
/// no value is split (a split value could escape its known-value match); a single token that
/// long is cut where it is.
fn bound_raw(s: &str) -> &str {
    match s.char_indices().nth(MAX_RAW_CHARS) {
        None => s,
        Some((cut, _)) => {
            let head = s.get(..cut).unwrap_or_default();
            head.rfind(char::is_whitespace)
                .and_then(|ws| head.get(..ws))
                .unwrap_or(head)
        }
    }
}

/// Redact, escape and bound a multi-line text: at most `max_lines` lines of at most
/// [`MAX_LINE_CHARS`], at most `max_chars` overall. Newlines (LF, CRLF, CR) separate lines.
fn clean_text(redactor: &Redactor, raw: &str, max_lines: usize, max_chars: usize) -> String {
    let redacted = redactor.redact(bound_raw(raw));
    let normalised = redacted.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<String> = normalised
        .split('\n')
        .take(max_lines)
        .map(|l| truncate_chars(&escape_line(l), MAX_LINE_CHARS))
        .collect();
    let more = normalised.split('\n').count() > max_lines;
    let mut text = truncate_chars(lines.join("\n").trim_end(), max_chars);
    if more && !text.ends_with('…') {
        text.push_str("\n…");
    }
    text
}

/// A code fence longer than any backtick run in `content`.
fn fenced(info: &str, content: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in content.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let fence = "`".repeat((longest + 1).max(3));
    format!("{fence}{info}\n{content}\n{fence}\n")
}

/// `AppError.code`: `[A-Za-z][A-Za-z0-9]{0,63}`, else `unknown`.
pub fn valid_code(code: &str) -> String {
    let ok = code.len() <= 64
        && code.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && code.chars().all(|c| c.is_ascii_alphanumeric());
    if ok {
        code.to_owned()
    } else {
        "unknown".to_owned()
    }
}

/// An action id or command name: `[A-Za-z0-9_.:-]{1,80}`, else none.
pub fn valid_context(context: &str) -> Option<String> {
    let ok = !context.is_empty()
        && context.len() <= 80
        && context
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-'));
    ok.then(|| context.to_owned())
}

/// A language tag: `[A-Za-z0-9-]{1,35}`, else none.
pub fn valid_language(tag: &str) -> Option<String> {
    let ok = !tag.is_empty()
        && tag.len() <= 35
        && tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    ok.then(|| tag.to_owned())
}

/// `YYYY-MM-DD HH:MM:SS UTC` of a Unix ms timestamp.
pub fn format_utc(ms: i64) -> Option<String> {
    let secs = u64::try_from(ms.div_euclid(1000)).ok()?;
    let day = secs / 86_400;
    let rem = secs % 86_400;
    // Beyond year 9999 is not a real error time.
    if day > 2_932_896 {
        return None;
    }
    Some(format!(
        "{} {:02}:{:02}:{:02} UTC",
        crate::logging::civil_date(day),
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    ))
}

// ---------------------------------------------------------------------------------------------
// environment
// ---------------------------------------------------------------------------------------------

/// The bug form's `arch` option for this build.
pub fn arch_option(os: &str, arch: &str) -> &'static str {
    match (os, arch) {
        ("windows" | "linux", "x86_64") => "x64 (Windows or Linux)",
        ("windows", "aarch64") => "ARM64 (Windows)",
        ("macos", "aarch64") => "Apple Silicon (macOS)",
        _ => "Other",
    }
}

/// How this copy runs, as the bug form's `install` option text; `None` when it cannot be told.
pub fn install_option(
    platform: crate::ipc::dto::Platform,
    dev_build: bool,
    appimage_env: bool,
    bundle: Option<tauri::utils::config::BundleType>,
) -> Option<&'static str> {
    use crate::ipc::dto::Platform;
    use tauri::utils::config::BundleType;
    if dev_build {
        return Some("Running in development (npm run tauri dev)");
    }
    match platform {
        Platform::Windows => match bundle {
            Some(BundleType::Nsis) => Some("Windows installer (NSIS .exe)"),
            None => Some("Built from source (npm run tauri build)"),
            _ => None,
        },
        // A Homebrew cask installs the same .app as the DMG.
        Platform::Macos => None,
        Platform::Linux => match bundle {
            Some(BundleType::Deb) => Some("Linux deb package"),
            Some(BundleType::Rpm) => Some("Linux rpm package"),
            Some(BundleType::AppImage) => Some("Linux AppImage"),
            None if appimage_env => Some("Linux AppImage"),
            None => Some("Built from source (npm run tauri build)"),
            _ => None,
        },
    }
}

/// The options of the bug form's `install` dropdown (kept in sync by a test).
pub const INSTALL_OPTIONS: &[&str] = &[
    "Windows installer (NSIS .exe)",
    "macOS DMG",
    "macOS Homebrew cask",
    "Linux AppImage",
    "Linux deb package",
    "Linux rpm package",
    "Built from source (npm run tauri build)",
    "Running in development (npm run tauri dev)",
    "Other",
];

/// The options of the bug form's `arch` dropdown.
pub const ARCH_OPTIONS: &[&str] = &[
    "x64 (Windows or Linux)",
    "ARM64 (Windows)",
    "Apple Silicon (macOS)",
    "Other",
];

/// The environment of this process.
pub fn current_environment(app_version: &str, ui_language: Option<&str>) -> ReportEnvironment {
    let dev_build = cfg!(dev) || cfg!(debug_assertions);
    ReportEnvironment {
        app_version: crate::platform::os_info::clean(app_version, 64),
        os: crate::platform::os_info::os_description(),
        arch: std::env::consts::ARCH.to_owned(),
        install: install_option(
            crate::ipc::dto::Platform::current(),
            dev_build,
            cfg!(target_os = "linux")
                && std::env::var_os("APPIMAGE").is_some_and(|v| !v.is_empty()),
            tauri::utils::platform::bundle_type(),
        )
        .map(str::to_owned),
        ui_language: ui_language.and_then(valid_language),
    }
}

// ---------------------------------------------------------------------------------------------
// the report
// ---------------------------------------------------------------------------------------------

/// What the report is built from (raw; everything is redacted here).
#[derive(Debug, Default, Clone)]
pub struct ReportInput {
    pub code: Option<String>,
    pub params: Option<serde_json::Value>,
    pub detail: Option<String>,
    pub message: Option<String>,
    pub context: Option<String>,
    pub stack: Option<String>,
    pub occurred_at_ms: Option<i64>,
    /// The log tail (`None` = not requested).
    pub log: Option<Vec<String>>,
}

/// Build the report (pure: the caller collects the environment, known values and log tail).
pub fn build(input: &ReportInput, env: &ReportEnvironment, known: &KnownValues) -> ErrorReport {
    let redactor = Redactor::new(known);
    let code = input.code.as_deref().map(valid_code);
    let context = input
        .context
        .as_deref()
        .and_then(valid_context)
        .map(|c| clean_text(&redactor, &c, 1, 80))
        .filter(|c| !c.is_empty());
    let message = input
        .message
        .as_deref()
        .map(|m| clean_text(&redactor, m, 50, MAX_MESSAGE_CHARS))
        .filter(|m| !m.is_empty());
    let detail = input
        .detail
        .as_deref()
        .map(|d| clean_text(&redactor, d, 100, MAX_DETAIL_CHARS))
        .filter(|d| !d.is_empty());
    let params = input
        .params
        .as_ref()
        .filter(|p| !p.is_null())
        .map(|p| clean_text(&redactor, &p.to_string(), 1, MAX_PARAMS_CHARS));
    let stack = input
        .stack
        .as_deref()
        .map(|s| clean_text(&redactor, s, MAX_STACK_LINES, MAX_STACK_CHARS))
        .filter(|s| !s.is_empty());
    let time = input.occurred_at_ms.and_then(format_utc);
    let log_lines: Option<Vec<String>> = input.log.as_ref().map(|lines| {
        lines
            .iter()
            .map(|l| truncate_chars(&escape_line(&redactor.redact(bound_raw(l))), MAX_LINE_CHARS))
            .collect()
    });

    let mut md = String::new();
    if code.is_some() || message.is_some() || stack.is_some() {
        md.push_str("## Error\n\n");
        if let Some(m) = &message {
            md.push_str(&fenced("text", m));
            md.push('\n');
        }
        if let Some(c) = &code {
            md.push_str(&format!("- Code: `{c}`\n"));
        }
        if let Some(c) = &context {
            md.push_str(&format!("- Where: `{c}`\n"));
        }
        if let Some(t) = &time {
            md.push_str(&format!("- Time: {t}\n"));
        }
        if let Some(p) = &params {
            md.push_str("\nParameters:\n\n");
            md.push_str(&fenced("json", p));
        }
        if let Some(d) = &detail {
            md.push_str("\nDetails:\n\n");
            md.push_str(&fenced("text", d));
        }
        if let Some(s) = &stack {
            md.push_str("\nStack:\n\n");
            md.push_str(&fenced("text", s));
        }
        md.push('\n');
    }
    md.push_str("## Environment\n\n");
    md.push_str(&format!("- Shellfin: {}\n", env.app_version));
    md.push_str(&format!("- OS: {}\n", env.os));
    md.push_str(&format!("- Architecture: {}\n", env.arch));
    if let Some(i) = &env.install {
        md.push_str(&format!("- Installed as: {i}\n"));
    }
    if let Some(l) = &env.ui_language {
        md.push_str(&format!("- UI language: {l}\n"));
    }
    let mut log_count = 0;
    if let Some(lines) = log_lines.as_ref().filter(|l| !l.is_empty()) {
        log_count = lines.len();
        md.push_str(&format!(
            "\n## Log (last {} lines, redacted)\n\n",
            lines.len()
        ));
        md.push_str(&fenced("text", &lines.join("\n")));
    }

    let summary = code.as_ref().map(|c| UrlSummary {
        code: c.clone(),
        context: context.clone(),
        time: time.clone(),
        message: message.clone().unwrap_or_default(),
    });
    let (issue_url, in_url) = issue_url(
        env,
        summary.as_ref(),
        log_lines.as_deref().unwrap_or_default(),
    );
    ErrorReport {
        markdown: md.trim_end().to_owned() + "\n",
        issue_url,
        environment: env.clone(),
        log_included: log_count > 0,
        log_lines: u32::try_from(log_count).unwrap_or(u32::MAX),
        log_lines_in_url: u32::try_from(in_url).unwrap_or(u32::MAX),
    }
}

/// The error part of the URL (already redacted and escaped).
#[derive(Debug, Clone)]
pub struct UrlSummary {
    pub code: String,
    pub context: Option<String>,
    pub time: Option<String>,
    pub message: String,
}

fn encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn issue_base() -> String {
    format!("{}/issues/new", env!("CARGO_PKG_REPOSITORY"))
}

fn query_url(pairs: &[(&str, String)]) -> String {
    let query: Vec<String> = pairs
        .iter()
        .map(|(k, v)| format!("{k}={}", encode(v)))
        .collect();
    format!("{}?{}", issue_base(), query.join("&"))
}

fn actual_text(summary: &UrlSummary, message_chars: usize) -> String {
    let mut text = format!("Error `{}`", summary.code);
    if let Some(c) = &summary.context {
        text.push_str(&format!(" in `{c}`"));
    }
    if let Some(t) = &summary.time {
        text.push_str(&format!(" at {t}"));
    }
    let message = truncate_chars(&summary.message, message_chars);
    if message_chars > 0 && !message.is_empty() {
        text.push_str(":\n\n");
        text.push_str(&fenced("text", &message));
    }
    text
}

/// The pre-filled bug form: `title`, `version`, `os`, `arch`, `install`, `actual` and as many of
/// the newest log lines as fit in `logs`, the whole URL at most [`MAX_URL_CHARS`] after encoding.
/// Returns the URL and how many log lines it carries.
pub fn issue_url(
    env: &ReportEnvironment,
    summary: Option<&UrlSummary>,
    log: &[String],
) -> (String, usize) {
    let mut pairs: Vec<(&str, String)> = vec![("template", ISSUE_TEMPLATE.to_owned())];
    if let Some(s) = summary {
        let mut title = format!("[Bug]: Error {}", s.code);
        if let Some(c) = &s.context {
            title.push_str(&format!(" in {c}"));
        }
        pairs.push(("title", truncate_chars(&title, 120)));
    }
    pairs.push(("version", truncate_chars(&env.app_version, 64)));
    pairs.push(("os", truncate_chars(&env.os, 100)));
    pairs.push((
        "arch",
        arch_option(std::env::consts::OS, &env.arch).to_owned(),
    ));
    if let Some(i) = env
        .install
        .as_deref()
        .filter(|i| INSTALL_OPTIONS.contains(i))
    {
        pairs.push(("install", i.to_owned()));
    }
    let fits = |pairs: &[(&str, String)]| query_url(pairs).len() <= MAX_URL_CHARS;
    if let Some(s) = summary {
        // The longest message prefix that fits (the full message is in the copied report).
        let mut chars = MAX_URL_MESSAGE_CHARS;
        loop {
            let mut candidate = pairs.clone();
            candidate.push(("actual", actual_text(s, chars)));
            if fits(&candidate) {
                pairs = candidate;
                break;
            }
            if chars == 0 {
                break; // even the bare summary does not fit: leave `actual` empty
            }
            chars /= 2;
        }
    }
    // The newest lines that fit; each line adds its encoding plus an encoded newline.
    let base = query_url(&pairs).len() + "&logs=".len();
    let mut used = base;
    let mut count = 0;
    for line in log.iter().rev() {
        let add = encode(line).len() + if count > 0 { 3 } else { 0 };
        if used + add > MAX_URL_CHARS {
            break;
        }
        used += add;
        count += 1;
    }
    if count > 0 {
        let tail = log.get(log.len() - count..).unwrap_or_default().join("\n");
        pairs.push(("logs", tail));
    }
    let url = query_url(&pairs);
    // The frontend opens it with `app_open_external`: it must pass the same checks.
    match normalize_external_url(&url) {
        Ok(u) if u.as_str().len() <= MAX_URL_CHARS => (u.to_string(), count),
        _ => (query_url(&[("template", ISSUE_TEMPLATE.to_owned())]), 0),
    }
}

#[cfg(test)]
mod tests;
