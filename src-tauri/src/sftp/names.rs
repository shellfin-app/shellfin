//! Local file names for downloads (§6.6, §9.4): the remote name is server-controlled and hostile.
//!
//! `sanitize_local_name(remote)`: last segment after `/` and `\`; `<>:"/\|?*`, C0, DEL and C1 →
//! `_`; bidi, zero-width and other invisible format characters (`is_deceptive_char` without
//! U+FE0E/FE0F) removed; reserved device names
//! (`CON`, `NUL`, `COM1`…, with any extension) get `_` appended to the stem; trailing dots and
//! spaces trimmed; at most 200 UTF-8 bytes (the extension is kept; bytes, not
//! characters, so the name plus a `.NN.part` suffix stays within 255 bytes on Linux/macOS and
//! 255 UTF-16 units on Windows, which never exceed the UTF-8 byte count); empty → `download`.
//! The same rules apply on every OS, so a name that is safe on Windows is also used on Unix.

use crate::platform::local_path::is_reserved_device_name;

/// Characters that are removed outright: the `is_deceptive_char` set (bidi controls, zero-width
/// and other invisible format characters, annotation marks, tags, §9.5) except the presentation
/// selectors U+FE0E/FE0F, which stay so emoji file names (`❤️.txt`) keep their look and their
/// on-disk name.
fn is_invisible(c: char) -> bool {
    !matches!(c, '\u{FE0E}' | '\u{FE0F}') && crate::ssh::shellquote::is_deceptive_char(c)
}

fn is_forbidden(c: char) -> bool {
    matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() // C0, DEL, C1
}

/// Maximum length of a sanitized name in UTF-8 bytes.
pub const MAX_NAME_BYTES: usize = 200;
/// Longest extension kept when a name is shortened (bytes, incl. the dot).
const MAX_EXT_BYTES: usize = 20;
/// Default name when nothing usable remains.
pub const FALLBACK: &str = "download";

pub fn sanitize_local_name(remote: &str) -> String {
    let last = remote.rsplit(['/', '\\']).next().unwrap_or(remote);
    let mut name: String = last
        .chars()
        .filter(|c| !is_invisible(*c))
        .map(|c| if is_forbidden(c) { '_' } else { c })
        .collect();
    name = name.trim_end_matches(['.', ' ']).to_owned();
    if name.len() > MAX_NAME_BYTES {
        name = shorten(&name);
    }
    if is_reserved_device_name(&name) {
        let stem_end = name.find('.').unwrap_or(name.len());
        name.insert(stem_end, '_');
    }
    if name.is_empty() {
        FALLBACK.to_owned()
    } else {
        name
    }
}

/// Keep a short extension (≤ 20 bytes incl. the dot) and truncate the stem at a character
/// boundary so the whole name is ≤ [`MAX_NAME_BYTES`].
fn shorten(name: &str) -> String {
    let ext = name
        .rfind('.')
        .filter(|&i| i > 0)
        .and_then(|i| name.get(i..))
        .filter(|e| e.len() <= MAX_EXT_BYTES)
        .unwrap_or("");
    let stem_len = name.len() - ext.len();
    let mut keep = MAX_NAME_BYTES.saturating_sub(ext.len()).min(stem_len);
    while !name.is_char_boundary(keep) {
        keep -= 1;
    }
    let stem = name.get(..keep).unwrap_or_default();
    let stem = stem.trim_end_matches(['.', ' ']);
    format!("{stem}{ext}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_names_and_streams() {
        assert_eq!(sanitize_local_name("CON"), "CON_");
        assert_eq!(sanitize_local_name("CON.txt"), "CON_.txt");
        assert_eq!(sanitize_local_name("nul.tar.gz"), "nul_.tar.gz");
        assert_eq!(sanitize_local_name("com1"), "com1_");
        assert_eq!(sanitize_local_name("a:b"), "a_b");
        assert_eq!(
            sanitize_local_name("file.txt:Zone.Identifier"),
            "file.txt_Zone.Identifier"
        );
        assert_eq!(sanitize_local_name("CONSOLE.txt"), "CONSOLE.txt");
    }

    #[test]
    fn traversal_and_separators() {
        assert_eq!(sanitize_local_name("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_local_name("..\\..\\win.ini"), "win.ini");
        assert_eq!(sanitize_local_name(".."), FALLBACK);
        assert_eq!(sanitize_local_name("."), FALLBACK);
        assert_eq!(sanitize_local_name("dir/"), FALLBACK);
        assert_eq!(sanitize_local_name(""), FALLBACK);
        assert_eq!(sanitize_local_name("\\\\srv\\share\\x"), "x");
    }

    #[test]
    fn controls_bidi_and_trailing() {
        assert_eq!(sanitize_local_name("a\u{202E}txt.exe"), "atxt.exe");
        assert_eq!(sanitize_local_name("x\u{200B}y"), "xy");
        assert_eq!(sanitize_local_name("bell\u{7}\u{1b}[31m"), "bell__[31m");
        assert_eq!(sanitize_local_name("c1\u{85}"), "c1_");
        assert_eq!(sanitize_local_name("name. . "), "name");
        assert_eq!(sanitize_local_name("q?<>|*\"x"), "q______x");
        assert_eq!(sanitize_local_name("Привіт ✓.txt"), "Привіт ✓.txt");
        assert_eq!(sanitize_local_name("CON .txt"), "CON _.txt");
    }

    /// The whole `is_deceptive_char` set is removed (#33), presentation selectors stay.
    #[test]
    fn invisible_and_annotation_characters_are_removed() {
        assert_eq!(
            sanitize_local_name("invoice\u{FFF9}.pdf\u{FFFA}.exe"),
            "invoice.pdf.exe"
        );
        assert_eq!(
            sanitize_local_name("\u{FFEF}a\u{FFF0}b\u{FFF8}c\u{FFF9}d\u{FFFB}e\u{FFFC}"),
            "\u{FFEF}abcde\u{FFFC}"
        );
        for hostile in [
            "re\u{00AD}port.pdf",
            "re\u{034F}port.pdf",
            "re\u{115F}\u{1160}\u{3164}\u{FFA0}port.pdf",
            "re\u{17B4}\u{180E}port.pdf",
            "re\u{2028}\u{2029}port.pdf",
            "re\u{2065}\u{206A}\u{206F}port.pdf",
            "re\u{FE00}\u{FE0D}port.pdf",
            "re\u{1BCA0}\u{1D173}port.pdf",
            "re\u{E0041}\u{E007F}\u{E0100}port.pdf",
            "\u{FEFF}report\u{202E}.pdf",
        ] {
            assert_eq!(sanitize_local_name(hostile), "report.pdf", "{hostile:?}");
        }
        // Emoji names keep their presentation selectors (unchanged on disk).
        for emoji in [
            "\u{2764}\u{FE0F}.txt",
            "1\u{FE0F}\u{20E3} notes.md",
            "\u{1F600}\u{FE0E}",
        ] {
            assert_eq!(sanitize_local_name(emoji), emoji);
        }
        // Nothing usable left, or only dots after stripping: the fallback.
        for empty in [
            "\u{FFF9}\u{FFFA}\u{FFFB}",
            "\u{FFF9}..\u{FFFB}",
            ".\u{200B}.",
            "dir/\u{E0001}",
            "\u{00AD} . \u{2028}",
        ] {
            assert_eq!(sanitize_local_name(empty), FALLBACK, "{empty:?}");
        }
        // Stripping cannot smuggle in a reserved device name or a separator.
        assert_eq!(sanitize_local_name("C\u{FFF9}ON.txt"), "CON_.txt");
        assert_eq!(sanitize_local_name("..\u{200B}/x\u{FFFA}"), "x");
    }

    #[test]
    fn length_cap_keeps_extension() {
        let long = format!("{}.tar.gz", "x".repeat(400));
        let s = sanitize_local_name(&long);
        assert_eq!(s.len(), MAX_NAME_BYTES);
        assert!(s.ends_with(".gz"), "{s}");
        let s = sanitize_local_name(&"ж".repeat(300));
        assert_eq!(s.len(), MAX_NAME_BYTES);
    }

    /// Multi-byte and non-BMP names are capped in bytes, so the name plus the longest
    /// `.part` suffix (`.99.part`) fits 255 bytes (Unix) and 255 UTF-16 units (Windows).
    #[test]
    fn length_cap_is_in_bytes() {
        let worst_suffix = ".99.part";
        for long in [
            "ж".repeat(200),
            "😀".repeat(200),
            format!("{}.txt", "€".repeat(150)),
            format!("{}.{}", "a".repeat(150), "😀".repeat(30)),
            format!("x{}", "😀".repeat(100)),
        ] {
            let s = sanitize_local_name(&long);
            assert!(s.len() <= MAX_NAME_BYTES, "{} bytes", s.len());
            assert!(s.len() + worst_suffix.len() <= 255);
            assert!(s.encode_utf16().count() + worst_suffix.len() <= 255);
            assert!(!s.is_empty());
        }
        let s = sanitize_local_name(&format!("{}.txt", "€".repeat(150)));
        assert!(s.ends_with(".txt"), "{s}");
        // Short names are untouched.
        assert_eq!(sanitize_local_name("😀.txt"), "😀.txt");
    }
}
