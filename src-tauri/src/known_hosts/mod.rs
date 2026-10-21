//! Own OpenSSH `known_hosts` parser, matcher and writer (§2.1, §5.3, §9.3).
//!
//! - [`parse`] is tolerant: comments, blank lines, tabs, `@revoked`, `@cert-authority`, hashed
//!   `|1|salt|hash` hosts, comma lists, `*`/`?` wildcards, `!` negation and `[host]:port`.
//!   Unparsable lines are skipped and reported as [`ParseIssue`]s for logging.
//! - [`verify`] returns one [`Verdict`] with the §5.3 priority
//!   Revoked > CaManaged > Trusted > Changed > Unknown.
//! - [`check_against_pin`] is the reconnect pin decision (§5.3 steps 1–2) as a pure function.
//! - [`learn`] appends exactly one validated line (never russh's `learn_known_hosts_path`).
//!
//! The module is pure apart from [`learn`]: reading the files (path allowlist, trust check,
//! 4 MiB cap) is the caller's job (§7.4.1, §9.4), and keys are passed as raw SSH wire blobs, so
//! there is no russh dependency here.

#![deny(clippy::indexing_slicing)]

pub mod parse;
pub mod verify;
pub mod write;

use std::collections::HashSet;
use std::sync::{Mutex, PoisonError};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD_NO_PAD;

pub use parse::{
    Entry, HostPattern, HostPatterns, KnownHostsFile, Marker, Origin, ParseIssue, parse,
};
pub use verify::{PinDecision, RejectReason, Verdict, check_against_pin, known_key_types, verify};
pub use write::{LearnError, learn};

/// A host public key as the SSH wire-format blob (`string key-type, …`), e.g. the base64-decoded
/// third field of a known_hosts line or `PublicKey::to_bytes()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostKeyRef<'a> {
    pub key_type: &'a str,
    pub blob: &'a [u8],
}

/// `SHA256:<base64 without padding>` of the key blob — the OpenSSH / russh fingerprint format.
pub fn fingerprint_sha256(blob: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, blob);
    format!("SHA256:{}", STANDARD_NO_PAD.encode(digest.as_ref()))
}

/// Lookup name per hop (OpenSSH `sshconnect.c`): `HostKeyAlias` verbatim if set; else `hostname`
/// if the port is 22; else `[hostname]:port`. ASCII-lowercased like OpenSSH (`ssh.c` lowercases
/// the host name and `HostKeyAlias` before host-key matching).
pub fn lookup_name(hostname: &str, port: u16, host_key_alias: Option<&str>) -> String {
    let name = match host_key_alias {
        Some(alias) => alias.to_owned(),
        None if port == 22 => hostname.to_owned(),
        None => format!("[{hostname}]:{port}"),
    };
    name.to_ascii_lowercase()
}

/// The key type string embedded at the start of an SSH public key blob, if well-formed.
pub fn blob_key_type(blob: &[u8]) -> Option<&str> {
    let len_bytes: [u8; 4] = blob.get(..4)?.try_into().ok()?;
    let len = usize::try_from(u32::from_be_bytes(len_bytes)).ok()?;
    let end = 4usize.checked_add(len)?;
    std::str::from_utf8(blob.get(4..end)?).ok()
}

/// Process-wide "trust once" decisions (host key accepted without "Add to known_hosts"):
/// `(lookup, fingerprint)` pairs, in memory only (§5.3 step 3).
#[derive(Debug, Default)]
pub struct TrustOnceApprovals {
    approved: Mutex<HashSet<(String, String)>>,
}

impl TrustOnceApprovals {
    pub fn insert(&self, lookup: &str, fingerprint: &str) {
        self.approved
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((lookup.to_owned(), fingerprint.to_owned()));
    }

    pub fn contains(&self, lookup: &str, fingerprint: &str) -> bool {
        self.approved
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&(lookup.to_owned(), fingerprint.to_owned()))
    }
}

/// Test-only SSH key blobs in real wire format (deterministic, no comments).
#[cfg(test)]
pub(crate) mod test_keys {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;

    fn put_string(out: &mut Vec<u8>, data: &[u8]) {
        let len = u32::try_from(data.len()).unwrap_or(u32::MAX);
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(data);
    }

    /// `ssh-ed25519` blob with a 32-byte public key derived from `seed`.
    pub fn ed25519(seed: u8) -> Vec<u8> {
        let mut blob = Vec::new();
        put_string(&mut blob, b"ssh-ed25519");
        let key: Vec<u8> = (0u8..32)
            .map(|i| i.wrapping_mul(7).wrapping_add(seed))
            .collect();
        put_string(&mut blob, &key);
        blob
    }

    /// `ssh-rsa` blob (e = 65537, 2048-bit modulus derived from `seed`).
    pub fn rsa(seed: u8) -> Vec<u8> {
        let mut blob = Vec::new();
        put_string(&mut blob, b"ssh-rsa");
        put_string(&mut blob, &[0x01, 0x00, 0x01]);
        let mut n = vec![0x00, 0xC3];
        n.extend((0u16..255).map(|i| u8::try_from((i * 31 + u16::from(seed)) % 256).unwrap_or(0)));
        put_string(&mut blob, &n);
        blob
    }

    pub fn b64(blob: &[u8]) -> String {
        STANDARD.encode(blob)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    #[test]
    fn lookup_names() {
        assert_eq!(lookup_name("example.com", 22, None), "example.com");
        assert_eq!(lookup_name("Example.COM", 22, None), "example.com");
        assert_eq!(lookup_name("example.com", 2222, None), "[example.com]:2222");
        assert_eq!(lookup_name("2001:db8::1", 2222, None), "[2001:db8::1]:2222");
        // HostKeyAlias + non-22 port: the plain alias (no brackets, no port).
        assert_eq!(lookup_name("10.0.0.5", 2222, Some("MyAlias")), "myalias");
        assert_eq!(
            lookup_name("10.0.0.5", 22, Some("db-primary")),
            "db-primary"
        );
    }

    #[test]
    fn fingerprint_format() {
        let fp = fingerprint_sha256(&test_keys::ed25519(1));
        assert!(fp.starts_with("SHA256:"));
        assert!(!fp.ends_with('='));
        assert_eq!(fp.len(), "SHA256:".len() + 43);
        assert_ne!(fp, fingerprint_sha256(&test_keys::ed25519(2)));
    }

    #[test]
    fn blob_types() {
        assert_eq!(blob_key_type(&test_keys::ed25519(1)), Some("ssh-ed25519"));
        assert_eq!(blob_key_type(&test_keys::rsa(1)), Some("ssh-rsa"));
        assert_eq!(blob_key_type(&[0, 0, 0, 9, b'a']), None);
        assert_eq!(blob_key_type(&[]), None);
    }

    #[test]
    fn trust_once() {
        let t = TrustOnceApprovals::default();
        assert!(!t.contains("h", "SHA256:x"));
        t.insert("h", "SHA256:x");
        assert!(t.contains("h", "SHA256:x"));
        assert!(!t.contains("h", "SHA256:y"));
        assert!(!t.contains("[h]:2222", "SHA256:x"));
    }
}
