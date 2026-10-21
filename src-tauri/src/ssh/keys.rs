//! Identity files (§5.6 step 3, §9.4, §7.4.1): discovery of default identities, loading through
//! `platform::local_path` (64 KiB cap, owner/permission trust check on the opened handle),
//! public key extraction **without** decryption, encryption detection, and decryption.
//!
//! Decryption runs bcrypt-pbkdf (≈ 1 s for OpenSSH keys): [`decrypt`] is a plain blocking
//! function; async callers run it via `spawn_blocking`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use russh::keys::ssh_key;
use russh::keys::{HashAlg, PrivateKey, PublicKey};
use zeroize::Zeroizing;

use crate::error::{AppError, ErrorCode, KeyLoadReason};
use crate::ipc::dto::LocalKeyInfo;
use crate::platform::file_trust::FileKind;
use crate::platform::local_path::{LocalPathError, read_local};

/// OpenSSH default identity names, in `pubkey_prepare` order (only existing files are used).
pub const DEFAULT_IDENTITY_NAMES: &[&str] = &["id_rsa", "id_ecdsa", "id_ed25519"];

/// One identity file candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityFile {
    pub path: PathBuf,
    /// From config/bookmark (`true`) or a default identity (`false`).
    pub explicit: bool,
}

/// The default identities that exist in `user_ssh_dir` (`explicit = false`).
pub fn default_identity_files(user_ssh_dir: &Path) -> Vec<IdentityFile> {
    DEFAULT_IDENTITY_NAMES
        .iter()
        .map(|name| user_ssh_dir.join(name))
        .filter(|p| p.is_file())
        .map(|path| IdentityFile {
            path,
            explicit: false,
        })
        .collect()
}

/// `~`-abbreviated display form of `path` (for `SessionInfo.auth.keySource`, prompts, errors).
pub fn abbreviate_home(path: &Path, home: &Path) -> String {
    let shown = match path.strip_prefix(home) {
        Ok(rest) if !home.as_os_str().is_empty() => {
            format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display())
        }
        _ => path.display().to_string(),
    };
    native_separators(shown)
}

/// Config values like `~/.ssh/id_ed25519` keep their `/` after joining, which shows as
/// `~\.ssh/id_ed25519` on Windows. `/` is a separator there (never part of a name), so show
/// native separators throughout.
fn native_separators(shown: String) -> String {
    if cfg!(windows) {
        shown.replace('/', "\\")
    } else {
        shown
    }
}

/// Why a key file could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyLoadFailure {
    pub reason: KeyLoadReason,
    /// English developer detail (never key material).
    pub detail: String,
}

impl KeyLoadFailure {
    fn new(reason: KeyLoadReason, detail: impl Into<String>) -> Self {
        Self {
            reason,
            detail: detail.into(),
        }
    }

    /// `keyLoadFailed{path, reason}` with `detail`.
    pub fn to_app_error(&self, display_path: &str) -> AppError {
        AppError::new(ErrorCode::KeyLoadFailed {
            path: display_path.to_owned(),
            reason: self.reason,
        })
        .with_detail(self.detail.clone())
    }
}

fn map_local(e: LocalPathError) -> KeyLoadFailure {
    let detail = e.to_string();
    let reason = match e {
        LocalPathError::NotFound => KeyLoadReason::NotFound,
        LocalPathError::PermissionDenied => KeyLoadReason::PermissionDenied,
        LocalPathError::Insecure(_) => KeyLoadReason::InsecurePermissions,
        LocalPathError::UntrustedPath(_) => KeyLoadReason::UntrustedPath,
        LocalPathError::NotAFile => KeyLoadReason::NotAFile,
        LocalPathError::TooLarge(_) => KeyLoadReason::TooLarge,
        LocalPathError::Io(_) => KeyLoadReason::Corrupt,
    };
    KeyLoadFailure::new(reason, detail)
}

/// Text formats we recognise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    OpenSsh,
    /// PKCS#1 / SEC1 / PKCS#8 PEM.
    Pem,
    Ppk,
}

/// A loaded (not decrypted) private key file.
#[derive(Clone)]
pub struct KeyFile {
    pub path: PathBuf,
    /// `~`-abbreviated path.
    pub display: String,
    /// Public key, if obtainable without decryption (OpenSSH header, `.pub` sibling, or an
    /// unencrypted key).
    pub public: Option<PublicKey>,
    pub encrypted: bool,
    /// The key's KDF parameters exceed the caps ([`kdf_cost_error`]); English detail.
    /// [`decrypt`] refuses before asking for a passphrase or deriving anything.
    kdf_too_costly: Option<String>,
    text: Zeroizing<String>,
}

impl std::fmt::Debug for KeyFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyFile")
            .field("path", &self.path)
            .field("encrypted", &self.encrypted)
            .field(
                "public",
                &self
                    .public
                    .as_ref()
                    .map(|k| k.fingerprint(HashAlg::Sha256).to_string()),
            )
            .finish_non_exhaustive()
    }
}

impl KeyFile {
    /// `keyLoadFailed{unsupported}` when the encrypted key's KDF parameters exceed the
    /// caps; checked before a passphrase prompt and before any derivation.
    pub fn kdf_refusal(&self) -> Option<KeyLoadFailure> {
        let detail = self.kdf_too_costly.as_ref().filter(|_| self.encrypted)?;
        Some(KeyLoadFailure::new(
            KeyLoadReason::Unsupported,
            detail.clone(),
        ))
    }

    /// SHA-256 fingerprint of the public key, if known.
    pub fn fingerprint(&self) -> Option<String> {
        self.public
            .as_ref()
            .map(|k| k.fingerprint(HashAlg::Sha256).to_string())
    }
}

/// Format marker, not key material.
const OPENSSH_HEADER: &str = "-----BEGIN OPENSSH PRIVATE KEY-----"; // gitleaks:allow

fn detect_format(text: &str) -> Option<Format> {
    let trimmed = text.trim_start();
    if trimmed.starts_with("PuTTY-User-Key-File-") {
        return Some(Format::Ppk);
    }
    if trimmed.lines().any(|l| l.trim() == OPENSSH_HEADER) {
        return Some(Format::OpenSsh);
    }
    let pem = trimmed.lines().any(|l| {
        let l = l.trim();
        l.starts_with("-----BEGIN ") && l.ends_with("PRIVATE KEY-----")
    });
    pem.then_some(Format::Pem)
}

fn pem_is_encrypted(text: &str) -> bool {
    text.lines().any(|l| {
        let l = l.trim();
        l == "Proc-Type: 4,ENCRYPTED" || l == "-----BEGIN ENCRYPTED PRIVATE KEY-----"
    })
}

fn ppk_is_encrypted(text: &str) -> bool {
    text.lines()
        .filter_map(|l| l.strip_prefix("Encryption:"))
        .any(|v| !v.trim().eq_ignore_ascii_case("none"))
}

/// Public key from the `<path>.pub` sibling (read through the trust check; failures ignored).
fn sibling_public_key(path: &Path) -> Option<PublicKey> {
    let mut pub_path = path.as_os_str().to_owned();
    pub_path.push(".pub");
    let bytes = read_local(Path::new(&pub_path), FileKind::PublicKey).ok()?;
    let text = std::str::from_utf8(&bytes).ok()?;
    PublicKey::from_openssh(text.trim()).ok()
}

fn unsupported_or_corrupt(e: &russh::keys::Error) -> KeyLoadReason {
    match e {
        russh::keys::Error::UnsupportedKeyType { .. } | russh::keys::Error::UnknownAlgorithm(_) => {
            KeyLoadReason::Unsupported
        }
        russh::keys::Error::SshKey(
            ssh_key::Error::AlgorithmUnknown | ssh_key::Error::AlgorithmUnsupported { .. },
        ) => KeyLoadReason::Unsupported,
        _ => KeyLoadReason::Corrupt,
    }
}

/// Read a private key file through `local_path` (blocking) and extract what is available
/// without decrypting it.
pub fn read_key_file(path: &Path, home: &Path) -> Result<KeyFile, KeyLoadFailure> {
    let bytes = read_local(path, FileKind::PrivateKey).map_err(map_local)?;
    // The buffer moves into the zeroizing String; on a UTF-8 error it is zeroized as well.
    let text = Zeroizing::new(String::from_utf8(bytes).map_err(|e| {
        drop(Zeroizing::new(e.into_bytes()));
        KeyLoadFailure::new(KeyLoadReason::Corrupt, "key file is not UTF-8 text")
    })?);
    let format = detect_format(&text).ok_or_else(|| {
        KeyLoadFailure::new(KeyLoadReason::Unsupported, "unrecognised key file format")
    })?;
    let mut kdf_too_costly = None;
    let (encrypted, public) = match format {
        Format::OpenSsh => {
            let key = ssh_key::PrivateKey::from_openssh(text.as_bytes()).map_err(|e| {
                let reason = match e {
                    ssh_key::Error::AlgorithmUnknown
                    | ssh_key::Error::AlgorithmUnsupported { .. } => KeyLoadReason::Unsupported,
                    _ => KeyLoadReason::Corrupt,
                };
                KeyLoadFailure::new(reason, e.to_string())
            })?;
            if let ssh_key::Kdf::Bcrypt { rounds, .. } = key.kdf() {
                kdf_too_costly = bcrypt_cost_error(*rounds);
            }
            (key.is_encrypted(), Some(key.public_key().clone()))
        }
        Format::Pem | Format::Ppk => {
            let encrypted = if format == Format::Pem {
                pem_is_encrypted(&text)
            } else {
                ppk_is_encrypted(&text)
            };
            if encrypted {
                kdf_too_costly = if format == Format::Pem {
                    pkcs8_cost_error(&text)
                } else {
                    ppk_cost_error(&text)
                };
            }
            let public = if encrypted {
                None
            } else {
                let key = russh::keys::decode_secret_key(&text, None)
                    .map_err(|e| KeyLoadFailure::new(unsupported_or_corrupt(&e), e.to_string()))?;
                Some(key.public_key().clone())
            };
            (encrypted, public)
        }
    };
    let public = public.or_else(|| sibling_public_key(path));
    Ok(KeyFile {
        path: path.to_path_buf(),
        display: abbreviate_home(path, home),
        public,
        encrypted,
        kdf_too_costly,
        text,
    })
}

// ---------------------------------------------------------------------------------------------
// KDF cost caps. A key file's header chooses how much CPU time and memory deriving the
// decryption key takes; a crafted file could pin a blocking thread for hours (bcrypt rounds,
// PBKDF2 iterations) or make Argon2/scrypt allocate terabytes (an allocation failure aborts the
// process). The caps sit far above what the usual tools write, so real keys are unaffected.
// ---------------------------------------------------------------------------------------------

/// OpenSSH `bcrypt` rounds. `ssh-keygen` writes 16 by default; `-a 100` is the common "strong"
/// advice and even `-a 1000` (≈ 10 s per unlock) is rare. 1024 rounds ≈ 10 s on a desktop CPU.
pub const MAX_BCRYPT_ROUNDS: u32 = 1024;
/// PuTTY `.ppk` v3 Argon2 memory in KiB. PuTTYgen writes 8192 (8 MiB) by default.
pub const MAX_ARGON2_MEMORY_KIB: u64 = 256 * 1024;
/// PuTTY `.ppk` v3 Argon2 passes. PuTTYgen calibrates to ≈ 100 ms, typically 10–50 passes.
pub const MAX_ARGON2_PASSES: u64 = 1000;
pub const MAX_ARGON2_PARALLELISM: u64 = 64;
/// Argon2 memory (KiB) × passes: 16 GiB of block passes, a few seconds (default: 8 MiB × ~34).
pub const MAX_ARGON2_WORK: u64 = 16 * 1024 * 1024;
/// Encrypted PKCS#8, PBKDF2 iterations (OpenSSL writes 2048; OWASP suggests 600 000).
pub const MAX_PBKDF2_ITERATIONS: u64 = 10_000_000;
/// Encrypted PKCS#8, scrypt memory `128 · r · N` bytes (OpenSSL default N = 16384, r = 8: 16 MiB).
pub const MAX_SCRYPT_MEMORY: u64 = 256 * 1024 * 1024;
/// scrypt work `128 · r · N · p`.
pub const MAX_SCRYPT_WORK: u64 = 4 * 1024 * 1024 * 1024;

fn bcrypt_cost_error(rounds: u32) -> Option<String> {
    (rounds > MAX_BCRYPT_ROUNDS)
        .then(|| format!("bcrypt KDF rounds {rounds} exceed the limit {MAX_BCRYPT_ROUNDS}"))
}

/// `.ppk` v3: the `Argon2-*` header values (missing or non-numeric values are left to the key
/// parser, which rejects them).
fn ppk_cost_error(text: &str) -> Option<String> {
    // Digits only; a value too large for u64 is over every cap.
    let value = |name: &str| -> Option<u64> {
        let v = text
            .lines()
            .find_map(|l| l.strip_prefix(name)?.strip_prefix(':'))?
            .trim();
        (!v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
            .then(|| v.parse::<u64>().unwrap_or(u64::MAX))
    };
    let memory = value("Argon2-Memory");
    let passes = value("Argon2-Passes");
    let lanes = value("Argon2-Parallelism");
    let over = |v: Option<u64>, max: u64| v.is_some_and(|v| v > max);
    let work = memory.zip(passes).map(|(m, p)| m.saturating_mul(p));
    if over(memory, MAX_ARGON2_MEMORY_KIB)
        || over(passes, MAX_ARGON2_PASSES)
        || over(lanes, MAX_ARGON2_PARALLELISM)
        || over(work, MAX_ARGON2_WORK)
    {
        return Some(format!(
            "Argon2 KDF parameters (memory {} KiB, passes {}, parallelism {}) exceed the limits",
            memory.unwrap_or(0),
            passes.unwrap_or(0),
            lanes.unwrap_or(0)
        ));
    }
    None
}

/// DER OIDs (tag + length + value) of the PKCS#5 KDFs.
const OID_PBKDF2: &[u8] = &[
    0x06, 0x09, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x05, 0x0C,
];
const OID_SCRYPT: &[u8] = &[
    0x06, 0x09, 0x2B, 0x06, 0x01, 0x04, 0x01, 0xDA, 0x47, 0x04, 0x0B,
];

/// One DER TLV at the start of `der`: `(tag, value, rest)`. Definite lengths up to 4 bytes.
fn der_tlv(der: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = der.split_first()?;
    let (&first, mut rest) = rest.split_first()?;
    let len = if first < 0x80 {
        usize::from(first)
    } else {
        let n = usize::from(first & 0x7F);
        if n == 0 || n > 4 {
            return None;
        }
        let (bytes, after) = rest.split_at_checked(n)?;
        rest = after;
        bytes
            .iter()
            .fold(0usize, |acc, b| (acc << 8) | usize::from(*b))
    };
    let (value, rest) = rest.split_at_checked(len)?;
    Some((tag, value, rest))
}

/// A DER INTEGER as u64; `u64::MAX` for anything larger (it is over every cap anyway).
fn der_uint(der: &[u8]) -> Option<(u64, &[u8])> {
    let (tag, value, rest) = der_tlv(der)?;
    if tag != 0x02 {
        return None;
    }
    let digits = value.iter().skip_while(|b| **b == 0).count();
    let n = if digits > 8 {
        u64::MAX
    } else {
        value.iter().fold(0u64, |acc, b| (acc << 8) | u64::from(*b))
    };
    Some((n, rest))
}

/// The parameters SEQUENCE right after `oid` in `der`, minus its leading salt OCTET STRING.
fn kdf_params_after<'a>(der: &'a [u8], oid: &[u8]) -> Option<&'a [u8]> {
    let at = der.windows(oid.len()).position(|w| w == oid)?;
    let after = der.get(at.checked_add(oid.len())?..)?;
    let (tag, params, _) = der_tlv(after)?;
    if tag != 0x30 {
        return None;
    }
    let (salt_tag, _, rest) = der_tlv(params)?;
    (salt_tag == 0x04).then_some(rest)
}

/// Encrypted PKCS#8 (`BEGIN ENCRYPTED PRIVATE KEY`): PBKDF2 iterations and scrypt N/r/p.
/// Legacy `Proc-Type: 4,ENCRYPTED` PEM uses one MD5 round and needs no cap.
fn pkcs8_cost_error(text: &str) -> Option<String> {
    use base64::Engine as _;
    let body: String = text
        .lines()
        .map(str::trim)
        .skip_while(|l| !(l.starts_with("-----BEGIN ") && l.contains("ENCRYPTED PRIVATE KEY")))
        .skip(1)
        .take_while(|l| !l.starts_with("-----END "))
        .collect();
    let der = Zeroizing::new(
        base64::engine::general_purpose::STANDARD
            .decode(body.as_bytes())
            .ok()?,
    );
    if let Some(params) = kdf_params_after(&der, OID_PBKDF2) {
        let (iterations, _) = der_uint(params)?;
        if iterations > MAX_PBKDF2_ITERATIONS {
            return Some(format!(
                "PBKDF2 iterations {iterations} exceed the limit {MAX_PBKDF2_ITERATIONS}"
            ));
        }
    }
    if let Some(params) = kdf_params_after(&der, OID_SCRYPT) {
        let (n, rest) = der_uint(params)?;
        let (r, rest) = der_uint(rest)?;
        let (p, _) = der_uint(rest)?;
        let memory = n.saturating_mul(r).saturating_mul(128);
        if memory > MAX_SCRYPT_MEMORY || memory.saturating_mul(p) > MAX_SCRYPT_WORK {
            return Some(format!(
                "scrypt parameters (N {n}, r {r}, p {p}) exceed the limits"
            ));
        }
    }
    None
}

/// Result of [`decrypt`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecryptError {
    /// The key is encrypted and no passphrase was given.
    NeedsPassphrase,
    WrongPassphrase,
    Failed(KeyLoadFailure),
}

/// Decrypt/decode the private key (blocking; bcrypt-pbkdf for OpenSSH keys).
pub fn decrypt(key: &KeyFile, passphrase: Option<&str>) -> Result<PrivateKey, DecryptError> {
    if let Some(failure) = key.kdf_refusal() {
        return Err(DecryptError::Failed(failure));
    }
    if key.encrypted && passphrase.is_none() {
        return Err(DecryptError::NeedsPassphrase);
    }
    let pass = if key.encrypted { passphrase } else { None };
    match russh::keys::decode_secret_key(&key.text, pass) {
        Ok(k) => Ok(k),
        Err(russh::keys::Error::KeyIsEncrypted) => Err(DecryptError::NeedsPassphrase),
        Err(e) => {
            let reason = unsupported_or_corrupt(&e);
            if key.encrypted && reason != KeyLoadReason::Unsupported {
                // OpenSSH: SshKey(Crypto) (App. A #5); PEM/PKCS#8/PPK: padding/DER/integrity errors.
                Err(DecryptError::WrongPassphrase)
            } else {
                Err(DecryptError::Failed(KeyLoadFailure::new(
                    reason,
                    e.to_string(),
                )))
            }
        }
    }
}

fn is_key_candidate(dir: &Path, name: &str) -> bool {
    if name.ends_with(".pub") {
        return false;
    }
    if name.starts_with("id_") {
        return true;
    }
    dir.join(format!("{name}.pub")).is_file()
}

/// Candidate private key files of `user_ssh_dir`, sorted (the set `keys_list` shows).
fn key_candidates(user_ssh_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(user_ssh_dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file() || t.is_symlink()))
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| is_key_candidate(user_ssh_dir, n))
        })
        .map(|e| e.path())
        .collect();
    paths.sort();
    paths
}

/// `keys_read_public`: the one-line OpenSSH public key of a key that `keys_list`
/// shows. `path` must be exactly one of the listed paths (the webview cannot read arbitrary
/// files). The `.pub` sibling is read through the trust check (64 KiB cap, then 16 KiB for the
/// text) and must parse as a public key; without a usable `.pub`, the public half derived from
/// the private key (OpenSSH header / unencrypted key) is used. Never decrypts.
pub fn read_public(user_ssh_dir: &Path, home: &Path, path: &str) -> Result<String, AppError> {
    /// A public key line is a few hundred bytes; more is not a public key.
    const MAX_PUB_BYTES: usize = 16 * 1024;
    let listed = key_candidates(user_ssh_dir)
        .into_iter()
        .find(|p| p.display().to_string() == path)
        .ok_or_else(|| AppError::invalid_argument("path"))?;
    let from_pub = {
        let mut pub_path = listed.as_os_str().to_owned();
        pub_path.push(".pub");
        read_local(Path::new(&pub_path), FileKind::PublicKey)
            .ok()
            .filter(|b| b.len() <= MAX_PUB_BYTES)
            .and_then(|b| String::from_utf8(b).ok())
            .and_then(|t| {
                let line = t.lines().map(str::trim).find(|l| !l.is_empty())?;
                PublicKey::from_openssh(line).ok()
            })
    };
    let public = match from_pub {
        Some(k) => Some(k),
        None => {
            read_key_file(&listed, home)
                .map_err(|e| e.to_app_error(&abbreviate_home(&listed, home)))?
                .public
        }
    };
    let public = public.ok_or_else(|| {
        AppError::new(ErrorCode::KeyLoadFailed {
            path: abbreviate_home(&listed, home),
            reason: KeyLoadReason::NotFound,
        })
        .with_detail("no .pub file and the private key is encrypted")
    })?;
    public
        .to_openssh()
        .map_err(|e| AppError::internal(e.to_string()))
}

/// `keys_list`: private keys in `~/.ssh` (every regular file named `id_*` without `.pub`, plus
/// every `X` for which `X.pub` exists). Never decrypts. Sorted by path.
pub fn local_keys(
    user_ssh_dir: &Path,
    home: &Path,
    agent_fingerprints: &HashSet<String>,
) -> Vec<LocalKeyInfo> {
    key_candidates(user_ssh_dir)
        .into_iter()
        .map(|path| {
            let shown = path.display().to_string();
            match read_key_file(&path, home) {
                Ok(k) => {
                    let fingerprint = k.fingerprint();
                    LocalKeyInfo {
                        path: shown,
                        key_type: k.public.as_ref().map(|p| p.algorithm().to_string()),
                        in_agent: fingerprint
                            .as_ref()
                            .is_some_and(|f| agent_fingerprints.contains(f)),
                        fingerprint,
                        encrypted: k.encrypted,
                        error: None,
                    }
                }
                Err(e) => LocalKeyInfo {
                    key_type: None,
                    fingerprint: None,
                    encrypted: false,
                    in_agent: false,
                    error: Some(e.to_app_error(&abbreviate_home(&path, home))),
                    path: shown,
                },
            }
        })
        .collect()
}

/// Throwaway test keys (generated with `ssh-keygen -C shellfin-test`; never used anywhere else).
#[cfg(test)]
pub(crate) mod fixtures {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::path::{Path, PathBuf};

    pub const PASSPHRASE: &str = "test-passphrase";

    // Test-only keys live under tests/fixtures/keys/ (secret-scanner allowlisted).
    pub const ED_PLAIN: &str = include_str!("../../tests/fixtures/keys/unit/ed25519");
    pub const ED_PLAIN_PUB: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEp9asy/7jt+pc9kmiua9cG4gB+vv9M4EC1iZHCYgXH2 shellfin-test";

    /// bcrypt rounds = 1 (fast in debug tests).
    pub const ED_ENC: &str = include_str!("../../tests/fixtures/keys/unit/ed25519_enc_rounds1");
    pub const ED_ENC_PUB: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIMio1UExiuLsjaSa6Xv8q6QON3r1WsimEPnRqow67SYD shellfin-test";

    pub const RSA_OPENSSH: &str = include_str!("../../tests/fixtures/keys/unit/rsa_openssh");

    pub const RSA_PEM: &str = include_str!("../../tests/fixtures/keys/unit/rsa_pem");

    pub const RSA_PEM_ENC: &str = include_str!("../../tests/fixtures/keys/unit/rsa_pem_enc");
    pub const RSA_PEM_ENC_PUB: &str = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQCPd+pCt4xZSH+rRafVJba4HzkTVefkR89t9/BAEbuhwm3h4Et06GHvFc0wy87U5HzTY6Vx5ncn+w4tKoZf2i0NbZdAZM1rq/YA7/1DcRujjfcFg7YPvdCnv/WtTB5W32AhsgBjFHBUKGnFm88/9RtS+lsLB3sgOYLnvMfrEbN00UoRwm4CxP5CVn48TgAnWYC59/Ut0gGEBvuDvAUcIRiUp6KexYbaXyBn9IHQE82tkMq0XJgiWkigY5J9He5Vw/peswvFjXm99m4nm5wUMgAcJuFZO0taI8OORm0apGhj1pDtQYedPnFqwVnVrkLzcU8ZQ4+lIIjoDVI/7FzDkbS/ shellfin-test";

    pub const ED_AGENT_PUB: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIITdnR0c9ByM1MQn03r29rYQSyW56naywCENe4MyKoPj shellfin-test";

    /// Write `text` to `dir/name` with key-file permissions (Unix 0600; Windows: the dir is private).
    pub fn write_key(dir: &Path, name: &str, text: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, text).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        p
    }

    pub fn public(line: &str) -> russh::keys::PublicKey {
        russh::keys::PublicKey::from_openssh(line).unwrap()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::fixtures::*;
    use super::*;
    use crate::platform::test_support::private_tempdir;

    /// The scanner allowlist trusts the fixture DIRECTORY, so
    /// the content is checked here (runs in every CI unit job). Every file in
    /// `tests/fixtures/keys/` (and `unit/`) is the README, a `.pub` with the comment
    /// `shellfin-test`, or a private key that decodes with a known TEST passphrase to exactly
    /// the key in its sibling `.pub`, carrying the embedded comment `shellfin-test` (OpenSSH
    /// format) or one of the pinned PEM fingerprints. A real key dropped there fails this test.
    #[test]
    fn fixture_keys_are_all_test_keys() {
        const PEM_FIXTURE_FINGERPRINTS: &[&str] = &[
            "SHA256:qoZNcKBmNLVMPb5vvkXMkwGDwbLrNmZI2mPKCB+s2mU", // id_rsa_pem
            "SHA256:oZlT5sHY1pDHl74ao7KDWVMr0RgEqCkJbXtUoIeXqcM", // unit/rsa_pem
            "SHA256:QQxSKIBaFDrx1ySz1o/os5wdLdvmAOcCtpdmC/iGdnM", // unit/rsa_pem_enc
        ];
        const TEST_PASSPHRASES: &[Option<&str>] = &[
            None,
            Some(PASSPHRASE),                     // unit tests (this module)
            Some("correct-horse-battery-staple"), // integration suite (tests/support)
        ];
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/keys");
        let mut private_keys = 0;
        for dir in [root.clone(), root.join("unit")] {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                if path.is_dir() {
                    assert_eq!(path, root.join("unit"), "unexpected directory {name}");
                    continue;
                }
                if name == "README.md" && dir == root {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                if name.ends_with(".pub") {
                    let fields: Vec<&str> = text.split_whitespace().collect();
                    assert_eq!(fields.len(), 3, "{name}: one key line with a comment");
                    assert_eq!(fields[2], "shellfin-test", "{name}: comment");
                    let private = path.with_extension("");
                    assert!(private.is_file(), "{name}: no matching private key");
                    continue;
                }
                let pub_path = dir.join(format!("{name}.pub"));
                let expected = std::fs::read_to_string(&pub_path)
                    .unwrap_or_else(|_| panic!("{name}: missing {name}.pub (test keys only)"));
                let expected = PublicKey::from_openssh(expected.trim()).unwrap();
                let key = TEST_PASSPHRASES
                    .iter()
                    .find_map(|p| russh::keys::decode_secret_key(&text, *p).ok())
                    .unwrap_or_else(|| {
                        panic!("{name}: not a test key (no test passphrase opens it)")
                    });
                assert_eq!(
                    key.public_key().key_data(),
                    expected.key_data(),
                    "{name}: does not match {name}.pub"
                );
                // The `.pub` comment can be forged, so check the key itself: OpenSSH keys embed
                // their comment; PEM keys have none and are pinned by fingerprint.
                if text.trim_start().starts_with(OPENSSH_HEADER) {
                    assert_eq!(
                        key.comment().as_bytes(),
                        b"shellfin-test",
                        "{name}: embedded comment"
                    );
                } else {
                    let fp = key.public_key().fingerprint(HashAlg::Sha256).to_string();
                    assert!(
                        PEM_FIXTURE_FINGERPRINTS.contains(&fp.as_str()),
                        "{name}: unknown PEM key {fp}"
                    );
                }
                private_keys += 1;
            }
        }
        assert!(private_keys >= 17, "found only {private_keys} private keys");
    }

    #[test]
    fn defaults_only_existing_in_openssh_order() {
        let dir = private_tempdir();
        write_key(dir.path(), "id_ed25519", ED_PLAIN);
        write_key(dir.path(), "id_rsa", RSA_OPENSSH);
        let d = default_identity_files(dir.path());
        let names: Vec<_> = d
            .iter()
            .map(|f| f.path.file_name().unwrap().to_str().unwrap().to_owned())
            .collect();
        assert_eq!(names, ["id_rsa", "id_ed25519"]);
        assert!(d.iter().all(|f| !f.explicit));
    }

    #[test]
    fn read_public_only_for_listed_keys_and_validates() {
        let dir = private_tempdir();
        let home = dir.path();
        let key = write_key(dir.path(), "id_ed25519", ED_PLAIN);
        let shown = key.display().to_string();
        // No .pub: derived from the unencrypted private key.
        let derived = read_public(dir.path(), home, &shown).unwrap();
        assert!(derived.starts_with("ssh-ed25519 "), "{derived}");
        // With a .pub: its text (comment kept).
        std::fs::write(
            dir.path().join("id_ed25519.pub"),
            format!(
                "{ED_PLAIN_PUB}
"
            ),
        )
        .unwrap();
        assert_eq!(read_public(dir.path(), home, &shown).unwrap(), ED_PLAIN_PUB);
        // Not a listed key: refused, whatever it is.
        std::fs::write(dir.path().join("notes.txt"), "x").unwrap();
        for bad in [
            dir.path().join("notes.txt").display().to_string(),
            format!("{shown}.pub"),
            format!("{}/../id_ed25519", dir.path().display()),
            "/etc/passwd".to_owned(),
        ] {
            let err = read_public(dir.path(), home, &bad).unwrap_err();
            assert!(
                matches!(err.kind, ErrorCode::InvalidArgument { .. }),
                "{bad}"
            );
        }
        // Encrypted key without a usable .pub: keyLoadFailed, no decryption.
        write_key(dir.path(), "id_rsa", RSA_PEM_ENC);
        let enc = dir.path().join("id_rsa").display().to_string();
        let err = read_public(dir.path(), home, &enc).unwrap_err();
        assert!(
            matches!(err.kind, ErrorCode::KeyLoadFailed { .. }),
            "{err:?}"
        );
        // A garbage .pub falls back to the private key / fails the same way.
        std::fs::write(dir.path().join("id_rsa.pub"), "not a key").unwrap();
        assert!(read_public(dir.path(), home, &enc).is_err());
    }

    #[test]
    fn abbreviation() {
        let home = Path::new(if cfg!(windows) {
            r"C:\Users\alice"
        } else {
            "/home/alice"
        });
        let p = home.join(".ssh").join("id_ed25519");
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(
            abbreviate_home(&p, home),
            format!("~{sep}.ssh{sep}id_ed25519")
        );
        assert_eq!(
            abbreviate_home(Path::new("/elsewhere/k"), home),
            format!("{sep}elsewhere{sep}k")
        );
        // A config value joined onto the home keeps its `/` — shown natively.
        assert_eq!(
            abbreviate_home(&home.join(".ssh/id_ed25519_enc"), home),
            format!("~{sep}.ssh{sep}id_ed25519_enc")
        );
    }

    #[test]
    fn formats_public_keys_and_encryption() {
        let dir = private_tempdir();
        let home = dir.path();
        let ed = read_key_file(&write_key(home, "ed", ED_PLAIN), home).unwrap();
        assert!(!ed.encrypted);
        assert_eq!(
            ed.public.as_ref().unwrap().key_data(),
            public(ED_PLAIN_PUB).key_data()
        );
        let enc = read_key_file(&write_key(home, "ed_enc", ED_ENC), home).unwrap();
        assert!(enc.encrypted);
        // OpenSSH format: public key available before decryption.
        assert_eq!(
            enc.public.as_ref().unwrap().key_data(),
            public(ED_ENC_PUB).key_data()
        );
        let rsa = read_key_file(&write_key(home, "rsa", RSA_OPENSSH), home).unwrap();
        assert_eq!(
            rsa.public.as_ref().unwrap().algorithm().to_string(),
            "ssh-rsa"
        );
        let pem = read_key_file(&write_key(home, "pem", RSA_PEM), home).unwrap();
        assert!(!pem.encrypted && pem.public.is_some());
        // Encrypted PEM: no public key without the .pub sibling …
        let pem_enc_path = write_key(home, "pem_enc", RSA_PEM_ENC);
        let pem_enc = read_key_file(&pem_enc_path, home).unwrap();
        assert!(pem_enc.encrypted);
        assert!(pem_enc.public.is_none());
        // … and with it.
        std::fs::write(home.join("pem_enc.pub"), RSA_PEM_ENC_PUB).unwrap();
        let pem_enc = read_key_file(&pem_enc_path, home).unwrap();
        assert_eq!(
            pem_enc.public.as_ref().unwrap().key_data(),
            public(RSA_PEM_ENC_PUB).key_data()
        );
        assert!(pem_enc.display.starts_with('~'));
    }

    #[test]
    fn decrypt_outcomes() {
        let dir = private_tempdir();
        let home = dir.path();
        let enc = read_key_file(&write_key(home, "ed_enc", ED_ENC), home).unwrap();
        assert_eq!(
            decrypt(&enc, None).unwrap_err(),
            DecryptError::NeedsPassphrase
        );
        assert_eq!(
            decrypt(&enc, Some("wrong")).unwrap_err(),
            DecryptError::WrongPassphrase
        );
        let k = decrypt(&enc, Some(PASSPHRASE)).unwrap();
        assert_eq!(k.public_key().key_data(), public(ED_ENC_PUB).key_data());
        let pem_enc = read_key_file(&write_key(home, "pem_enc", RSA_PEM_ENC), home).unwrap();
        assert_eq!(
            decrypt(&pem_enc, None).unwrap_err(),
            DecryptError::NeedsPassphrase
        );
        assert_eq!(
            decrypt(&pem_enc, Some("wrong")).unwrap_err(),
            DecryptError::WrongPassphrase
        );
        let k = decrypt(&pem_enc, Some(PASSPHRASE)).unwrap();
        assert_eq!(
            k.public_key().key_data(),
            public(RSA_PEM_ENC_PUB).key_data()
        );
        let plain = read_key_file(&write_key(home, "ed", ED_PLAIN), home).unwrap();
        // A passphrase given for an unencrypted key is ignored.
        assert!(decrypt(&plain, Some("whatever")).is_ok());
    }

    /// ED_ENC with its bcrypt rounds replaced (the kdfoptions field of the openssh-key-v1 blob).
    fn openssh_with_rounds(rounds: u32) -> String {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let lines: Vec<&str> = ED_ENC.lines().collect();
        let body: String = lines[1..lines.len() - 1].concat();
        let mut blob = b64.decode(body).unwrap();
        let at = blob.windows(6).position(|w| w == b"bcrypt").unwrap() + 6;
        let salt_len = u32::from_be_bytes(blob[at + 4..at + 8].try_into().unwrap()) as usize;
        let r = at + 8 + salt_len;
        blob[r..r + 4].copy_from_slice(&rounds.to_be_bytes());
        let enc = b64.encode(blob);
        let wrapped: Vec<String> = enc
            .as_bytes()
            .chunks(70)
            .map(|c| String::from_utf8(c.to_vec()).unwrap())
            .collect();
        format!(
            "{}\n{}\n{}\n",
            lines[0],
            wrapped.join("\n"),
            lines[lines.len() - 1]
        )
    }

    /// Hostile bcrypt rounds are refused before any prompt or derivation.
    #[test]
    fn openssh_bcrypt_rounds_are_capped() {
        let dir = private_tempdir();
        let home = dir.path();
        let at_cap = openssh_with_rounds(MAX_BCRYPT_ROUNDS);
        let kf = read_key_file(&write_key(home, "cap", &at_cap), home).unwrap();
        assert!(kf.encrypted && kf.kdf_refusal().is_none());
        for rounds in [MAX_BCRYPT_ROUNDS + 1, u32::MAX] {
            let text = openssh_with_rounds(rounds);
            let kf = read_key_file(&write_key(home, "hostile", &text), home).unwrap();
            assert!(kf.public.is_some(), "public half still listed");
            let refusal = kf.kdf_refusal().unwrap();
            assert_eq!(refusal.reason, KeyLoadReason::Unsupported);
            let start = std::time::Instant::now();
            match decrypt(&kf, Some(PASSPHRASE)) {
                Err(DecryptError::Failed(f)) => assert_eq!(f.reason, KeyLoadReason::Unsupported),
                other => panic!("{other:?}"),
            }
            assert_eq!(
                decrypt(&kf, None).unwrap_err(),
                DecryptError::Failed(refusal)
            );
            assert!(start.elapsed() < std::time::Duration::from_secs(1));
        }
    }

    fn ppk(memory: &str, passes: &str, lanes: &str) -> String {
        format!(
            "PuTTY-User-Key-File-3: ssh-ed25519\nEncryption: aes256-cbc\nComment: shellfin-test\n\
             Public-Lines: 1\nAAAAC3NzaC1lZDI1NTE5AAAAIEp9asy/7jt+pc9kmiua9cG4gB+vv9M4EC1iZHCYgXH2\n\
             Key-Derivation: Argon2id\nArgon2-Memory: {memory}\nArgon2-Passes: {passes}\n\
             Argon2-Parallelism: {lanes}\nArgon2-Salt: 63d1d43f7bf7700720496646a2f5ec17\n\
             Private-Lines: 1\nAAAA\nPrivate-MAC: 00\n"
        )
    }

    /// `.ppk` v3 Argon2 memory/passes/parallelism caps (a hostile memory value would
    /// otherwise make Argon2 allocate terabytes, which aborts the process).
    #[test]
    fn ppk_argon2_parameters_are_capped() {
        assert_eq!(
            ppk_cost_error(&ppk("8192", "34", "1")),
            None,
            "PuTTYgen default"
        );
        assert_eq!(
            ppk_cost_error(&ppk("262144", "64", "4")),
            None,
            "256 MiB × 64"
        );
        for hostile in [
            ppk("4294967295", "1", "1"),
            ppk("262145", "1", "1"),
            ppk("8192", "1001", "1"),
            ppk("8192", "34", "65"),
            ppk("262144", "65", "1"),
            ppk("99999999999999999999999999", "1", "1"),
        ] {
            assert!(ppk_cost_error(&hostile).is_some(), "{hostile}");
        }
        // Garbage values are left to the key parser.
        assert_eq!(ppk_cost_error(&ppk("-1", "x", "")), None);

        let dir = private_tempdir();
        let home = dir.path();
        let path = write_key(home, "hostile.ppk", &ppk("4294967295", "1", "1"));
        let kf = read_key_file(&path, home).unwrap();
        assert!(kf.encrypted);
        match decrypt(&kf, Some("x")) {
            Err(DecryptError::Failed(f)) => assert_eq!(f.reason, KeyLoadReason::Unsupported),
            other => panic!("{other:?}"),
        }
    }

    fn der(tag: u8, value: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        if value.len() < 0x80 {
            out.push(value.len() as u8);
        } else {
            out.push(0x82);
            out.extend_from_slice(&(value.len() as u16).to_be_bytes());
        }
        out.extend_from_slice(value);
        out
    }

    fn der_int(v: u64) -> Vec<u8> {
        let mut bytes = v.to_be_bytes().to_vec();
        while bytes.len() > 1 && bytes[0] == 0 && bytes[1] < 0x80 {
            bytes.remove(0);
        }
        if bytes[0] >= 0x80 {
            bytes.insert(0, 0);
        }
        der(0x02, &bytes)
    }

    /// Minimal EncryptedPrivateKeyInfo-shaped DER: `SEQ { SEQ { oid, SEQ { salt, ints… } } }`.
    fn pkcs8(oid: &[u8], ints: &[Vec<u8>]) -> String {
        use base64::Engine as _;
        let mut params = der(0x04, &[7u8; 16]);
        for i in ints {
            params.extend_from_slice(i);
        }
        let mut alg = oid.to_vec();
        alg.extend(der(0x30, &params));
        let body = der(0x30, &der(0x30, &alg));
        let label = ["ENCRYPTED", "PRIVATE KEY"].join(" ");
        format!(
            "-----BEGIN {label}-----\n{}\n-----END {label}-----\n",
            base64::engine::general_purpose::STANDARD.encode(body)
        )
    }

    /// Encrypted PKCS#8 PBKDF2 iterations and scrypt N/r/p caps; malformed DER is
    /// left to the key parser (no panic).
    #[test]
    fn pkcs8_kdf_parameters_are_capped() {
        assert_eq!(pkcs8_cost_error(&pkcs8(OID_PBKDF2, &[der_int(2048)])), None);
        assert!(pkcs8_cost_error(&pkcs8(OID_PBKDF2, &[der_int(100_000_000)])).is_some());
        let huge = der(0x02, &[0x7F; 12]);
        assert!(pkcs8_cost_error(&pkcs8(OID_PBKDF2, std::slice::from_ref(&huge))).is_some());
        let scrypt = |n: u64, r: u64, p: u64| {
            pkcs8_cost_error(&pkcs8(OID_SCRYPT, &[der_int(n), der_int(r), der_int(p)]))
        };
        assert_eq!(scrypt(16384, 8, 1), None, "OpenSSL default");
        assert!(scrypt(1 << 31, 8, 1).is_some());
        assert!(scrypt(1 << 18, 8, 64).is_some(), "work cap");
        assert!(pkcs8_cost_error(&pkcs8(OID_SCRYPT, &[huge, der_int(1), der_int(1)])).is_some());
        // Truncated / garbage DER: no verdict, no panic.
        for text in [
            pkcs8(OID_SCRYPT, &[der_int(5)]),
            pkcs8(OID_PBKDF2, &[]),
            pkcs8(OID_PBKDF2, &[]).replace("MI", "!!"),
        ] {
            let _ = pkcs8_cost_error(&text);
        }
        assert_eq!(der_tlv(&[0x30, 0x85, 1, 1, 1, 1, 1]), None, "5-byte length");
        assert_eq!(der_tlv(&[0x30, 0x82, 0xFF]), None, "short length bytes");
        assert_eq!(der_tlv(&[0x30, 0x05, 1]), None, "value past the end");

        let dir = private_tempdir();
        let home = dir.path();
        let path = write_key(
            home,
            "p8",
            &pkcs8(OID_SCRYPT, &[der_int(1 << 31), der_int(8), der_int(1)]),
        );
        let kf = read_key_file(&path, home).unwrap();
        assert!(kf.encrypted && kf.kdf_refusal().is_some());
    }

    #[test]
    fn load_failures_map_to_reasons() {
        let dir = private_tempdir();
        let home = dir.path();
        let err = read_key_file(&home.join("missing"), home).unwrap_err();
        assert_eq!(err.reason, KeyLoadReason::NotFound);
        let garbage = write_key(home, "garbage", "not a key\n");
        assert_eq!(
            read_key_file(&garbage, home).unwrap_err().reason,
            KeyLoadReason::Unsupported
        );
        let broken = write_key(
            home,
            "broken",
            "-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA\n-----END OPENSSH PRIVATE KEY-----\n",
        );
        assert_eq!(
            read_key_file(&broken, home).unwrap_err().reason,
            KeyLoadReason::Corrupt
        );
        let big = write_key(home, "big", &"a".repeat(64 * 1024 + 1));
        assert_eq!(
            read_key_file(&big, home).unwrap_err().reason,
            KeyLoadReason::TooLarge
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let loose = write_key(home, "loose", ED_PLAIN);
            std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(
                read_key_file(&loose, home).unwrap_err().reason,
                KeyLoadReason::InsecurePermissions
            );
        }
        #[cfg(windows)]
        {
            let shared = write_key(home, "shared", ED_PLAIN);
            crate::platform::test_support::set_dacl(&shared, "D:P(A;;FA;;;WD)");
            assert_eq!(
                read_key_file(&shared, home).unwrap_err().reason,
                KeyLoadReason::InsecurePermissions
            );
        }
        let e = KeyLoadFailure::new(KeyLoadReason::Corrupt, "x").to_app_error("~/k");
        assert_eq!(
            e.kind,
            ErrorCode::KeyLoadFailed {
                path: "~/k".into(),
                reason: KeyLoadReason::Corrupt
            }
        );
    }

    #[test]
    fn local_keys_listing() {
        let dir = private_tempdir();
        let ssh = dir.path().join(".ssh");
        std::fs::create_dir(&ssh).unwrap();
        crate::platform::test_support::make_private(&ssh);
        write_key(&ssh, "id_ed25519", ED_PLAIN);
        std::fs::write(ssh.join("id_ed25519.pub"), ED_PLAIN_PUB).unwrap();
        write_key(&ssh, "work", ED_ENC);
        std::fs::write(ssh.join("work.pub"), ED_ENC_PUB).unwrap();
        write_key(&ssh, "id_bad", "nope");
        std::fs::write(ssh.join("config"), "Host x\n").unwrap();
        std::fs::write(ssh.join("known_hosts"), "").unwrap();
        let in_agent: HashSet<String> = [public(ED_PLAIN_PUB)
            .fingerprint(HashAlg::Sha256)
            .to_string()]
        .into();
        let keys = local_keys(&ssh, dir.path(), &in_agent);
        let names: Vec<_> = keys
            .iter()
            .map(|k| {
                Path::new(&k.path)
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(names, ["id_bad", "id_ed25519", "work"]);
        assert!(keys[0].error.is_some());
        assert!(
            keys[1].in_agent
                && !keys[1].encrypted
                && keys[1].key_type.as_deref() == Some("ssh-ed25519")
        );
        assert!(!keys[2].in_agent && keys[2].encrypted && keys[2].fingerprint.is_some());
        assert!(local_keys(&dir.path().join("absent"), dir.path(), &in_agent).is_empty());
    }
}
