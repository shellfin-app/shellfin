//! Docker sshd harness for the integration suite (§10.2). Use via `mod support;` from a
//! `tests/it_*.rs` file that starts with `#![cfg(feature = "it")]`.
//!
//! - `sshd()` starts `tests/sshd/compose.yml` once per test binary (project `shellfin-it`; the
//!   image is tagged with a hash of its inputs, so containers are reused across test binaries):
//!   `bastion` on 127.0.0.1:2222 and `target` (internal; reachable from bastion as `target:22`).
//!   With `SHELLFIN_IT_EXTERNAL=1` the stack is assumed to be running already (CI starts it in a
//!   separate step); `SHELLFIN_IT_HOST` / `SHELLFIN_IT_PORT` override the bastion address.
//! - `docker_exec*` run commands inside a service (needs the docker CLI).
//! - Fixture keys are copied into private temp dirs so the app's §7.4.1 trust checks accept them.
//! - `totp` implements RFC 6238 (HMAC-SHA1, 30 s) for the `tester2fa` user.
//!
//! Stop the stack: `docker compose -p shellfin-it -f src-tauri/tests/sshd/compose.yml down -v`.

// Test harness: spawns `docker`, panics on failure (clear test errors), and each test binary uses
// a different subset of these helpers.
#![allow(
    clippy::disallowed_methods,
    clippy::unwrap_used,
    clippy::expect_used,
    dead_code
)]

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------------------------
// Test-only credentials (also hard-coded in tests/sshd/setup-users.sh). Never real secrets.
// ---------------------------------------------------------------------------------------------

/// Password + pubkey (`id_rsa_openssh`, `id_rsa_pem`, `id_ed25519_enc`); login shell bash.
pub const USER: &str = "tester";
pub const USER_PASSWORD: &str = "tester-password";
/// `AuthenticationMethods publickey,keyboard-interactive`: key `id_ed25519`, then PAM
/// "Password:" (pam_unix) and "Verification code:" (google-authenticator TOTP).
pub const USER_2FA: &str = "tester2fa";
pub const USER_2FA_PASSWORD: &str = "tester2fa-password";
/// Base32 TOTP secret of `tester2fa`.
pub const TOTP_SECRET: &str = "JBSWY3DPEHPK3PXP";
/// Login shell fish (same keys as `tester` minus `id_rsa_pem`).
pub const USER_FISH: &str = "tester-fish";
pub const USER_FISH_PASSWORD: &str = "tester-fish-password";
/// Login shell tcsh.
pub const USER_TCSH: &str = "tester-tcsh";
pub const USER_TCSH_PASSWORD: &str = "tester-tcsh-password";
/// `MaxAuthTries 3` (server budget below the client's 6); same keys as `tester`.
pub const USER_LOW_TRIES: &str = "tester3";
pub const USER_LOW_TRIES_PASSWORD: &str = "tester3-password";

/// `ForceCommand internal-sftp`: any `exec` runs the SFTP server (waits on stdin); key
/// `id_rsa_openssh`.
pub const USER_SFTP_ONLY: &str = "tester-sftponly";

/// Passphrase of `id_ed25519_enc`.
pub const KEY_PASSPHRASE: &str = "correct-horse-battery-staple";
/// Encrypted ed25519 (OpenSSH format), authorized for tester/tester3/fish/tcsh.
pub const KEY_ED25519_ENC: &str = "id_ed25519_enc";
/// Unencrypted RSA 3072 (OpenSSH format), authorized for tester/tester3/fish/tcsh.
pub const KEY_RSA_OPENSSH: &str = "id_rsa_openssh";
/// Unencrypted RSA 3072 (PEM / PKCS#1), authorized for tester/tester3.
pub const KEY_RSA_PEM: &str = "id_rsa_pem";
/// Unencrypted ed25519, authorized for tester2fa only.
pub const KEY_ED25519: &str = "id_ed25519";
/// Unauthorized ed25519 keys for the agent budget test.
pub const AGENT_EXTRA_KEYS: [&str; 8] = [
    "agent_extra_1",
    "agent_extra_2",
    "agent_extra_3",
    "agent_extra_4",
    "agent_extra_5",
    "agent_extra_6",
    "agent_extra_7",
    "agent_extra_8",
];

/// Server banner text (`Banner /etc/ssh/banner.txt`).
pub const BANNER: &str = "Shellfin integration test server";

pub const COMPOSE_PROJECT: &str = "shellfin-it";
pub const BASTION: &str = "bastion";
pub const TARGET: &str = "target";

pub fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn fixture_dir() -> PathBuf {
    manifest_dir().join("tests").join("fixtures").join("keys")
}

pub fn compose_file() -> PathBuf {
    manifest_dir()
        .join("tests")
        .join("sshd")
        .join("compose.yml")
}

// ---------------------------------------------------------------------------------------------
// Compose stack
// ---------------------------------------------------------------------------------------------

/// Address of the published bastion sshd and the internal target.
#[derive(Debug, Clone)]
pub struct Sshd {
    pub host: String,
    pub port: u16,
    /// Host name of the target as seen from the bastion (ProxyJump / direct-tcpip).
    pub target_host: String,
    pub target_port: u16,
}

/// Hash of everything that goes into the image (tests/sshd/* and the fixture public keys), used
/// as the image tag: `up` then builds and recreates the containers only when those change
/// (a plain `up --build` produces a new image id and recreates the containers every time).
pub fn image_tag() -> String {
    let mut files: Vec<PathBuf> = Vec::new();
    for dir in [manifest_dir().join("tests").join("sshd"), fixture_dir()] {
        for e in std::fs::read_dir(&dir).expect("read image input dir") {
            let p = e.expect("dir entry").path();
            let is_input = dir.ends_with("sshd") || p.extension().is_some_and(|x| x == "pub");
            if p.is_file() && is_input {
                files.push(p);
            }
        }
    }
    files.sort();
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    for f in &files {
        ctx.update(f.file_name().unwrap().to_string_lossy().as_bytes());
        ctx.update(&[0]);
        // Normalise CRLF so a Windows checkout and Linux CI produce the same tag.
        let bytes: Vec<u8> = std::fs::read(f)
            .expect("read image input")
            .into_iter()
            .filter(|b| *b != b'\r')
            .collect();
        ctx.update(&bytes);
        ctx.update(&[0]);
    }
    ctx.finish().as_ref()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn compose() -> Command {
    let mut cmd = Command::new("docker");
    cmd.env("SHELLFIN_IT_IMAGE_TAG", image_tag());
    cmd.arg("compose")
        .arg("-p")
        .arg(COMPOSE_PROJECT)
        .arg("-f")
        .arg(compose_file());
    cmd
}

/// The running test servers; starts the compose stack on first use (once per test binary).
pub fn sshd() -> &'static Sshd {
    static SSHD: OnceLock<Sshd> = OnceLock::new();
    SSHD.get_or_init(|| {
        let external = std::env::var("SHELLFIN_IT_EXTERNAL").is_ok_and(|v| v == "1");
        if !external {
            let out = compose()
                .args(["up", "-d", "--wait"])
                .output()
                .expect("run `docker compose up` (is Docker running?)");
            assert!(
                out.status.success(),
                "docker compose up failed:\n{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let host = std::env::var("SHELLFIN_IT_HOST").unwrap_or_else(|_| "127.0.0.1".into());
        let port = std::env::var("SHELLFIN_IT_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(2222);
        wait_for_ssh_banner(&host, port, Duration::from_secs(60));
        Sshd {
            host,
            port,
            target_host: TARGET.into(),
            target_port: 22,
        }
    })
}

/// Read the first line the server sends (`SSH-2.0-…`), or `None`.
pub fn read_ssh_banner(host: &str, port: u16) -> Option<String> {
    let addr = (host, port).to_socket_addrs().ok()?.next()?;
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while buf.len() < 255 {
        match s.read(&mut byte) {
            Ok(1) if byte[0] == b'\n' => break,
            Ok(1) => buf.push(byte[0]),
            _ => return None,
        }
    }
    let line = String::from_utf8_lossy(&buf).trim_end().to_owned();
    line.starts_with("SSH-").then_some(line)
}

pub fn wait_for_ssh_banner(host: &str, port: u16, timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(b) = read_ssh_banner(host, port) {
            return b;
        }
        assert!(
            Instant::now() < deadline,
            "sshd at {host}:{port} did not answer within {timeout:?}"
        );
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// `docker compose <args…>` for this stack (e.g. `pause target`); asserts success.
pub fn docker_compose_ok(args: &[&str]) {
    let out = compose()
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("run `docker compose`");
    assert!(
        out.status.success(),
        "`docker compose {}` failed:\n{}{}",
        args.join(" "),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Unpauses `service` when dropped (a failing test must not leave the stack paused).
pub struct PauseGuard(pub &'static str);

impl PauseGuard {
    pub fn pause(service: &'static str) -> Self {
        docker_compose_ok(&["pause", service]);
        Self(service)
    }

    pub fn unpause(self) {
        // Drop does the work.
    }
}

impl Drop for PauseGuard {
    fn drop(&mut self) {
        let _ = compose()
            .args(["unpause", self.0])
            .stdin(Stdio::null())
            .output();
    }
}

/// Backs up the host keys of `service` and restores them (+ sshd reload) when dropped, so a test
/// that regenerates them leaves the shared stack as it found it (QA's and developers'
/// saved known_hosts entries keep working).
pub struct HostKeyGuard(pub &'static str);

const HOST_KEY_BACKUP: &str = "/root/shellfin-hostkey-backup";

impl HostKeyGuard {
    pub fn backup(service: &'static str) -> Self {
        docker_exec_ok(
            service,
            &[
                "sh",
                "-c",
                &format!(
                    "rm -rf {HOST_KEY_BACKUP} && mkdir -m 700 {HOST_KEY_BACKUP} \
                     && cp -p /etc/ssh/ssh_host_* {HOST_KEY_BACKUP}/"
                ),
            ],
        );
        Self(service)
    }
}

impl Drop for HostKeyGuard {
    fn drop(&mut self) {
        let out = docker_exec(
            self.0,
            &[
                "sh",
                "-c",
                &format!(
                    "rm -f /etc/ssh/ssh_host_* && cp -p {HOST_KEY_BACKUP}/* /etc/ssh/ \
                     && rm -rf {HOST_KEY_BACKUP} && pkill -HUP -o -x sshd \
                     && sleep 0.5 && for i in $(seq 50); do \
                     bash -c 'exec 3<>/dev/tcp/127.0.0.1/22' 2>/dev/null && exit 0; sleep 0.1; done; exit 1"
                ),
            ],
        );
        if !out.status.success() {
            eprintln!(
                "WARNING: could not restore the host keys of {}: {}",
                self.0,
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
}

/// `docker compose exec -T <service> <args…>`.
pub fn docker_exec(service: &str, args: &[&str]) -> Output {
    compose()
        .args(["exec", "-T", service])
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("run `docker compose exec`")
}

/// Like [`docker_exec`] but feeds `stdin` to the command.
pub fn docker_exec_stdin(service: &str, args: &[&str], stdin: &[u8]) -> Output {
    let mut child = compose()
        .args(["exec", "-T", service])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run `docker compose exec`");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin)
        .expect("write stdin");
    child.wait_with_output().expect("wait for docker exec")
}

fn expect_ok(service: &str, args: &[&str], out: Output) -> String {
    assert!(
        out.status.success(),
        "`{}` in {service} failed ({}):\n{}{}",
        args.join(" "),
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// [`docker_exec`] that asserts success and returns stdout.
pub fn docker_exec_ok(service: &str, args: &[&str]) -> String {
    let out = docker_exec(service, args);
    expect_ok(service, args, out)
}

/// [`docker_exec_stdin`] that asserts success and returns stdout.
pub fn docker_exec_stdin_ok(service: &str, args: &[&str], stdin: &[u8]) -> String {
    let out = docker_exec_stdin(service, args, stdin);
    expect_ok(service, args, out)
}

/// The target's SSH identification line as seen from the bastion (retries for up to 5 s, e.g.
/// while the target's sshd re-execs after [`regenerate_host_keys`]).
pub fn target_banner_from_bastion() -> String {
    docker_exec_ok(
        BASTION,
        &[
            "bash",
            "-c",
            "for i in $(seq 50); do \
             l=$( ( exec 3<>/dev/tcp/target/22 && head -n 1 <&3 ) 2>/dev/null ); \
             case \"$l\" in SSH-*) printf '%s\\n' \"$l\"; exit 0;; esac; sleep 0.1; \
             done; exit 1",
        ],
    )
    .trim_end()
    .to_owned()
}

/// Host public keys of a service as `(key type, base64 blob)`, sorted by type.
pub fn host_public_keys(service: &str) -> Vec<(String, String)> {
    let out = docker_exec_ok(service, &["sh", "-c", "cat /etc/ssh/ssh_host_*_key.pub"]);
    let mut keys: Vec<(String, String)> = out
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some((it.next()?.to_owned(), it.next()?.to_owned()))
        })
        .collect();
    keys.sort();
    keys
}

/// Replace all host keys of `service` and make sshd reload them (SIGHUP re-execs the listener).
/// Returns once the service accepts connections again.
pub fn regenerate_host_keys(service: &str) {
    let before = host_public_keys(service);
    docker_exec_ok(
        service,
        &[
            "sh",
            "-c",
            "d=$(mktemp -d) && mkdir -p \"$d/etc/ssh\" && ssh-keygen -A -f \"$d\" >/dev/null \
             && mv -f \"$d\"/etc/ssh/ssh_host_* /etc/ssh/ && rm -rf \"$d\" \
             && pkill -HUP -o -x sshd \
             && sleep 0.5 && for i in $(seq 50); do \
             bash -c 'exec 3<>/dev/tcp/127.0.0.1/22' 2>/dev/null && exit 0; sleep 0.1; done; exit 1",
        ],
    );
    let after = host_public_keys(service);
    assert_ne!(before, after, "host keys of {service} did not change");
    if service == BASTION {
        let s = sshd();
        wait_for_ssh_banner(&s.host, s.port, Duration::from_secs(20));
    }
}

// ---------------------------------------------------------------------------------------------
// Fixtures and private temp dirs
// ---------------------------------------------------------------------------------------------

/// A temp dir that the app's §7.4.1 trust checks accept (Windows: protected owner-only DACL
/// inherited by children; Unix: 0700). Use it for known_hosts, configs and keys the tests write.
pub fn private_tempdir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    shellfin_lib::platform::test_support::make_private(dir.path());
    dir
}

/// Write `bytes` to a new file in a private dir (the file inherits the dir's ACL on Windows) with
/// `mode` on Unix.
pub fn write_private_file(dir: &Path, name: &str, bytes: &[u8], mode: u32) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("write fixture copy");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).expect("chmod");
    }
    #[cfg(not(unix))]
    let _ = mode;
    path
}

/// Raw bytes of a fixture key file (e.g. `"id_rsa_pem"` or `"id_rsa_pem.pub"`).
pub fn fixture_bytes(file: &str) -> Vec<u8> {
    std::fs::read(fixture_dir().join(file)).unwrap_or_else(|e| panic!("fixture {file}: {e}"))
}

/// Copy fixture key `name` (and `name.pub`) into a fresh private temp dir: key 0600, `.pub`
/// 0644 on Unix; on Windows both are written new so they inherit the dir's owner-only DACL.
/// Returns the dir guard (keep it alive) and the private key path.
pub fn private_key_copy(name: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = private_tempdir();
    let key = write_private_file(dir.path(), name, &fixture_bytes(name), 0o600);
    let pub_name = format!("{name}.pub");
    write_private_file(dir.path(), &pub_name, &fixture_bytes(&pub_name), 0o644);
    (dir, key)
}

// ---------------------------------------------------------------------------------------------
// TOTP (RFC 6238, HMAC-SHA1)
// ---------------------------------------------------------------------------------------------

/// RFC 4648 base32 (case-insensitive; `=`, spaces and `-` ignored).
pub fn base32_decode(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut acc: u64 = 0;
    let mut bits = 0u32;
    for c in s.chars().filter(|c| !matches!(c, '=' | ' ' | '-')) {
        let v = match c.to_ascii_uppercase() {
            c @ 'A'..='Z' => c as u64 - 'A' as u64,
            c @ '2'..='7' => c as u64 - '2' as u64 + 26,
            other => panic!("invalid base32 character {other:?}"),
        };
        acc = (acc << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

/// HOTP/TOTP code for raw `key` at `unix_time` (30 s step) with `digits` digits.
pub fn totp_with_key(key: &[u8], unix_time: u64, digits: u32) -> String {
    let counter = (unix_time / 30).to_be_bytes();
    let k = ring::hmac::Key::new(ring::hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, key);
    let tag = ring::hmac::sign(&k, &counter);
    let h = tag.as_ref();
    let offset = (h[h.len() - 1] & 0x0f) as usize;
    let bin = (u32::from(h[offset] & 0x7f) << 24)
        | (u32::from(h[offset + 1]) << 16)
        | (u32::from(h[offset + 2]) << 8)
        | u32::from(h[offset + 3]);
    let code = bin % 10u32.pow(digits);
    format!("{code:0width$}", width = digits as usize)
}

/// 6-digit TOTP for a base32 secret at `unix_time`.
pub fn totp(secret_b32: &str, unix_time: u64) -> String {
    totp_with_key(&base32_decode(secret_b32), unix_time, 6)
}

/// 6-digit TOTP for a base32 secret now.
pub fn totp_now(secret_b32: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    totp(secret_b32, now)
}
