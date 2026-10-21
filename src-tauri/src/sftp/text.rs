//! Text codec of the editor (§6.3 read, §6.4 encode): BOM sniffing, explicit encodings, binary
//! detection, strict UTF-8, EOL detection/normalisation, SHA-256 etags. Pure functions (no I/O).
//!
//! Encoding names in DTOs are lower-case WHATWG names (`utf-8`, `windows-1251`, `koi8-u`,
//! `utf-16le`, …). Labels are resolved with `encoding_rs::Encoding::for_label`, so aliases work
//! (`latin1` and `iso-8859-1` resolve to `windows-1252`, as in browsers). UTF-16 is encoded by
//! hand because `encoding_rs` only decodes it.

use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE};

use crate::error::{AppError, ErrorCode};
use crate::ipc::dto::Eol;

/// Bytes inspected by the binary check (§6.3: NUL in the first 8000 bytes ⇒ binary).
pub const BINARY_SNIFF: usize = 8000;

/// A decoded document (before it is wrapped into a `TextDocument`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    /// Text with every line ending normalised to `\n`.
    pub text: String,
    pub encoding: String,
    pub has_bom: bool,
    pub eol: Eol,
    pub mixed_eol: bool,
}

/// Lower-case canonical name of an encoding.
pub fn encoding_name(enc: &'static Encoding) -> String {
    enc.name().to_ascii_lowercase()
}

/// Resolve an encoding label; `replacement` (which decodes everything to U+FFFD) is refused.
pub fn resolve(label: &str) -> Result<&'static Encoding, AppError> {
    let unsupported = || {
        AppError::new(ErrorCode::UnsupportedEncoding {
            encoding: label.chars().take(64).collect(),
        })
    };
    let enc = Encoding::for_label(label.trim().as_bytes()).ok_or_else(unsupported)?;
    if enc == encoding_rs::REPLACEMENT {
        return Err(unsupported());
    }
    Ok(enc)
}

/// §6.3 step 3–4: BOM sniff → explicit encoding → binary check → strict UTF-8; then EOL.
pub fn decode(path: &str, raw: &[u8], explicit: Option<&str>) -> Result<Decoded, AppError> {
    let decode_failed = |enc: &'static Encoding| {
        AppError::new(ErrorCode::DecodeFailed {
            path: path.to_owned(),
            encoding: encoding_name(enc),
        })
    };
    let (enc, has_bom, body) = match Encoding::for_bom(raw) {
        Some((enc, len)) => (enc, true, raw.get(len..).unwrap_or_default()),
        None => match explicit {
            Some(label) => (resolve(label)?, false, raw),
            None => {
                let head = raw.get(..BINARY_SNIFF.min(raw.len())).unwrap_or_default();
                if head.contains(&0) {
                    return Err(AppError::new(ErrorCode::FileIsBinary {
                        path: path.to_owned(),
                    }));
                }
                (UTF_8, false, raw)
            }
        },
    };
    let text = enc
        .decode_without_bom_handling_and_without_replacement(body)
        .ok_or_else(|| decode_failed(enc))?;
    let (text, eol, mixed_eol) = normalize_eol(&text);
    Ok(Decoded {
        text,
        encoding: encoding_name(enc),
        has_bom,
        eol,
        mixed_eol,
    })
}

/// Dominant line ending (ties → LF), whether more than one kind occurs, and the text with every
/// ending normalised to `\n`.
pub fn normalize_eol(text: &str) -> (String, Eol, bool) {
    let (mut crlf, mut lf, mut cr) = (0usize, 0usize, 0usize);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' if chars.peek() == Some(&'\n') => {
                chars.next();
                crlf += 1;
                out.push('\n');
            }
            '\r' => {
                cr += 1;
                out.push('\n');
            }
            '\n' => {
                lf += 1;
                out.push('\n');
            }
            other => out.push(other),
        }
    }
    let kinds = [crlf, lf, cr].iter().filter(|n| **n > 0).count();
    let eol = if crlf > lf && crlf >= cr {
        Eol::Crlf
    } else if cr > lf && cr > crlf {
        Eol::Cr
    } else {
        Eol::Lf
    };
    (out, eol, kinds > 1)
}

/// §6.4: text (`\n` line endings; stray `\r\n`/`\r` are normalised first) → bytes in `encoding`
/// with `eol`, plus a BOM for Unicode encodings when `has_bom`. Unmappable characters ⇒
/// `encodingUnmappable`.
pub fn encode(text: &str, encoding: &str, has_bom: bool, eol: Eol) -> Result<Vec<u8>, AppError> {
    let enc = resolve(encoding)?;
    let (normalized, _, _) = normalize_eol(text);
    let sep = match eol {
        Eol::Lf => "\n",
        Eol::Crlf => "\r\n",
        Eol::Cr => "\r",
    };
    let with_eol = if sep == "\n" {
        normalized
    } else {
        normalized.replace('\n', sep)
    };
    if enc == UTF_8 {
        let mut out = Vec::with_capacity(with_eol.len() + 3);
        if has_bom {
            out.extend_from_slice(b"\xEF\xBB\xBF");
        }
        out.extend_from_slice(with_eol.as_bytes());
        return Ok(out);
    }
    if enc == UTF_16LE || enc == UTF_16BE {
        let le = enc == UTF_16LE;
        let mut out = Vec::with_capacity(with_eol.len() * 2 + 2);
        let mut push = |u: u16| {
            out.extend_from_slice(&if le { u.to_le_bytes() } else { u.to_be_bytes() });
        };
        if has_bom {
            push(0xFEFF);
        }
        for u in with_eol.encode_utf16() {
            push(u);
        }
        return Ok(out);
    }
    let (bytes, _, unmappable) = enc.encode(&with_eol);
    // `encode` substitutes HTML numeric references for unmappable characters; refuse instead.
    if unmappable || enc.output_encoding() != enc {
        return Err(AppError::new(ErrorCode::EncodingUnmappable {
            encoding: encoding_name(enc),
        }));
    }
    Ok(bytes.into_owned())
}

/// Lower-case hex SHA-256 (etags, §6.3 step 5).
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
    digest.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn code(e: &AppError) -> &ErrorCode {
        &e.kind
    }

    #[test]
    fn utf8_lf_and_crlf() {
        let d = decode("/f", b"a\nb\n", None).unwrap();
        assert_eq!(d.text, "a\nb\n");
        assert_eq!((d.eol, d.mixed_eol, d.has_bom), (Eol::Lf, false, false));
        assert_eq!(d.encoding, "utf-8");
        let d = decode("/f", b"a\r\nb\r\nc\n", None).unwrap();
        assert_eq!(d.text, "a\nb\nc\n");
        assert_eq!((d.eol, d.mixed_eol), (Eol::Crlf, true));
        let d = decode("/f", b"a\rb\r", None).unwrap();
        assert_eq!((d.eol, d.mixed_eol), (Eol::Cr, false));
        assert_eq!(d.text, "a\nb\n");
        // Ties → LF.
        let d = decode("/f", b"a\r\nb\n", None).unwrap();
        assert_eq!((d.eol, d.mixed_eol), (Eol::Lf, true));
        let d = decode("/f", b"", None).unwrap();
        assert_eq!((d.text.as_str(), d.eol), ("", Eol::Lf));
    }

    #[test]
    fn bom_binary_and_strict_utf8() {
        let d = decode("/f", b"\xEF\xBB\xBFhi", None).unwrap();
        assert_eq!(
            (d.text.as_str(), d.has_bom, d.encoding.as_str()),
            ("hi", true, "utf-8")
        );
        // UTF-16 with BOM is text even though it contains NUL bytes.
        let d = decode("/f", b"\xFF\xFEh\0i\0", None).unwrap();
        assert_eq!((d.text.as_str(), d.encoding.as_str()), ("hi", "utf-16le"));
        let e = decode("/f", b"ab\0cd", None).unwrap_err();
        assert!(matches!(code(&e), ErrorCode::FileIsBinary { .. }));
        // NUL after the sniff window: strict UTF-8 accepts it.
        let mut late = vec![b'a'; BINARY_SNIFF];
        late.push(0);
        assert!(decode("/f", &late, None).is_ok());
        let e = decode("/f", b"caf\xE9", None).unwrap_err();
        assert!(
            matches!(code(&e), ErrorCode::DecodeFailed { encoding, .. } if encoding == "utf-8")
        );
    }

    #[test]
    fn explicit_encodings() {
        // "Привіт" in windows-1251 and koi8-u.
        let cp1251 = b"\xCF\xF0\xE8\xE2\xB3\xF2";
        let d = decode("/f", cp1251, Some("windows-1251")).unwrap();
        assert_eq!(d.text, "Привіт");
        assert_eq!(d.encoding, "windows-1251");
        let koi = encode("Привіт", "koi8-u", false, Eol::Lf).unwrap();
        assert_eq!(decode("/f", &koi, Some("KOI8-U")).unwrap().text, "Привіт");
        assert_eq!(
            decode("/f", b"caf\xE9", Some("iso-8859-1"))
                .unwrap()
                .encoding,
            "windows-1252"
        );
        let e = decode("/f", b"x", Some("no-such")).unwrap_err();
        assert!(matches!(code(&e), ErrorCode::UnsupportedEncoding { .. }));
        let e = decode("/f", b"x", Some("replacement")).unwrap_err();
        assert!(matches!(code(&e), ErrorCode::UnsupportedEncoding { .. }));
        // Malformed for the chosen encoding.
        let e = decode("/f", b"\xFF\xFE\x00", Some("utf-16le")).unwrap_err();
        assert!(matches!(code(&e), ErrorCode::DecodeFailed { .. }), "{e:?}");
    }

    #[test]
    fn encode_round_trips() {
        assert_eq!(
            encode("a\nb", "utf-8", true, Eol::Crlf).unwrap(),
            b"\xEF\xBB\xBFa\r\nb"
        );
        assert_eq!(
            encode("a\r\nb\rc", "utf-8", false, Eol::Lf).unwrap(),
            b"a\nb\nc"
        );
        assert_eq!(
            encode("hi\n", "utf-16le", true, Eol::Lf).unwrap(),
            b"\xFF\xFEh\0i\0\n\0"
        );
        assert_eq!(encode("hi", "utf-16be", false, Eol::Lf).unwrap(), b"\0h\0i");
        let bytes = encode("Привіт\n", "windows-1251", false, Eol::Crlf).unwrap();
        assert_eq!(bytes, b"\xCF\xF0\xE8\xE2\xB3\xF2\r\n");
        let e = encode("✓", "windows-1251", false, Eol::Lf).unwrap_err();
        assert!(
            matches!(code(&e), ErrorCode::EncodingUnmappable { encoding } if encoding == "windows-1251")
        );
        let e = encode("x", "bogus", false, Eol::Lf).unwrap_err();
        assert!(matches!(code(&e), ErrorCode::UnsupportedEncoding { .. }));
        // Decode → encode is byte-identical for a mixed document saved with its dominant EOL.
        let raw = "x\r\ny\r\n".as_bytes();
        let d = decode("/f", raw, None).unwrap();
        assert_eq!(encode(&d.text, &d.encoding, d.has_bom, d.eol).unwrap(), raw);
    }

    #[test]
    fn sha256_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
