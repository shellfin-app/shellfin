//! Remote POSIX paths (§6.2, §9.4): the only place that splits and joins remote paths.
//!
//! Paths from the frontend are validated before use: non-empty, absolute (`/…`, which also covers
//! Windows OpenSSH servers' `/C:/…` form), no NUL. Paths that contain U+FFFD came from a name
//! russh-sftp decoded lossily (§13 risk 4): the server-side name differs from what the UI shows, so
//! operations on them are refused (the lossy text could match another, real file).

use crate::error::AppError;

/// The replacement character russh-sftp substitutes for undecodable name bytes.
pub const LOSSY: char = '\u{FFFD}';

/// `invalidArgument{field}` unless `path` is a usable absolute remote path.
pub fn validate(path: &str, field: &str) -> Result<(), AppError> {
    if path.is_empty() || !path.starts_with('/') || path.contains('\0') || path.len() > 4096 {
        return Err(AppError::invalid_argument(field));
    }
    Ok(())
}

/// [`validate`], and additionally refuse lossy paths (U+FFFD) for operations that act on a file.
pub fn validate_exact(path: &str, field: &str) -> Result<(), AppError> {
    validate(path, field)?;
    if path.contains(LOSSY) {
        return Err(AppError::invalid_argument(field).with_detail("path contains U+FFFD"));
    }
    Ok(())
}

/// `true` if a directory entry name returned by the server can be used as one path component.
pub fn valid_entry_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\0'])
}

/// `dir` + `/` + `name` (no doubled slash).
/// Allocates exactly the needed capacity (listings hold one per entry).
pub fn join(dir: &str, name: &str) -> String {
    let sep = !dir.ends_with('/');
    let mut out = String::with_capacity(dir.len() + usize::from(sep) + name.len());
    out.push_str(dir);
    if sep {
        out.push('/');
    }
    out.push_str(name);
    out
}

fn trim_trailing(path: &str) -> &str {
    let t = path.trim_end_matches('/');
    if t.is_empty() && path.starts_with('/') {
        "/"
    } else {
        t
    }
}

/// Last component (`"/a/b/"` → `"b"`, `"/"` → `""`).
pub fn basename(path: &str) -> &str {
    let t = trim_trailing(path);
    if t == "/" {
        return "";
    }
    t.rsplit('/').next().unwrap_or(t)
}

/// Parent directory (`"/a/b"` → `"/a"`, `"/a"` → `"/"`, `"/"` → `"/"`).
pub fn parent(path: &str) -> &str {
    let t = trim_trailing(path);
    match t.rfind('/') {
        Some(0) | None => "/",
        Some(i) => t.get(..i).unwrap_or("/"),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn components() {
        assert_eq!(join("/a", "b"), "/a/b");
        assert_eq!(join("/", "b"), "/b");
        assert_eq!(join("/a/", "b"), "/a/b");
        assert_eq!(basename("/a/b"), "b");
        assert_eq!(basename("/a/b/"), "b");
        assert_eq!(basename("/"), "");
        assert_eq!(basename("/C:/x.txt"), "x.txt");
        assert_eq!(parent("/a/b"), "/a");
        assert_eq!(parent("/a"), "/");
        assert_eq!(parent("/"), "/");
        assert_eq!(parent("/a/b/"), "/a");
    }

    #[test]
    fn validation() {
        assert!(validate("/home/x", "path").is_ok());
        assert!(validate("/C:/Users", "path").is_ok());
        for bad in ["", "relative", "a/b", "/a\0b"] {
            assert!(validate(bad, "path").is_err(), "{bad:?}");
        }
        assert!(validate(&format!("/{}", "a".repeat(5000)), "path").is_err());
        assert!(validate("/x\u{FFFD}", "path").is_ok());
        assert!(validate_exact("/x\u{FFFD}", "path").is_err());
        assert!(valid_entry_name("a b"));
        for bad in ["", ".", "..", "a/b", "a\0"] {
            assert!(!valid_entry_name(bad), "{bad:?}");
        }
    }
}
