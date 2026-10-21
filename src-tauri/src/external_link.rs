//! External links (§9.4, §4.2 `app_open_external`): only http/https, parsed and re-serialized
//! with the `url` crate; the backend shows a **native** confirm with the real URL first (host in
//! punycode — `Url::host_str()` is already IDNA-encoded), then the frontend-supplied title/body
//! after sanitizing (C0/C1 and bidi controls removed, ≤ 200 chars each). Only one confirm may be
//! open at a time.

use std::sync::atomic::{AtomicBool, Ordering};

use url::Url;

use crate::error::AppError;

pub const MAX_TEXT_CHARS: usize = 200;
/// URLs longer than this are refused (a confirm the user cannot read is no confirmation).
pub const MAX_URL_CHARS: usize = 2048;

/// Parse and normalise an external URL. Errors are `invalidArgument{field:"url"}`.
/// URLs with embedded credentials (`https://user:pw@host/`) are refused: the user part is a
/// classic way to make the visible URL start with a trusted-looking name.
pub fn normalize_external_url(raw: &str) -> Result<Url, AppError> {
    let invalid = || AppError::invalid_argument("url");
    if raw.chars().count() > MAX_URL_CHARS {
        return Err(invalid());
    }
    let url = Url::parse(raw.trim()).map_err(|e| invalid().with_detail(e.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(invalid().with_detail(format!("scheme {} not allowed", url.scheme())));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(invalid().with_detail("missing host"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid().with_detail("URLs with credentials are not opened"));
    }
    // The limit applies to what is shown and opened (percent-encoding can grow it).
    if url.as_str().len() > MAX_URL_CHARS {
        return Err(invalid().with_detail("URL too long after normalisation"));
    }
    Ok(url)
}

/// Remove C0/C1 controls, bidi controls and invisible / default-ignorable characters (line and
/// paragraph separators and newlines become spaces), then truncate to `MAX_TEXT_CHARS`.
pub fn sanitize_dialog_text(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .filter_map(|c| match c {
            '\n' | '\r' | '\t' | '\u{2028}' | '\u{2029}' => Some(' '),
            c if c.is_control() || crate::ssh::shellquote::is_deceptive_char(c) => None,
            c => Some(c),
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.chars().count() <= MAX_TEXT_CHARS {
        return trimmed.to_owned();
    }
    let mut out: String = trimmed.chars().take(MAX_TEXT_CHARS - 1).collect();
    out.push('…');
    out
}

/// The confirm text: the URL on the first line, then the (sanitized) frontend body.
pub fn confirm_message(url: &Url, body: &str) -> String {
    let body = sanitize_dialog_text(body);
    if body.is_empty() {
        url.as_str().to_owned()
    } else {
        format!("{}\n\n{body}", url.as_str())
    }
}

static CONFIRM_OPEN: AtomicBool = AtomicBool::new(false);

/// Held while a confirm dialog is open; a second request gets `None` (the command returns `false`).
pub struct ConfirmSlot(());

impl ConfirmSlot {
    pub fn try_acquire() -> Option<Self> {
        CONFIRM_OPEN
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self(()))
    }
}

impl Drop for ConfirmSlot {
    fn drop(&mut self) {
        CONFIRM_OPEN.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::error::ErrorCode;

    #[test]
    fn length_is_checked_after_normalisation() {
        // 700 spaces percent-encode to 2100 chars: under the raw limit, over the shown one.
        let raw = format!("https://example.com/a{}b", " ".repeat(700));
        assert!(raw.chars().count() <= MAX_URL_CHARS);
        let err = normalize_external_url(&raw).unwrap_err();
        assert_eq!(
            err.kind,
            ErrorCode::InvalidArgument {
                field: "url".into()
            }
        );
    }

    #[test]
    fn only_http_and_https() {
        assert_eq!(
            normalize_external_url("https://example.com/a b")
                .unwrap()
                .as_str(),
            "https://example.com/a%20b"
        );
        assert_eq!(
            normalize_external_url(" HTTP://Example.COM ")
                .unwrap()
                .as_str(),
            "http://example.com/"
        );
        for bad in [
            "javascript:alert(1)",
            "file:///C:/Windows/win.ini",
            "data:text/html,x",
            "tauri://localhost",
            "ftp://example.com",
            "mailto:alice@example.com",
            "https://",
            "not a url",
            "https://alice:secret@example.com/",
            "https://trusted.example@evil.example/",
        ] {
            let err = normalize_external_url(bad).unwrap_err();
            assert_eq!(
                err.kind,
                ErrorCode::InvalidArgument {
                    field: "url".into()
                },
                "{bad}"
            );
        }
        assert!(
            normalize_external_url(&format!(
                "https://example.com/{}",
                "a".repeat(MAX_URL_CHARS)
            ))
            .is_err()
        );
    }

    #[test]
    fn idn_host_is_shown_in_punycode() {
        let u = normalize_external_url("https://bücher.example/path").unwrap();
        assert_eq!(u.host_str(), Some("xn--bcher-kva.example"));
        assert!(confirm_message(&u, "").starts_with("https://xn--bcher-kva.example/path"));
        // Cyrillic look-alike of "apple.com" is visibly different in punycode.
        let spoof = normalize_external_url("https://аррӏе.com/").unwrap();
        assert!(spoof.host_str().unwrap().starts_with("xn--"));
    }

    #[test]
    fn texts_are_sanitized_and_capped() {
        assert_eq!(
            sanitize_dialog_text("Open\u{202E}gpj.exe\u{0}\u{9b}\nlink?"),
            "Opengpj.exe link?"
        );
        // Separators become spaces; invisibles are dropped.
        assert_eq!(
            sanitize_dialog_text(
                "a\u{2028}b\u{2029}c\u{200B}d\u{FEFF}e\u{2060}f\u{3164}g\u{E0041}h\u{00AD}i"
            ),
            "a b cdefghi"
        );
        let long = "x".repeat(500);
        let s = sanitize_dialog_text(&long);
        assert_eq!(s.chars().count(), MAX_TEXT_CHARS);
        assert!(s.ends_with('…'));
        let u = normalize_external_url("https://example.com/").unwrap();
        assert_eq!(
            confirm_message(&u, " Body\u{7}"),
            "https://example.com/\n\nBody"
        );
    }

    #[test]
    fn one_confirm_at_a_time() {
        let first = ConfirmSlot::try_acquire().unwrap();
        assert!(ConfirmSlot::try_acquire().is_none());
        drop(first);
        assert!(ConfirmSlot::try_acquire().is_some());
    }
}
