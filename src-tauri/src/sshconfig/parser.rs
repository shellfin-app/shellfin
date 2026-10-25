//! File → items, `Include` expansion and load-time validation (§7.4).
//!
//! Includes are expanded at load time (they do not depend on the target host), so the parsed
//! tree is host-independent; the per-host evaluation of `Host`/`Match` happens in `resolve`.
//! Values of interpreted keywords are syntax-checked here; invalid lines become `badLine`
//! warnings and are dropped, so resolution never sees them.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use crate::ipc::dto::{ConfigWarning, ConfigWarningCode, SourceLocation};

use super::lexer::{Lexed, LexedLine, lex_line};
use super::{
    ConfigEnv, ConfigScope, ConfigSource, MAX_CONFIG_FILES, MAX_INCLUDE_DEPTH, ParsedFile,
    ReadOutcome, location,
};

/// Interpreted keywords (§7.4). Everything else is parsed and ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Key {
    HostName,
    User,
    Port,
    IdentityFile,
    IdentitiesOnly,
    IdentityAgent,
    ProxyJump,
    ProxyCommand,
    UserKnownHostsFile,
    GlobalKnownHostsFile,
    StrictHostKeyChecking,
    HostKeyAlias,
    HostKeyAlgorithms,
    HashKnownHosts,
    ServerAliveInterval,
    ServerAliveCountMax,
    ConnectTimeout,
    PreferredAuthentications,
    PubkeyAuthentication,
    PasswordAuthentication,
    /// Also `ChallengeResponseAuthentication` (deprecated alias in OpenSSH, same option).
    KbdInteractiveAuthentication,
    AddKeysToAgent,
    CertificateFile,
    /// Read only to know that `%h` and `%C` cannot be computed like OpenSSH (it would use the
    /// canonicalised name); Shellfin does not canonicalise.
    CanonicalizeHostname,
}

impl Key {
    fn from_keyword(kw: &str) -> Option<Self> {
        Some(match kw {
            "hostname" => Self::HostName,
            "user" => Self::User,
            "port" => Self::Port,
            "identityfile" => Self::IdentityFile,
            "identitiesonly" => Self::IdentitiesOnly,
            "identityagent" => Self::IdentityAgent,
            "proxyjump" => Self::ProxyJump,
            "proxycommand" => Self::ProxyCommand,
            "userknownhostsfile" => Self::UserKnownHostsFile,
            "globalknownhostsfile" => Self::GlobalKnownHostsFile,
            "stricthostkeychecking" => Self::StrictHostKeyChecking,
            "hostkeyalias" => Self::HostKeyAlias,
            "hostkeyalgorithms" => Self::HostKeyAlgorithms,
            "hashknownhosts" => Self::HashKnownHosts,
            "serveraliveinterval" => Self::ServerAliveInterval,
            "serveralivecountmax" => Self::ServerAliveCountMax,
            "connecttimeout" => Self::ConnectTimeout,
            "preferredauthentications" => Self::PreferredAuthentications,
            "pubkeyauthentication" => Self::PubkeyAuthentication,
            "passwordauthentication" => Self::PasswordAuthentication,
            "kbdinteractiveauthentication" | "challengeresponseauthentication" => {
                Self::KbdInteractiveAuthentication
            }
            "addkeystoagent" => Self::AddKeysToAgent,
            "certificatefile" => Self::CertificateFile,
            "canonicalizehostname" => Self::CanonicalizeHostname,
            _ => return None,
        })
    }
}

/// An interpreted option line.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OptLine {
    pub key: Key,
    pub args: Vec<String>,
    pub loc: SourceLocation,
    pub scope: ConfigScope,
}

impl OptLine {
    pub fn first(&self) -> &str {
        self.args.first().map(String::as_str).unwrap_or_default()
    }
}

/// `Match` criterion kinds we evaluate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CritKind {
    All,
    /// HostName so far (or the alias).
    Host,
    OriginalHost,
    /// User so far (or the local user).
    User,
    LocalUser,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Criterion {
    pub kind: CritKind,
    pub negate: bool,
    /// Comma-separated pattern list (empty for `all`).
    pub list: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Item {
    Host {
        patterns: Vec<String>,
        loc: SourceLocation,
    },
    /// `unsupported` non-empty ⇒ the block never matches (§7.4).
    Match {
        criteria: Vec<Criterion>,
        unsupported: Vec<String>,
        loc: SourceLocation,
    },
    Opt(OptLine),
    /// The files one `Include` line expanded to, in glob order.
    Include {
        files: Vec<ParsedFile>,
    },
}

pub(crate) fn warning(loc: SourceLocation, kind: ConfigWarningCode) -> ConfigWarning {
    ConfigWarning {
        location: loc,
        kind,
    }
}

/// At most this many `badLine` warnings per file (a 4 MiB file of junk lines would
/// otherwise produce 10^5+ warnings on every snapshot).
pub(crate) const MAX_BAD_LINES_PER_FILE: usize = 50;

/// Glob matches looked at per `Include` argument (opened, skipped as untrusted or as a cycle).
pub(crate) const MAX_INCLUDE_MATCHES: usize = 4 * MAX_CONFIG_FILES;

/// What a `badLine` warning may show of its line: the keyword if it is one we know,
/// else `?`. The raw text is never echoed: an `Include ~/.ssh/*` can match a private key, whose
/// base64 lines would otherwise travel to the webview.
fn bad_line_text(raw: &str) -> String {
    let s = raw.trim_start_matches([' ', '\t']);
    let end = s.find([' ', '\t', '=']).unwrap_or(s.len());
    let kw = s.get(..end).unwrap_or_default().to_ascii_lowercase();
    if matches!(kw.as_str(), "host" | "match" | "include") || Key::from_keyword(&kw).is_some() {
        kw
    } else {
        "?".to_owned()
    }
}

/// A file that must not be parsed as a config at all: private key material or binary
/// data (NUL bytes).
/// The (unparsable) line starts with the `Host` or `Match` keyword.
fn opens_block(raw: &str) -> bool {
    let s = raw.trim_start_matches([' ', '\t']);
    let end = s.find([' ', '\t', '=', '\r']).unwrap_or(s.len());
    s.get(..end)
        .is_some_and(|kw| kw.eq_ignore_ascii_case("host") || kw.eq_ignore_ascii_case("match"))
}

fn looks_like_secret_or_binary(bytes: &[u8]) -> bool {
    const MARKER: &[u8] = b"PRIVATE KEY-----";
    bytes.contains(&0) || bytes.windows(MARKER.len()).any(|w| w == MARKER)
}

pub(crate) fn parse_bool(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "yes" | "true" => Some(true),
        "no" | "false" => Some(false),
        _ => None,
    }
}

/// OpenSSH `convtime`: `30`, `30s`, `1m30s`, `2h`, `1d`, `1w` (seconds). `None` if invalid.
pub(crate) fn parse_time(v: &str) -> Option<u32> {
    if v.is_empty() {
        return None;
    }
    let mut total: u64 = 0;
    let mut num: Option<u64> = None;
    for c in v.chars() {
        if let Some(d) = c.to_digit(10) {
            num = Some(
                num.unwrap_or(0)
                    .checked_mul(10)?
                    .checked_add(u64::from(d))?,
            );
            continue;
        }
        let mult = match c.to_ascii_lowercase() {
            's' => 1,
            'm' => 60,
            'h' => 3600,
            'd' => 86_400,
            'w' => 604_800,
            _ => return None,
        };
        total = total.checked_add(num.take()?.checked_mul(mult)?)?;
    }
    if let Some(n) = num {
        total = total.checked_add(n)?;
    }
    u32::try_from(total).ok()
}

/// Load-time syntax check of an interpreted option's value.
fn value_ok(key: Key, args: &[String]) -> bool {
    let first = args.first().map(String::as_str).unwrap_or_default();
    match key {
        Key::Port => {
            first.parse::<u16>().is_ok_and(|p| p != 0) && first.bytes().all(|b| b.is_ascii_digit())
        }
        Key::IdentitiesOnly
        | Key::HashKnownHosts
        | Key::PasswordAuthentication
        | Key::KbdInteractiveAuthentication => parse_bool(first).is_some(),
        Key::PubkeyAuthentication => {
            parse_bool(first).is_some()
                || matches!(
                    first.to_ascii_lowercase().as_str(),
                    "unbound" | "host-bound"
                )
        }
        Key::StrictHostKeyChecking => matches!(
            first.to_ascii_lowercase().as_str(),
            "yes" | "true" | "ask" | "accept-new" | "no" | "off" | "false"
        ),
        Key::CanonicalizeHostname => matches!(
            first.to_ascii_lowercase().as_str(),
            "yes" | "true" | "no" | "false" | "always"
        ),
        Key::ServerAliveInterval | Key::ConnectTimeout => parse_time(first).is_some(),
        Key::ServerAliveCountMax => {
            first.parse::<u32>().is_ok() && first.bytes().all(|b| b.is_ascii_digit())
        }
        _ => true,
    }
}

/// Directory part of an Include pattern: everything before the first component with glob
/// metacharacters (or the parent directory if there are none).
fn pattern_dir(pattern: &Path) -> Option<PathBuf> {
    let mut dir = PathBuf::new();
    let mut had_glob = false;
    for comp in pattern.components() {
        let s = comp.as_os_str().to_string_lossy();
        if s.contains(['*', '?', '[']) {
            had_glob = true;
            break;
        }
        dir.push(comp.as_os_str());
    }
    if had_glob {
        Some(dir)
    } else {
        pattern.parent().map(Path::to_path_buf)
    }
}

/// Lexical normalisation for cycle detection (no filesystem access).
fn normalise(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

pub(crate) struct Loader<'a> {
    source: &'a dyn ConfigSource,
    env: &'a ConfigEnv,
    warnings: Vec<ConfigWarning>,
    files_read: Vec<PathBuf>,
    include_dirs: Vec<PathBuf>,
    include_dirs_seen: HashSet<PathBuf>,
    /// Files currently being read (cycle detection), normalised.
    stack: Vec<PathBuf>,
    /// Files opened so far (main + includes), for the 256-file cap.
    opened: usize,
    /// `badLine` warnings of the file being read (saved/restored around includes).
    bad_lines: usize,
    /// `badLine` warnings not reported because their file hit `MAX_BAD_LINES_PER_FILE`
    /// (all files together, saturating).
    dropped_warnings: u32,
    /// Files loaded so far, as spelled (the same key warnings are deduplicated by). A file included
    /// more than once reports the same warnings each time, so its dropped lines are counted on the
    /// first load only.
    loaded: HashSet<PathBuf>,
    /// Whether the file being read counts its dropped lines (saved/restored around includes).
    count_dropped: bool,
}

/// What [`Loader::finish`] hands back to `load`.
pub(crate) struct LoadResult {
    pub warnings: Vec<ConfigWarning>,
    pub dropped_warnings: u32,
    pub files_read: Vec<PathBuf>,
    pub include_dirs: Vec<PathBuf>,
}

impl<'a> Loader<'a> {
    pub fn new(source: &'a dyn ConfigSource, env: &'a ConfigEnv) -> Self {
        Self {
            source,
            env,
            warnings: Vec::new(),
            files_read: Vec::new(),
            include_dirs: Vec::new(),
            include_dirs_seen: HashSet::new(),
            stack: Vec::new(),
            opened: 0,
            bad_lines: 0,
            dropped_warnings: 0,
            loaded: HashSet::new(),
            count_dropped: true,
        }
    }

    /// Report a line we could not use (keyword only, capped per file; lines beyond the cap
    /// are only counted).
    fn bad_line(&mut self, loc: SourceLocation, raw: &str) {
        if self.bad_lines >= MAX_BAD_LINES_PER_FILE {
            if self.count_dropped {
                self.dropped_warnings = self.dropped_warnings.saturating_add(1);
            }
            return;
        }
        self.bad_lines += 1;
        let text = bad_line_text(raw);
        self.warnings
            .push(warning(loc, ConfigWarningCode::BadLine { text }));
    }

    pub fn finish(self) -> LoadResult {
        LoadResult {
            warnings: self.warnings,
            dropped_warnings: self.dropped_warnings,
            files_read: self.files_read,
            include_dirs: self.include_dirs,
        }
    }

    pub fn load_main(&mut self, path: &Path, scope: ConfigScope) -> Option<ParsedFile> {
        self.load_file(path, scope, 0)
    }

    fn file_warning(&mut self, path: &Path, kind: ConfigWarningCode) {
        // File-level problems are reported at line 0 of the file itself.
        self.warnings.push(warning(location(path, 0), kind));
    }

    fn load_file(&mut self, path: &Path, scope: ConfigScope, depth: usize) -> Option<ParsedFile> {
        self.opened += 1;
        let bytes = match self.source.read(path, scope) {
            ReadOutcome::Ok(b) => b,
            ReadOutcome::NotFound => return None,
            ReadOutcome::TooLarge => {
                let file = path.display().to_string();
                self.file_warning(path, ConfigWarningCode::FileTooLarge { file });
                return None;
            }
            ReadOutcome::Untrusted | ReadOutcome::Error(_) => {
                let file = path.display().to_string();
                self.file_warning(path, ConfigWarningCode::UntrustedFile { file });
                return None;
            }
        };
        self.files_read.push(path.to_path_buf());
        if looks_like_secret_or_binary(&bytes) {
            // Typically `Include ~/.ssh/*` matching a key: skipped whole, one warning, nothing
            // of its content reported.
            self.file_warning(
                path,
                ConfigWarningCode::BadLine {
                    text: "?".to_owned(),
                },
            );
            return None;
        }
        let norm = normalise(path);
        let first_load = self.loaded.insert(path.to_path_buf());
        let outer_count_dropped = std::mem::replace(&mut self.count_dropped, first_load);
        self.stack.push(norm);
        let text = String::from_utf8_lossy(&bytes);
        // A UTF-8 BOM (PowerShell 5.1, old Notepad) would turn the first keyword into
        // an unknown one and make the first block's options global.
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        let outer_bad_lines = std::mem::replace(&mut self.bad_lines, 0);
        let mut items = Vec::new();
        for (idx, raw) in text.split('\n').enumerate() {
            let loc = location(path, idx + 1);
            match lex_line(raw) {
                Lexed::Empty => {}
                Lexed::Bad => {
                    // A broken `Host`/`Match` line still ends the previous block; its
                    // options go to a block that never matches (OpenSSH rejects the file).
                    if opens_block(raw) {
                        items.push(Item::Host {
                            patterns: Vec::new(),
                            loc: loc.clone(),
                        });
                    }
                    self.bad_line(loc, raw);
                }
                Lexed::Line(line) => {
                    if let Some(item) = self.parse_line(line, raw, loc, scope, depth) {
                        items.push(item);
                    }
                }
            }
        }
        self.bad_lines = outer_bad_lines;
        self.count_dropped = outer_count_dropped;
        self.stack.pop();
        Some(ParsedFile {
            path: path.to_path_buf(),
            scope,
            items,
        })
    }

    fn parse_line(
        &mut self,
        line: LexedLine,
        raw: &str,
        loc: SourceLocation,
        scope: ConfigScope,
        depth: usize,
    ) -> Option<Item> {
        match line.keyword.as_str() {
            "host" => Some(Item::Host {
                patterns: line.args,
                loc,
            }),
            "match" => Some(self.parse_match(line.args, raw, loc)),
            "include" => Some(Item::Include {
                files: self.include(&line.args, &loc, scope, depth),
            }),
            kw => {
                let key = Key::from_keyword(kw)?;
                if value_ok(key, &line.args) {
                    Some(Item::Opt(OptLine {
                        key,
                        args: line.args,
                        loc,
                        scope,
                    }))
                } else {
                    self.bad_line(loc, raw);
                    None
                }
            }
        }
    }

    fn parse_match(&mut self, args: Vec<String>, raw: &str, loc: SourceLocation) -> Item {
        let mut criteria = Vec::new();
        let mut unsupported = Vec::new();
        let mut it = args.into_iter();
        while let Some(tok) = it.next() {
            let lower = tok.to_ascii_lowercase();
            let (negate, name) = match lower.strip_prefix('!') {
                Some(n) => (true, n.to_owned()),
                None => (false, lower.clone()),
            };
            let kind = match name.as_str() {
                "all" => Some(CritKind::All),
                "host" => Some(CritKind::Host),
                "originalhost" => Some(CritKind::OriginalHost),
                "user" => Some(CritKind::User),
                "localuser" => Some(CritKind::LocalUser),
                _ => None,
            };
            match kind {
                Some(CritKind::All) => criteria.push(Criterion {
                    kind: CritKind::All,
                    negate,
                    list: String::new(),
                }),
                Some(kind) => match it.next() {
                    Some(list) => criteria.push(Criterion { kind, negate, list }),
                    None => {
                        self.bad_line(loc.clone(), raw);
                        unsupported.push(name);
                    }
                },
                None => {
                    // exec/localnetwork/tagged/version/sessiontype/command take an argument;
                    // canonical/final do not. Nothing is ever executed.
                    if matches!(
                        name.as_str(),
                        "exec" | "localnetwork" | "tagged" | "version" | "sessiontype" | "command"
                    ) {
                        it.next();
                    }
                    unsupported.push(name);
                }
            }
        }
        if !unsupported.is_empty() {
            let criteria_text = unsupported.join(",");
            self.warnings.push(warning(
                loc.clone(),
                ConfigWarningCode::UnsupportedMatchCriteria {
                    criteria: criteria_text,
                },
            ));
        }
        Item::Match {
            criteria,
            unsupported,
            loc,
        }
    }

    /// Expand one `Include` line into the parsed files it names.
    fn include(
        &mut self,
        args: &[String],
        loc: &SourceLocation,
        scope: ConfigScope,
        depth: usize,
    ) -> Vec<ParsedFile> {
        let mut files = Vec::new();
        for arg in args {
            let pattern_str = arg.clone();
            if arg.contains("**") {
                self.warnings.push(warning(
                    loc.clone(),
                    ConfigWarningCode::IncludeRecursiveGlob {
                        pattern: pattern_str,
                    },
                ));
                continue;
            }
            let (glob_pattern, plain) = self.anchor(arg, scope);
            if !self.source.path_allowed(&plain) {
                let file = plain.display().to_string();
                self.warnings.push(warning(
                    loc.clone(),
                    ConfigWarningCode::UntrustedFile { file },
                ));
                continue;
            }
            if let Some(dir) = pattern_dir(&plain)
                && self.include_dirs_seen.insert(dir.clone())
            {
                self.include_dirs.push(dir);
            }
            if depth + 1 > MAX_INCLUDE_DEPTH {
                self.warnings.push(warning(
                    loc.clone(),
                    ConfigWarningCode::IncludeDepthExceeded {
                        pattern: pattern_str,
                    },
                ));
                continue;
            }
            let options = glob::MatchOptions {
                case_sensitive: true,
                require_literal_separator: true,
                require_literal_leading_dot: true,
            };
            // Matches are taken lazily (not collected in full first), and at most
            // `MAX_INCLUDE_MATCHES` are looked at per argument, so a huge directory or a
            // multi-level wildcard cannot make the loader build or warn about millions of paths.
            let matches = match glob::glob_with(&glob_pattern, options) {
                Ok(paths) => paths.filter_map(Result::ok),
                Err(_) => {
                    self.bad_line(loc.clone(), "include");
                    continue;
                }
            };
            for (seen, m) in matches.enumerate() {
                if seen >= MAX_INCLUDE_MATCHES {
                    self.warnings.push(warning(
                        loc.clone(),
                        ConfigWarningCode::IncludeTooManyFiles {
                            pattern: pattern_str.clone(),
                        },
                    ));
                    break;
                }
                if !self.source.path_allowed(&m) {
                    let file = m.display().to_string();
                    self.warnings.push(warning(
                        loc.clone(),
                        ConfigWarningCode::UntrustedFile { file },
                    ));
                    continue;
                }
                if self.stack.contains(&normalise(&m)) {
                    let file = m.display().to_string();
                    self.warnings.push(warning(
                        loc.clone(),
                        ConfigWarningCode::IncludeCycle { file },
                    ));
                    continue;
                }
                if self.opened >= MAX_CONFIG_FILES {
                    self.warnings.push(warning(
                        loc.clone(),
                        ConfigWarningCode::IncludeTooManyFiles {
                            pattern: pattern_str.clone(),
                        },
                    ));
                    break;
                }
                if let Some(f) = self.load_file(&m, scope, depth + 1) {
                    files.push(f);
                }
            }
        }
        files
    }

    /// Anchor an Include argument: `~` → home; relative → `~/.ssh` (user) or the system ssh
    /// directory (system). Returns the glob pattern (with the fixed base escaped) and the plain
    /// path used for the allowlist and the watched directory.
    fn anchor(&self, arg: &str, scope: ConfigScope) -> (String, PathBuf) {
        let home = &self.env.home;
        let base_and_rest: Option<(&Path, &str)> = if arg == "~" {
            Some((home.as_path(), ""))
        } else if let Some(rest) = arg.strip_prefix("~/").or_else(|| arg.strip_prefix("~\\")) {
            Some((home.as_path(), rest))
        } else if Path::new(arg).is_absolute() || Path::new(arg).has_root() {
            None
        } else {
            Some(match scope {
                ConfigScope::User => (self.env.user_ssh_dir.as_path(), arg),
                ConfigScope::System => (self.env.system_ssh_dir.as_path(), arg),
            })
        };
        match base_and_rest {
            Some((base, rest)) => {
                let plain = if rest.is_empty() {
                    base.to_path_buf()
                } else {
                    base.join(rest)
                };
                let escaped_base = glob::Pattern::escape(&base.display().to_string());
                let sep = std::path::MAIN_SEPARATOR;
                let pattern = if rest.is_empty() {
                    escaped_base
                } else {
                    format!("{escaped_base}{sep}{rest}")
                };
                (pattern, plain)
            }
            None => (arg.to_owned(), PathBuf::from(arg)),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use crate::sshconfig::testutil::{Fixture, PickySource};

    fn codes(w: &[ConfigWarning]) -> Vec<ConfigWarningCode> {
        w.iter().map(|w| w.kind.clone()).collect()
    }

    #[test]
    fn convtime() {
        assert_eq!(parse_time("30"), Some(30));
        assert_eq!(parse_time("30s"), Some(30));
        assert_eq!(parse_time("1m30s"), Some(90));
        assert_eq!(parse_time("2h"), Some(7200));
        assert_eq!(parse_time("1W"), Some(604_800));
        assert_eq!(parse_time(""), None);
        assert_eq!(parse_time("x"), None);
        assert_eq!(parse_time("m"), None);
        assert_eq!(parse_time("99999999999"), None);
    }

    #[test]
    fn bad_lines_are_reported_with_location() {
        let f = Fixture::new();
        let cfg = f.user_config(
            "Host a\n  Port notanumber\n  HostName \"unterminated\n  Port 70000\n  Port 22\n",
        );
        let p = f.load();
        let w = p.load_warnings();
        assert_eq!(w.len(), 3, "{w:?}");
        assert_eq!(w[0].location.line, 2);
        assert_eq!(w[0].location.file, cfg.display().to_string());
        assert!(
            matches!(&w[0].kind, ConfigWarningCode::BadLine { text } if text == "port"),
            "only the keyword is reported"
        );
        assert_eq!(w[1].location.line, 3);
        assert_eq!(w[2].location.line, 4);
        assert_eq!(p.query("a", &f.env).unwrap().port, Some(22));
    }

    /// (hostile input): `Include ~/.ssh/*` matching a private key never ships key text to
    /// the UI: the key file is skipped with one content-free warning, and bad lines elsewhere
    /// show only a known keyword.
    #[test]
    fn included_private_key_text_never_reaches_warnings() {
        let f = Fixture::new();
        let b64 = "b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW";
        f.user_file(
            "id_test",
            &format!(
                "-----BEGIN OPENSSH PRIVATE KEY-----\n{b64}\n{b64}\n-----END OPENSSH PRIVATE KEY-----\n"
            ),
        );
        f.user_file("junk", &format!("{b64}\nPort {b64}\n"));
        f.user_file("bin", "Host x\0\n");
        f.user_config("Include *\nHost a\n  Port 22\n");
        let p = f.load();
        let w = p.load_warnings();
        let shown = format!("{w:?}");
        assert!(!shown.contains("b3BlbnNzaC"), "{shown}");
        assert!(!shown.contains("PRIVATE"), "{shown}");
        let texts: Vec<&str> = w
            .iter()
            .filter_map(|w| match &w.kind {
                ConfigWarningCode::BadLine { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(texts.iter().all(|t| *t == "?" || *t == "port"), "{texts:?}");
        assert!(texts.contains(&"port"));
        // The key and the binary file are reported once each, at line 0.
        let whole_file: Vec<_> = w.iter().filter(|w| w.location.line == 0).collect();
        assert_eq!(whole_file.len(), 2, "{w:?}");
        assert_eq!(p.query("a", &f.env).unwrap().port, Some(22));
    }

    /// A file of junk lines produces at most `MAX_BAD_LINES_PER_FILE` warnings; the cap
    /// is per file (an included file gets its own), and the outer count resumes after it.
    #[test]
    fn bad_line_warnings_are_capped_per_file() {
        let f = Fixture::new();
        f.user_file("inc", &"junk\n".repeat(10_000));
        let mut main = "junk\n".repeat(10);
        main.push_str("Include inc\n");
        main.push_str(&"junk\n".repeat(10_000));
        f.user_config(&main);
        let p = f.load();
        assert_eq!(p.load_warnings().len(), 2 * MAX_BAD_LINES_PER_FILE);
        // Everything beyond the cap is counted: (10 + 10 000 − 50) in the main file plus
        // (10 000 − 50) in the included one.
        assert_eq!(p.dropped_warnings, (10_010 - 50) + (10_000 - 50));
    }

    /// A file included twice reports the same (deduplicated) warnings both times, so its
    /// dropped lines count once; the including file still counts its own afterwards.
    #[test]
    fn file_included_twice_counts_dropped_once() {
        let f = Fixture::new();
        f.user_file("common.conf", &"junk\n".repeat(60));
        let mut main = "Host a\nInclude common.conf\nHost b\nInclude common.conf\n".to_owned();
        main.push_str(&"junk\n".repeat(MAX_BAD_LINES_PER_FILE + 3));
        f.user_config(&main);
        let p = f.load();
        assert_eq!(p.dropped_warnings, 10 + 3);
        let s = p.snapshot(0);
        assert_eq!(s.warnings.len(), 2 * MAX_BAD_LINES_PER_FILE, "deduplicated");
        assert_eq!(s.dropped_warnings, 13);
    }

    /// No junk, no count; exactly the cap is not "dropped" either.
    #[test]
    fn dropped_warnings_start_after_the_cap() {
        let f = Fixture::new();
        f.user_config("Host a\n  Port 22\n");
        assert_eq!(f.load().dropped_warnings, 0);
        f.user_config(&"junk\n".repeat(MAX_BAD_LINES_PER_FILE));
        let p = f.load();
        assert_eq!(p.load_warnings().len(), MAX_BAD_LINES_PER_FILE);
        assert_eq!(p.dropped_warnings, 0);
        f.user_config(&"junk\n".repeat(MAX_BAD_LINES_PER_FILE + 1));
        assert_eq!(f.load().dropped_warnings, 1);
    }

    /// A reload whose only difference is in the lines beyond the cap still counts as a
    /// change (the snapshot's `droppedWarnings` differs), and an identical one does not.
    #[test]
    fn change_beyond_the_cap_is_a_change() {
        use crate::sshconfig::SshConfigCache;
        let f = Fixture::new();
        let cache = SshConfigCache::default();
        f.user_config(&"junk\n".repeat(60));
        assert!(cache.replace(f.load()));
        assert!(!cache.replace(f.load()), "same files, no change");
        f.user_config(&"junk\n".repeat(70));
        let p = f.load();
        assert_eq!(p.load_warnings(), cache.get().unwrap().load_warnings());
        assert_eq!(p.dropped_warnings, 20);
        assert!(cache.replace(p), "only the dropped count changed");
    }

    /// A source that opens nothing (`readSshConfig` off) drops nothing.
    #[test]
    fn disabled_source_drops_nothing() {
        let f = Fixture::new();
        f.user_config(&"junk\n".repeat(500));
        let p = f.load_with(&crate::sshconfig::service::DisabledSource);
        assert!(p.load_warnings().is_empty());
        assert_eq!(p.dropped_warnings, 0);
    }

    /// A UTF-8 BOM does not hide the first keyword (the first block stays a block).
    #[test]
    fn utf8_bom_is_ignored() {
        let f = Fixture::new();
        f.user_config("\u{feff}Host a\n  Port 2222\nHost b\n  User bob\n");
        let p = f.load();
        assert!(p.load_warnings().is_empty(), "{:?}", p.load_warnings());
        assert_eq!(p.query("a", &f.env).unwrap().port, Some(2222));
        assert_eq!(p.query("b", &f.env).unwrap().port, None, "not global");
    }

    #[test]
    fn unsupported_match_warns_with_criteria() {
        let f = Fixture::new();
        f.user_config("Match exec \"true\" host x\n  Port 1\nMatch canonical\n  Port 2\n");
        let p = f.load();
        assert_eq!(
            codes(p.load_warnings()),
            vec![
                ConfigWarningCode::UnsupportedMatchCriteria {
                    criteria: "exec".into()
                },
                ConfigWarningCode::UnsupportedMatchCriteria {
                    criteria: "canonical".into()
                },
            ]
        );
    }

    #[test]
    fn include_relative_tilde_glob_order_nested_missing() {
        let f = Fixture::new();
        f.user_file("conf.d/b.conf", "Host b\n  Port 2\n");
        // The nested Include comes before any Host line, so it is unconditional.
        f.user_file("conf.d/a.conf", "Include nested/*.conf\nHost a\n  Port 1\n");
        f.user_file("conf.d/.hidden.conf", "Host hidden\n");
        f.user_file("nested/n.conf", "Host n\n  Port 3\n");
        let home_inc = f.home().join("extra.conf");
        std::fs::write(&home_inc, "Host h\n  Port 4\n").unwrap();
        f.user_config("Include conf.d/*.conf ~/extra.conf missing.conf\n");
        let p = f.load();
        assert!(p.load_warnings().is_empty(), "{:?}", p.load_warnings());
        let names: Vec<String> = p
            .files_read()
            .iter()
            .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            ["config", "a.conf", "n.conf", "b.conf", "extra.conf"]
        );
        assert_eq!(p.include_count(), 4);
        for (alias, port) in [("a", 1), ("b", 2), ("n", 3), ("h", 4)] {
            assert_eq!(p.query(alias, &f.env).unwrap().port, Some(port), "{alias}");
        }
        assert_eq!(
            p.query("hidden", &f.env).unwrap().port,
            None,
            "dotfiles need a literal leading dot"
        );
        let dirs = p.watch_dirs();
        assert!(dirs.contains(&f.ssh_dir().join("conf.d")));
        assert!(dirs.contains(&f.ssh_dir().join("nested")));
        assert!(dirs.contains(&f.ssh_dir()));
        assert!(dirs.contains(&f.home().to_path_buf()));
    }

    #[test]
    fn include_pattern_dir_is_watched_without_matches() {
        let f = Fixture::new();
        f.user_config("Include later.d/*\n");
        let p = f.load();
        assert!(p.watch_dirs().contains(&f.ssh_dir().join("later.d")));
        assert_eq!(p.include_count(), 0);
    }

    #[test]
    fn include_cycle_is_detected() {
        let f = Fixture::new();
        f.user_file("a.conf", "Include b.conf\nHost a\n  Port 1\n");
        f.user_file("b.conf", "Include a.conf\nHost b\n  Port 2\n");
        f.user_config("Include a.conf\n");
        let p = f.load();
        let cycles: Vec<_> = p
            .load_warnings()
            .iter()
            .filter(|w| matches!(w.kind, ConfigWarningCode::IncludeCycle { .. }))
            .collect();
        assert_eq!(cycles.len(), 1, "{:?}", p.load_warnings());
        assert_eq!(p.query("a", &f.env).unwrap().port, Some(1));
        assert_eq!(p.query("b", &f.env).unwrap().port, Some(2));
    }

    #[test]
    fn include_self_is_a_cycle() {
        let f = Fixture::new();
        f.user_config("Include config\nPort 7\n");
        let p = f.load();
        assert!(matches!(
            p.load_warnings()[0].kind,
            ConfigWarningCode::IncludeCycle { .. }
        ));
        assert_eq!(p.query("x", &f.env).unwrap().port, Some(7));
    }

    #[test]
    fn include_depth_17_is_rejected() {
        let f = Fixture::new();
        // config (depth 0) → d1 … d16 allowed; d16 including d17 exceeds the limit.
        for i in 1..=17 {
            f.user_file(
                &format!("d{i}.conf"),
                &format!("Include d{}.conf\nHost h{i}\n  Port {i}\n", i + 1),
            );
        }
        f.user_config("Include d1.conf\n");
        let p = f.load();
        let w = p.load_warnings();
        assert_eq!(
            codes(w),
            vec![ConfigWarningCode::IncludeDepthExceeded {
                pattern: "d17.conf".into()
            }]
        );
        assert_eq!(p.query("h16", &f.env).unwrap().port, Some(16));
        assert_eq!(p.query("h17", &f.env).unwrap().port, None);
    }

    #[test]
    fn recursive_glob_is_rejected() {
        let f = Fixture::new();
        f.user_file("deep/x/y.conf", "Port 9\n");
        f.user_config("Include deep/**/*.conf\n");
        let p = f.load();
        assert_eq!(
            codes(p.load_warnings()),
            vec![ConfigWarningCode::IncludeRecursiveGlob {
                pattern: "deep/**/*.conf".into()
            }]
        );
        assert_eq!(p.query("x", &f.env).unwrap().port, None);
    }

    /// Glob matches are examined lazily and capped per Include argument, so skipped
    /// (here: disallowed) matches cannot produce an unbounded number of warnings.
    #[test]
    fn include_matches_are_capped_per_argument() {
        let f = Fixture::new();
        for i in 0..(MAX_INCLUDE_MATCHES + 20) {
            f.user_file(&format!("many/denied{i:05}.conf"), "");
        }
        f.user_config("Include many/*.conf\n");
        let src = PickySource {
            untrusted: vec![],
            too_large: vec![],
            disallow_marker: Some("denied".into()),
        };
        let p = f.load_with(&src);
        let c = codes(p.load_warnings());
        assert_eq!(c.len(), MAX_INCLUDE_MATCHES + 1);
        assert_eq!(
            c.last(),
            Some(&ConfigWarningCode::IncludeTooManyFiles {
                pattern: "many/*.conf".into()
            })
        );
    }

    /// Options after a broken `Host`/`Match` line do not leak into the previous block.
    #[test]
    fn options_after_a_bad_block_line_attach_to_nothing() {
        let f = Fixture::new();
        f.user_config(
            "Host a\n  Port 1\nHost\n  Port 2\n  User mallory\nHost b\n  Port 3\n\
             Match\n  Port 4\nHost=\n  Port 5\n  Match \"unterminated\n  Port 6\n",
        );
        let p = f.load();
        let a = p.query("a", &f.env).unwrap();
        assert_eq!(a.port, Some(1));
        assert_eq!(a.user, None);
        assert_eq!(p.query("b", &f.env).unwrap().port, Some(3));
        let bad: Vec<u32> = p.load_warnings().iter().map(|w| w.location.line).collect();
        assert_eq!(bad, vec![3, 8, 10, 12]);
        assert!(opens_block("  HOST"));
        assert!(opens_block("match="));
        assert!(!opens_block("Hostname"));
        assert!(!opens_block(""));
    }

    #[test]
    fn more_than_256_files_are_capped() {
        let f = Fixture::new();
        for i in 0..300 {
            f.user_file(&format!("many/f{i:03}.conf"), "");
        }
        f.user_config("Include many/*.conf\n");
        let p = f.load();
        assert_eq!(p.files_read().len(), MAX_CONFIG_FILES);
        assert_eq!(
            codes(p.load_warnings()),
            vec![ConfigWarningCode::IncludeTooManyFiles {
                pattern: "many/*.conf".into()
            }]
        );
    }

    #[test]
    fn untrusted_and_too_large_files() {
        let f = Fixture::new();
        let bad = f.user_file("bad.conf", "Port 1\n");
        let big = f.user_file("big.conf", "Port 2\n");
        f.user_file("zz-remote-share/x.conf", "Port 3\n");
        f.user_config("Include bad.conf big.conf zz-remote-share/*.conf\nPort 22\n");
        let src = PickySource {
            untrusted: vec![bad.clone()],
            too_large: vec![big.clone()],
            disallow_marker: Some("zz-remote-share".into()),
        };
        let p = f.load_with(&src);
        let c = codes(p.load_warnings());
        assert_eq!(c.len(), 3, "{c:?}");
        assert_eq!(
            c[0],
            ConfigWarningCode::UntrustedFile {
                file: bad.display().to_string()
            }
        );
        assert_eq!(
            c[1],
            ConfigWarningCode::FileTooLarge {
                file: big.display().to_string()
            }
        );
        assert!(
            matches!(&c[2], ConfigWarningCode::UntrustedFile { file } if file.contains("zz-remote-share"))
        );
        assert_eq!(p.query("x", &f.env).unwrap().port, Some(22));
    }

    #[test]
    fn untrusted_main_file_is_ignored_with_warning() {
        let f = Fixture::new();
        let cfg = f.user_config("Port 1\n");
        let src = PickySource {
            untrusted: vec![cfg.clone()],
            too_large: vec![],
            disallow_marker: None,
        };
        let p = f.load_with(&src);
        assert_eq!(
            codes(p.load_warnings()),
            vec![ConfigWarningCode::UntrustedFile {
                file: cfg.display().to_string()
            }]
        );
        assert!(p.files_read().is_empty());
        assert_eq!(p.query("x", &f.env).unwrap().port, None);
    }

    #[test]
    fn system_includes_are_relative_to_system_dir() {
        let f = Fixture::new();
        std::fs::create_dir_all(f.env.system_ssh_dir.join("ssh_config.d")).unwrap();
        std::fs::write(
            f.env.system_ssh_dir.join("ssh_config.d").join("10.conf"),
            "Port 10\n",
        )
        .unwrap();
        f.system_config("Include ssh_config.d/*.conf\n");
        let p = f.load();
        assert_eq!(p.query("x", &f.env).unwrap().port, Some(10));
    }

    #[test]
    fn pattern_dirs() {
        assert_eq!(
            pattern_dir(Path::new("/a/b/*.conf")),
            Some(PathBuf::from("/a/b"))
        );
        assert_eq!(pattern_dir(Path::new("/a/b?/c")), Some(PathBuf::from("/a")));
        assert_eq!(
            pattern_dir(Path::new("/a/b/c.conf")),
            Some(PathBuf::from("/a/b"))
        );
    }
}
