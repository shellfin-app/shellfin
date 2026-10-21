//! Shell quoting (§4.5, §5.3, §9.5).
//!
//! - [`cd_wrapper`]: the only remote command built from data ("Open terminal here"). sshd runs an
//!   exec request as `<login shell> -c <command>`, and login shells disagree on quoting (fish
//!   treats `\'` inside `'…'` as an escape; csh/tcsh reject `${VAR:-x}`), so the wire command
//!   contains **no path-derived shell syntax**: every byte of the path becomes a 3-digit octal
//!   escape `\NNN` inside a fixed `/bin/sh -c` wrapper. Spike #35 verified this for
//!   sh/bash/dash/zsh/fish/tcsh/csh.
//! - [`posix_quote`] / [`powershell_quote`]: the local `ssh-keygen -R` hint shown with
//!   `hostKeyChanged` (§5.3), quoted for the local platform.

use std::fmt::Write as _;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum QuoteError {
    /// The value contains NUL or another C0 control (or DEL).
    #[error("value contains control characters")]
    ControlCharacters,
    /// `cd_wrapper` needs an absolute POSIX path.
    #[error("path is not absolute")]
    NotAbsolute,
}

fn has_controls(s: &str) -> bool {
    s.chars().any(|c| (c as u32) < 0x20 || c == '\u{7f}')
}

/// Fixed wrapper; `{}` is replaced by the octal-escaped path (only `\` and digits).
const WRAPPER_HEAD: &str = r#"exec /bin/sh -c 'cd -- "$(printf ""#;
const WRAPPER_TAIL: &str = r#"")" && exec "${SHELL:-/bin/sh}" -l'"#;

/// Same head, but a failing `cd` (folder gone,
/// no permission) leaves the shell in the exec's start directory (home) instead of ending it.
const FALLBACK_TAIL: &str = r#"")" 2>/dev/null; exec "${SHELL:-/bin/sh}" -l'"#;

/// Longest path [`cd_wrapper_or_home`] accepts (the remote path limit, §4.2).
pub const MAX_CWD_BYTES: usize = 4096;

/// `exec /bin/sh -c 'cd -- "$(printf "<P>")" && exec "${SHELL:-/bin/sh}" -l'` where `<P>` is
/// every UTF-8 byte of `path` as `\NNN`. `path` must be absolute and free of NUL/C0 controls
/// (`$(…)` would also strip trailing newlines) — callers map errors to
/// `invalidArgument{field:"cwd"}`.
pub fn cd_wrapper(path: &str) -> Result<String, QuoteError> {
    wrap(path, WRAPPER_TAIL)
}

/// Like [`cd_wrapper`], but `cd … 2>/dev/null;` instead of `&&`, so the login
/// shell starts in home when the folder cannot be entered. Same input rules (the caller falls
/// back to a plain login shell on an error).
pub fn cd_wrapper_or_home(path: &str) -> Result<String, QuoteError> {
    wrap(path, FALLBACK_TAIL)
}

fn wrap(path: &str, tail: &str) -> Result<String, QuoteError> {
    if has_controls(path) {
        return Err(QuoteError::ControlCharacters);
    }
    if !path.starts_with('/') {
        return Err(QuoteError::NotAbsolute);
    }
    let mut out = String::with_capacity(WRAPPER_HEAD.len() + tail.len() + path.len() * 4);
    out.push_str(WRAPPER_HEAD);
    for b in path.bytes() {
        // Writing to a String cannot fail.
        let _ = write!(out, "\\{b:03o}");
    }
    out.push_str(tail);
    Ok(out)
}

/// POSIX single-quoting: `'` → `'\''`. NUL/C0 rejected.
pub fn posix_quote(s: &str) -> Result<String, QuoteError> {
    if has_controls(s) {
        return Err(QuoteError::ControlCharacters);
    }
    Ok(format!("'{}'", s.replace('\'', r"'\''")))
}

/// PowerShell single-quoting: `'` → `''` (also the typographic quotes PowerShell treats as
/// quote characters). NUL/C0 rejected.
pub fn powershell_quote(s: &str) -> Result<String, QuoteError> {
    if has_controls(s) {
        return Err(QuoteError::ControlCharacters);
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            out.push(c);
        }
        out.push(c);
    }
    out.push('\'');
    Ok(out)
}

/// Characters that render differently from what is copied (bidi controls, zero-width and other
/// default-ignorable/invisible code points). A copy-paste security instruction must not contain
/// them.
pub fn is_deceptive_char(c: char) -> bool {
    matches!(c,
        '\u{00AD}' | '\u{034F}' | '\u{061C}' | '\u{115F}' | '\u{1160}' | '\u{17B4}' | '\u{17B5}'
        | '\u{180B}'..='\u{180F}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
        | '\u{2028}' | '\u{2029}' | '\u{2060}'..='\u{206F}' | '\u{3164}' | '\u{FE00}'..='\u{FE0F}'
        | '\u{FEFF}' | '\u{FFA0}' | '\u{FFF0}'..='\u{FFF8}' | '\u{E0000}'..='\u{E0FFF}')
}

/// The copyable removal hint for `hostKeyChanged` (§5.3):
/// `ssh-keygen -R '<lookup>' -f '<file>'` quoted for `shell`, where `lookup` is the real
/// known_hosts entry name (`HostKeyAlias` verbatim, else `host` / `[host]:port`).
/// `None` if a value cannot be shown and copied safely: control, bidi or invisible characters
/// (both shells), or a backslash for POSIX (fish treats `\'` inside single quotes as an escape,
/// so `'…\'…'` would break out of the quotes there).
pub fn keygen_remove_hint(
    lookup: &str,
    known_hosts_file: &str,
    shell: crate::error::HintShell,
) -> Option<String> {
    use crate::error::HintShell;
    let safe = |s: &str| {
        !s.chars().any(is_deceptive_char) && (shell != HintShell::Posix || !s.contains('\\'))
    };
    if !safe(lookup) || !safe(known_hosts_file) {
        return None;
    }
    let q = match shell {
        HintShell::Powershell => powershell_quote,
        HintShell::Posix => posix_quote,
    };
    Some(format!(
        "ssh-keygen -R {} -f {}",
        q(lookup).ok()?,
        q(known_hosts_file).ok()?
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    #[test]
    fn wrapper_shape() {
        assert_eq!(
            cd_wrapper("/a b").unwrap(),
            r#"exec /bin/sh -c 'cd -- "$(printf "\057\141\040\142")" && exec "${SHELL:-/bin/sh}" -l'"#
        );
        assert_eq!(cd_wrapper("relative"), Err(QuoteError::NotAbsolute));
        assert_eq!(cd_wrapper(""), Err(QuoteError::NotAbsolute));
        for bad in [
            "/a\0b",
            "/a\nb",
            "/a\rb",
            "/x\u{1b}[31m",
            "/x\u{7f}",
            "/trailing\n",
        ] {
            assert_eq!(
                cd_wrapper(bad),
                Err(QuoteError::ControlCharacters),
                "{bad:?}"
            );
        }
    }

    /// The payload between the fixed head and tail contains only `\` and octal digits, and
    /// decodes back to the exact path bytes.
    fn check_wrapper(path: &str) {
        let w = cd_wrapper(path).unwrap();
        let payload = w
            .strip_prefix(WRAPPER_HEAD)
            .unwrap()
            .strip_suffix(WRAPPER_TAIL)
            .unwrap();
        assert!(
            payload
                .bytes()
                .all(|b| b == b'\\' || (b'0'..=b'7').contains(&b)),
            "{payload}"
        );
        assert!(!payload.contains('\''));
        // The single-quoted outer string has no `'`, no `\'`, no `\\` anywhere.
        let inner = &w["exec /bin/sh -c '".len()..w.len() - 1];
        assert!(!inner.contains('\''), "{inner}");
        assert!(!w.contains(r"\'"));
        assert!(!w.contains(r"\\"));
        let decoded: Vec<u8> = payload
            .split('\\')
            .skip(1)
            .map(|o| u8::from_str_radix(o, 8).unwrap())
            .collect();
        assert_eq!(decoded, path.as_bytes());
    }

    #[test]
    fn hostile_names() {
        for name in [
            "/tmp/x';touch /tmp/P;#",
            r"/tmp/x\';touch /tmp/P;#",
            r"/tmp/x\\';touch /tmp/P",
            "/tmp/a&b",
            "/tmp/$(touch P)",
            "/tmp/`touch P`",
            "/tmp/\"q\"",
            r"/tmp/back\slash",
            "/tmp/bang!",
            "/tmp/Привіт ✓",
            "/tmp/with space",
            "/tmp/${SHELL}",
            "/",
            // Printf directives and backslash escapes in the name are carried as octal
            // escapes too (`\045` prints `%`, never starts a conversion; `\134` prints `\`).
            "/tmp/100%",
            "/tmp/%s%d%n%b%%",
            r"/tmp/a\cb",
            r"/tmp/x\ny\t\0101\x41",
            r"/tmp/\\\\",
        ] {
            check_wrapper(name);
        }
        let w = cd_wrapper("/%\\").unwrap();
        assert!(w.contains(r"\057\045\134"), "{w}");
        assert!(!w.contains('%'), "no raw `%` reaches printf: {w}");
    }

    /// The fallback wrapper differs only in its fixed tail, carries the path the same
    /// way (only `\` and octal digits) and refuses the same inputs.
    #[test]
    fn fallback_wrapper_shape_and_hostile_names() {
        assert_eq!(
            cd_wrapper_or_home("/a b").unwrap(),
            r#"exec /bin/sh -c 'cd -- "$(printf "\057\141\040\142")" 2>/dev/null; exec "${SHELL:-/bin/sh}" -l'"#
        );
        for bad in [
            "/a\nb",
            "/a\0",
            "/x\u{1b}]0;t\u{7}",
            "/trailing\n",
            "/a\rb",
            "/\u{7f}",
        ] {
            assert_eq!(
                cd_wrapper_or_home(bad),
                Err(QuoteError::ControlCharacters),
                "{bad:?}"
            );
        }
        assert_eq!(cd_wrapper_or_home("tmp"), Err(QuoteError::NotAbsolute));
        assert_eq!(cd_wrapper_or_home(""), Err(QuoteError::NotAbsolute));
        for name in [
            "/tmp/x';reboot;#",
            r"/tmp/x\';reboot;#",
            "/tmp/$(reboot)",
            "/tmp/`reboot`",
            "/tmp/a; reboot",
            "/tmp/a && reboot",
            "/tmp/a | reboot",
            "/tmp/\"q\" > /etc/x",
            "/tmp/\u{2028}line\u{2029}",
            "/tmp/\u{85}nel",
            "/tmp/\u{202e}evil",
            "/srv/Привіт ✓",
        ] {
            let w = cd_wrapper_or_home(name).unwrap();
            let payload = w
                .strip_prefix(WRAPPER_HEAD)
                .unwrap()
                .strip_suffix(FALLBACK_TAIL)
                .unwrap();
            assert!(
                payload
                    .bytes()
                    .all(|b| b == b'\\' || (b'0'..=b'7').contains(&b)),
                "{name:?}: {payload}"
            );
            let inner = &w["exec /bin/sh -c '".len()..w.len() - 1];
            assert!(!inner.contains('\''), "{inner}");
            let decoded: Vec<u8> = payload
                .split('\\')
                .skip(1)
                .map(|o| u8::from_str_radix(o, 8).unwrap())
                .collect();
            assert_eq!(decoded, name.as_bytes());
        }
    }

    /// Property test: arbitrary byte strings (as valid UTF-8 without controls) round-trip and
    /// never put quote/backslash syntax into the wrapper.
    #[test]
    fn property_arbitrary_paths() {
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let alphabet: Vec<char> =
            "/'\"\\`$(){}[];&|<>!#*?~ -_.aZ09\u{e9}\u{416}\u{2713}\u{202e}\u{1f600}"
                .chars()
                .collect();
        for _ in 0..5_000 {
            let len = (next() % 40) as usize;
            let mut s = String::from("/");
            for _ in 0..len {
                s.push(alphabet[(next() % alphabet.len() as u64) as usize]);
            }
            check_wrapper(&s);
        }
    }

    #[test]
    fn local_quoting() {
        assert_eq!(posix_quote("[h]:2222").unwrap(), "'[h]:2222'");
        assert_eq!(posix_quote("it's").unwrap(), r"'it'\''s'");
        assert_eq!(powershell_quote("it's").unwrap(), "'it''s'");
        assert_eq!(
            powershell_quote("a\u{2019}b").unwrap(),
            "'a\u{2019}\u{2019}b'"
        );
        assert!(posix_quote("a\nb").is_err());
        assert!(powershell_quote("a\0b").is_err());
    }

    /// Hint per shell, incl. the PowerShell typographic-quote break-out and fish.
    #[test]
    fn keygen_hint_quoting_and_rejections() {
        use crate::error::HintShell::{Posix, Powershell};
        assert_eq!(
            keygen_remove_hint("[example.com]:2222", "/home/alice/.ssh/known_hosts", Posix)
                .unwrap(),
            "ssh-keygen -R '[example.com]:2222' -f '/home/alice/.ssh/known_hosts'"
        );
        // HostKeyAlias: the plain alias is the entry name.
        assert_eq!(
            keygen_remove_hint("it-alias", "/home/alice/.ssh/known_hosts", Posix).unwrap(),
            "ssh-keygen -R 'it-alias' -f '/home/alice/.ssh/known_hosts'"
        );
        // PowerShell: a path with a typographic quote cannot break out (it is doubled).
        let ps = keygen_remove_hint("h", "C:\\x\\\u{2019}; New-Item PWNED; \u{2019}", Powershell)
            .unwrap();
        assert_eq!(
            ps,
            "ssh-keygen -R 'h' -f 'C:\\x\\\u{2019}\u{2019}; New-Item PWNED; \u{2019}\u{2019}'"
        );
        assert_eq!(
            keygen_remove_hint("h", "C:\\Users\\alice\\.ssh\\known_hosts", Powershell).unwrap(),
            "ssh-keygen -R 'h' -f 'C:\\Users\\alice\\.ssh\\known_hosts'"
        );
        // POSIX (fish-safe): backslashes refused rather than mis-quoted.
        assert!(keygen_remove_hint("h", "/tmp/a\\'b", Posix).is_none());
        // Display ≠ copy: bidi / zero-width / controls refused for every shell.
        for bad in [
            "a\u{202E}b",
            "a\u{200B}b",
            "a\u{FEFF}b",
            "a\nb",
            "a\u{E0041}",
        ] {
            assert!(keygen_remove_hint(bad, "/f", Posix).is_none(), "{bad:?}");
            assert!(
                keygen_remove_hint("h", bad, Powershell).is_none(),
                "{bad:?}"
            );
        }
    }
}
