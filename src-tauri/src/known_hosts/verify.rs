//! Verdicts (§5.3) and the reconnect pin decision.

use super::HostKeyRef;
use super::parse::{HostPatterns, KnownHostsFile, Marker};

/// The result of checking one host key against all known_hosts files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// A plain entry for the lookup lists exactly this key.
    Trusted { file: String, line: u32 },
    /// An `@revoked` line for the lookup lists this key.
    Revoked { file: String, line: u32 },
    /// An `@cert-authority` line matches the lookup and no plain entry trusts the key
    /// (host certificates are not negotiated → fail closed, §5.3).
    CaManaged { file: String, line: u32 },
    /// A plain entry for the lookup lists a different key of the same type.
    Changed { file: String, line: u32 },
    /// Nothing for this key type; `other_types` = key types recorded for the lookup.
    Unknown { other_types: Vec<String> },
}

/// Check `key` for `lookup` against `files` (in order: user files, then global files).
/// Priority: Revoked > CaManaged > Trusted > Changed > Unknown. Within a class the first
/// matching line (file order, then line order) is reported.
pub fn verify(lookup: &str, key: HostKeyRef<'_>, files: &[KnownHostsFile]) -> Verdict {
    let mut revoked = None;
    let mut ca = None;
    let mut trusted = None;
    let mut changed = None;
    let mut other_types: Vec<String> = Vec::new();
    let lower = Lookup::new(lookup);

    for file in files {
        for e in &file.entries {
            if !lower.matches(&e.hosts) {
                continue;
            }
            let at = || (file.display.clone(), e.line);
            match e.marker {
                Marker::Revoked => {
                    if revoked.is_none() && e.blob == key.blob {
                        revoked = Some(at());
                    }
                }
                Marker::CertAuthority => {
                    if ca.is_none() {
                        ca = Some(at());
                    }
                }
                Marker::None => {
                    if e.key_type == key.key_type {
                        if e.blob == key.blob {
                            if trusted.is_none() {
                                trusted = Some(at());
                            }
                        } else if changed.is_none() {
                            changed = Some(at());
                        }
                    } else if !other_types.contains(&e.key_type) {
                        other_types.push(e.key_type.clone());
                    }
                }
            }
        }
    }

    if let Some((file, line)) = revoked {
        return Verdict::Revoked { file, line };
    }
    if let Some((file, line)) = ca
        && trusted.is_none()
    {
        return Verdict::CaManaged { file, line };
    }
    if let Some((file, line)) = trusted {
        return Verdict::Trusted { file, line };
    }
    if let Some((file, line)) = changed {
        return Verdict::Changed { file, line };
    }
    Verdict::Unknown { other_types }
}

/// Key types recorded for `lookup` in plain entries (file order, deduplicated). Used to put
/// already-known algorithms first in russh's `Preferred.key` (§5.2).
pub fn known_key_types(lookup: &str, files: &[KnownHostsFile]) -> Vec<String> {
    let mut types: Vec<String> = Vec::new();
    let lower = Lookup::new(lookup);
    for e in files.iter().flat_map(|f| &f.entries) {
        if e.marker == Marker::None && lower.matches(&e.hosts) && !types.contains(&e.key_type) {
            types.push(e.key_type.clone());
        }
    }
    types
}

/// The lookup in both forms, lowercased once per check: plain patterns are compared
/// case-insensitively, hashed hosts against the lookup as given (exactly like
/// [`HostPatterns::matches`]).
struct Lookup<'a> {
    raw: &'a str,
    lower: std::borrow::Cow<'a, str>,
}

impl<'a> Lookup<'a> {
    fn new(raw: &'a str) -> Self {
        let lower = if raw.bytes().any(|b| b.is_ascii_uppercase()) {
            std::borrow::Cow::Owned(raw.to_ascii_lowercase())
        } else {
            std::borrow::Cow::Borrowed(raw)
        };
        Self { raw, lower }
    }

    fn matches(&self, hosts: &HostPatterns) -> bool {
        match hosts {
            HostPatterns::List(_) => hosts.matches_lowercase(&self.lower),
            HostPatterns::Hashed { .. } => hosts.matches_lowercase(self.raw),
        }
    }
}

/// Why a reconnect host key is rejected (§5.3 steps 1–2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectReason {
    Revoked,
    CaManaged { file: String, line: u32 },
    Changed { file: String, line: u32 },
    MismatchOnReconnect { expected: String, actual: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinDecision {
    /// Same key as pinned.
    Accept,
    /// Different key, but known_hosts trusts it (HA/LB backends) → re-pin (verification knownHosts).
    Repin,
    Reject(RejectReason),
}

/// Reconnect decision for a hop with an existing pin. Fatal verdicts (Revoked, CaManaged, Changed)
/// are evaluated **before** the pin comparison, so a key revoked, CA-managed or replaced in
/// known_hosts mid-session is rejected on auto-reconnect even if it equals the pin.
pub fn check_against_pin(
    verdict: &Verdict,
    pinned_fingerprint: &str,
    actual_fingerprint: &str,
) -> PinDecision {
    match verdict {
        Verdict::Revoked { .. } => return PinDecision::Reject(RejectReason::Revoked),
        Verdict::CaManaged { file, line } => {
            return PinDecision::Reject(RejectReason::CaManaged {
                file: file.clone(),
                line: *line,
            });
        }
        Verdict::Changed { file, line } => {
            return PinDecision::Reject(RejectReason::Changed {
                file: file.clone(),
                line: *line,
            });
        }
        Verdict::Trusted { .. } | Verdict::Unknown { .. } => {}
    }
    if pinned_fingerprint == actual_fingerprint {
        PinDecision::Accept
    } else if matches!(verdict, Verdict::Trusted { .. }) {
        PinDecision::Repin
    } else {
        PinDecision::Reject(RejectReason::MismatchOnReconnect {
            expected: pinned_fingerprint.to_owned(),
            actual: actual_fingerprint.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use std::path::Path;

    use super::*;
    use crate::known_hosts::fingerprint_sha256;
    use crate::known_hosts::parse::{Origin, parse};
    use crate::known_hosts::test_keys::{b64, ed25519, rsa};

    fn file(display: &str, origin: Origin, text: &str) -> KnownHostsFile {
        let (f, issues) = parse(Path::new(display), display, origin, text.as_bytes());
        assert!(issues.is_empty(), "{issues:?}");
        f
    }

    fn key(blob: &[u8]) -> HostKeyRef<'_> {
        HostKeyRef {
            key_type: crate::known_hosts::blob_key_type(blob).unwrap(),
            blob,
        }
    }

    fn trusted(file: &str, line: u32) -> Verdict {
        Verdict::Trusted {
            file: file.into(),
            line,
        }
    }

    #[test]
    fn trusted_unknown_changed() {
        let (k1, k2, r1) = (ed25519(1), ed25519(2), rsa(1));
        let f = file(
            "u",
            Origin::User,
            &format!(
                "example.com ssh-ed25519 {}\nexample.com ssh-rsa {}\n",
                b64(&k1),
                b64(&r1)
            ),
        );
        let files = [f];
        assert_eq!(verify("example.com", key(&k1), &files), trusted("u", 1));
        assert_eq!(verify("example.com", key(&r1), &files), trusted("u", 2));
        assert_eq!(
            verify("example.com", key(&k2), &files),
            Verdict::Changed {
                file: "u".into(),
                line: 1
            }
        );
        assert_eq!(
            verify("other.com", key(&k1), &files),
            Verdict::Unknown {
                other_types: vec![]
            }
        );

        let only_rsa = [file(
            "u",
            Origin::User,
            &format!("example.com ssh-rsa {}\n", b64(&r1)),
        )];
        assert_eq!(
            verify("example.com", key(&k1), &only_rsa),
            Verdict::Unknown {
                other_types: vec!["ssh-rsa".into()]
            }
        );
    }

    #[test]
    fn host_port_lookup() {
        let k = ed25519(1);
        let files = [file(
            "u",
            Origin::User,
            &format!("[h.example]:2222 ssh-ed25519 {}\n", b64(&k)),
        )];
        assert_eq!(verify("[h.example]:2222", key(&k), &files), trusted("u", 1));
        assert_eq!(
            verify("h.example", key(&k), &files),
            Verdict::Unknown {
                other_types: vec![]
            }
        );
        assert_eq!(
            verify("[h.example]:22", key(&k), &files),
            Verdict::Unknown {
                other_types: vec![]
            }
        );
    }

    #[test]
    fn hashed_entry_trusts() {
        let k = ed25519(5);
        let files = [file(
            "u",
            Origin::User,
            &format!(
                "|1|O33ESRMWPVkMYIwJ1Uw+n877jTo=|nuuC5vEqXlEZ/8BXQR7m619W6Ak= ssh-ed25519 {}\n",
                b64(&k)
            ),
        )];
        assert_eq!(verify("example.com", key(&k), &files), trusted("u", 1));
        assert_eq!(
            verify("example.org", key(&k), &files),
            Verdict::Unknown {
                other_types: vec![]
            }
        );
    }

    #[test]
    fn wildcard_and_negation() {
        let k = ed25519(1);
        let files = [file(
            "u",
            Origin::User,
            &format!("*.int,!vault.int ssh-ed25519 {}\n", b64(&k)),
        )];
        assert_eq!(verify("db.int", key(&k), &files), trusted("u", 1));
        assert_eq!(
            verify("vault.int", key(&k), &files),
            Verdict::Unknown {
                other_types: vec![]
            }
        );
    }

    #[test]
    fn revoked_wins_over_everything() {
        let k = ed25519(1);
        let files = [
            file("u", Origin::User, &format!("h ssh-ed25519 {}\n", b64(&k))),
            file(
                "g",
                Origin::Global,
                &format!(
                    "@cert-authority * ssh-rsa {}\n@revoked * ssh-ed25519 {}\n",
                    b64(&rsa(9)),
                    b64(&k)
                ),
            ),
        ];
        assert_eq!(
            verify("h", key(&k), &files),
            Verdict::Revoked {
                file: "g".into(),
                line: 2
            }
        );
        // A revoked line for a *different* key does not affect this key; the `@cert-authority *`
        // line then wins over Changed because no plain entry trusts the other key.
        let other = ed25519(2);
        assert_eq!(
            verify("h", key(&other), &files),
            Verdict::CaManaged {
                file: "g".into(),
                line: 1
            }
        );
        let no_ca = [
            files[0].clone(),
            file(
                "g",
                Origin::Global,
                &format!("@revoked * ssh-ed25519 {}\n", b64(&k)),
            ),
        ];
        assert_eq!(
            verify("h", key(&other), &no_ca),
            Verdict::Changed {
                file: "u".into(),
                line: 1
            }
        );
    }

    #[test]
    fn cert_authority_is_ca_managed_unless_plainly_trusted() {
        let (k, ca) = (ed25519(1), rsa(9));
        let ca_only = [file(
            "u",
            Origin::User,
            &format!("@cert-authority *.example.com ssh-rsa {}\n", b64(&ca)),
        )];
        assert_eq!(
            verify("web.example.com", key(&k), &ca_only),
            Verdict::CaManaged {
                file: "u".into(),
                line: 1
            }
        );
        assert_eq!(
            verify("example.org", key(&k), &ca_only),
            Verdict::Unknown {
                other_types: vec![]
            }
        );

        let ca_and_plain = [
            file(
                "u",
                Origin::User,
                &format!("web.example.com ssh-ed25519 {}\n", b64(&k)),
            ),
            file(
                "g",
                Origin::Global,
                &format!("@cert-authority *.example.com ssh-rsa {}\n", b64(&ca)),
            ),
        ];
        assert_eq!(
            verify("web.example.com", key(&k), &ca_and_plain),
            trusted("u", 1)
        );
        // A changed key under a CA line: CaManaged beats Changed.
        assert_eq!(
            verify("web.example.com", key(&ed25519(2)), &ca_and_plain),
            Verdict::CaManaged {
                file: "g".into(),
                line: 1
            }
        );
    }

    #[test]
    fn multiple_files_first_match_reported_and_trusted_beats_changed() {
        let (k1, k2) = (ed25519(1), ed25519(2));
        let files = [
            file(
                "user",
                Origin::User,
                &format!("# x\nh ssh-ed25519 {}\n", b64(&k2)),
            ),
            file(
                "global",
                Origin::Global,
                &format!("h ssh-ed25519 {}\n", b64(&k1)),
            ),
        ];
        // HA pool: both keys listed (in different files) → Trusted, not Changed.
        assert_eq!(verify("h", key(&k1), &files), trusted("global", 1));
        assert_eq!(verify("h", key(&k2), &files), trusted("user", 2));
        assert_eq!(
            verify("h", key(&ed25519(3)), &files),
            Verdict::Changed {
                file: "user".into(),
                line: 2
            }
        );
    }

    #[test]
    fn known_types_for_preferred_order() {
        let files = [
            file(
                "u",
                Origin::User,
                &format!(
                    "h ssh-rsa {}\nh ssh-ed25519 {}\n@revoked h ssh-ed25519 {}\n",
                    b64(&rsa(1)),
                    b64(&ed25519(1)),
                    b64(&ed25519(2))
                ),
            ),
            file(
                "g",
                Origin::Global,
                &format!(
                    "h ssh-rsa {}\nother ssh-ed25519 {}\n",
                    b64(&rsa(2)),
                    b64(&ed25519(3))
                ),
            ),
        ];
        assert_eq!(
            known_key_types("h", &files),
            vec!["ssh-rsa".to_owned(), "ssh-ed25519".to_owned()]
        );
        assert!(known_key_types("nobody", &files).is_empty());
    }

    // ---- reconnect pin (§10.1) ----

    #[test]
    fn pin_same_key_accept() {
        let k = ed25519(1);
        let fp = fingerprint_sha256(&k);
        let files = [file(
            "u",
            Origin::User,
            &format!("h ssh-ed25519 {}\n", b64(&k)),
        )];
        assert_eq!(
            check_against_pin(&verify("h", key(&k), &files), &fp, &fp),
            PinDecision::Accept
        );
        // trust-once pin (not in known_hosts) and same key → Accept
        assert_eq!(
            check_against_pin(&verify("h", key(&k), &[]), &fp, &fp),
            PinDecision::Accept
        );
    }

    #[test]
    fn pin_different_trusted_key_repins() {
        let (k1, k2) = (ed25519(1), ed25519(2));
        let files = [file(
            "u",
            Origin::User,
            &format!("h ssh-ed25519 {}\nh ssh-ed25519 {}\n", b64(&k1), b64(&k2)),
        )];
        let d = check_against_pin(
            &verify("h", key(&k2), &files),
            &fingerprint_sha256(&k1),
            &fingerprint_sha256(&k2),
        );
        assert_eq!(d, PinDecision::Repin);
    }

    #[test]
    fn pin_different_untrusted_key_mismatch() {
        let (k1, k2) = (ed25519(1), ed25519(2));
        let (fp1, fp2) = (fingerprint_sha256(&k1), fingerprint_sha256(&k2));
        // Pinned via trust-once / auto-accept: nothing in known_hosts.
        let d = check_against_pin(&verify("h", key(&k2), &[]), &fp1, &fp2);
        assert_eq!(
            d,
            PinDecision::Reject(RejectReason::MismatchOnReconnect {
                expected: fp1,
                actual: fp2
            })
        );
    }

    #[test]
    fn pin_now_revoked() {
        let k = ed25519(1);
        let fp = fingerprint_sha256(&k);
        let files = [file(
            "u",
            Origin::User,
            &format!(
                "h ssh-ed25519 {}\n@revoked * ssh-ed25519 {}\n",
                b64(&k),
                b64(&k)
            ),
        )];
        assert_eq!(
            check_against_pin(&verify("h", key(&k), &files), &fp, &fp),
            PinDecision::Reject(RejectReason::Revoked)
        );
    }

    #[test]
    fn pin_now_ca_managed() {
        let k = ed25519(1);
        let fp = fingerprint_sha256(&k);
        let files = [file(
            "u",
            Origin::User,
            &format!("@cert-authority h ssh-rsa {}\n", b64(&rsa(3))),
        )];
        assert_eq!(
            check_against_pin(&verify("h", key(&k), &files), &fp, &fp),
            PinDecision::Reject(RejectReason::CaManaged {
                file: "u".into(),
                line: 1
            })
        );
    }

    #[test]
    fn pin_line_replaced_by_other_key_of_same_type() {
        let (k1, k_new) = (ed25519(1), ed25519(7));
        let fp = fingerprint_sha256(&k1);
        // The pinned key was learned, then the user's file was edited to list a different ed25519 key.
        let files = [file(
            "u",
            Origin::User,
            &format!("h ssh-ed25519 {}\n", b64(&k_new)),
        )];
        assert_eq!(
            check_against_pin(&verify("h", key(&k1), &files), &fp, &fp),
            PinDecision::Reject(RejectReason::Changed {
                file: "u".into(),
                line: 1
            })
        );
    }

    /// A hostile 4 MiB known_hosts (the read cap) full of backtracking
    /// wildcard patterns is checked in bounded time, and the verdict is still right.
    #[test]
    fn hostile_four_mib_file_is_verified_in_bounded_time() {
        let k = b64(&ed25519(1));
        let star = "*a".repeat(20);
        let mut text = String::new();
        let mut i = 0u32;
        while text.len() < 4 * 1024 * 1024 - 200 {
            text.push_str(&format!("{star}b,!x{i}*?*?* ssh-ed25519 {k}\n"));
            i += 1;
        }
        let trusted = ed25519(2);
        text.push_str(&format!(
            "{} ssh-ed25519 {}\n",
            "a".repeat(200),
            b64(&trusted)
        ));
        let files = [file("u", Origin::User, &text)];
        let lookup = "A".repeat(200);
        let start = std::time::Instant::now();
        let v = verify(&lookup, key(&trusted), &files);
        let t = start.elapsed();
        assert!(matches!(v, Verdict::Trusted { .. }), "{v:?}");
        assert!(
            known_key_types(&lookup, &files) == vec!["ssh-ed25519".to_owned()],
            "types"
        );
        assert!(t < std::time::Duration::from_secs(10), "took {t:?}");
    }

    #[test]
    fn lookup_is_lowercased_once_for_plain_but_not_for_hashed() {
        let k = ed25519(1);
        let files = [file(
            "u",
            Origin::User,
            &format!("Example.COM ssh-ed25519 {}\n", b64(&k)),
        )];
        assert!(matches!(
            verify("EXAMPLE.com", key(&k), &files),
            Verdict::Trusted { .. }
        ));
    }
}
