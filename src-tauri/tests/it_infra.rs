//! Smoke test of the Docker integration infrastructure (§10.2): the compose stack, the harness
//! helpers, fixture keys, TOTP, and the sshd/PAM configuration (checked with a bare russh
//! client — independent of the app's own SSH code).
//!
//! Run: `cargo test --features it --test it_infra -- --nocapture` (from `src-tauri/`).
#![cfg(feature = "it")]
// Test code: spawns `docker` (through the harness) and fails fast with unwrap/expect.
#![allow(clippy::disallowed_methods, clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client::{self, AuthResult, KeyboardInteractiveAuthResponse};
use russh::keys::{PrivateKey, PrivateKeyWithHashAlg, PublicKeyOrCertificate, decode_secret_key};
use support::*;
use tokio::io::AsyncReadExt;

#[test]
fn rfc6238_vectors() {
    // RFC 6238 Appendix B, SHA1, 8 digits, key "12345678901234567890".
    let key = b"12345678901234567890";
    for (t, code) in [
        (59, "94287082"),
        (1_111_111_109, "07081804"),
        (1_111_111_111, "14050471"),
        (1_234_567_890, "89005924"),
        (2_000_000_000, "69279037"),
        (20_000_000_000, "65353130"),
    ] {
        assert_eq!(totp_with_key(key, t, 8), code, "t={t}");
    }
    assert_eq!(
        base32_decode("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"),
        key.to_vec()
    );
    assert_eq!(totp(TOTP_SECRET, 59).len(), 6);
}

#[test]
fn stack_is_up_and_exec_works() {
    let s = sshd();
    let banner = read_ssh_banner(&s.host, s.port).expect("ssh banner");
    assert!(banner.starts_with("SSH-2.0-OpenSSH"), "{banner}");
    assert_eq!(docker_exec_ok(BASTION, &["echo", "hello"]).trim(), "hello");
    // ssh-keygen exists in the image (used for `ssh-keygen -F` checks of learned lines).
    let out = docker_exec(
        BASTION,
        &["ssh-keygen", "-F", "nonexistent.example", "-f", "/dev/null"],
    );
    assert!(out.status.code().is_some(), "ssh-keygen did not run");
    // The target is reachable from the bastion only.
    assert!(target_banner_from_bastion().starts_with("SSH-2.0-OpenSSH"));
    for service in [BASTION, TARGET] {
        let keys = host_public_keys(service);
        assert!(
            keys.iter().any(|(t, _)| t == "ssh-ed25519"),
            "{service}: {keys:?}"
        );
        assert!(
            keys.iter().any(|(t, _)| t == "ssh-rsa"),
            "{service}: {keys:?}"
        );
    }
}

#[test]
fn sshd_effective_config() {
    sshd();
    let t = |user: &str| {
        docker_exec_ok(
            BASTION,
            &[
                "sshd",
                "-T",
                "-C",
                &format!("user={user},host=client.example,addr=127.0.0.1"),
            ],
        )
    };
    let fa = t(USER_2FA);
    assert!(
        fa.lines()
            .any(|l| l == "authenticationmethods publickey,keyboard-interactive"),
        "{fa}"
    );
    assert!(t(USER_LOW_TRIES).lines().any(|l| l == "maxauthtries 3"));
    let normal = t(USER);
    assert!(normal.lines().any(|l| l == "maxauthtries 6"));
    assert!(normal.lines().any(|l| l == "authenticationmethods any"));
    assert!(normal.lines().any(|l| l == "allowtcpforwarding yes"));
    assert!(
        normal
            .lines()
            .any(|l| l == "kbdinteractiveauthentication yes")
    );
    let shells = docker_exec_ok(
        BASTION,
        &[
            "sh",
            "-c",
            "getent passwd tester-fish tester-tcsh | cut -d: -f7",
        ],
    );
    assert_eq!(
        shells.lines().collect::<Vec<_>>(),
        ["/usr/bin/fish", "/usr/bin/tcsh"]
    );
}

#[test]
fn fixture_public_keys_match_private_keys() {
    sshd();
    let mut names: Vec<&str> = vec![KEY_ED25519_ENC, KEY_RSA_OPENSSH, KEY_RSA_PEM, KEY_ED25519];
    names.extend(AGENT_EXTRA_KEYS);
    for name in names {
        let private = fixture_bytes(name);
        let passphrase = if name == KEY_ED25519_ENC {
            KEY_PASSPHRASE
        } else {
            ""
        };
        let derived = docker_exec_stdin_ok(
            BASTION,
            &[
                "sh",
                "-c",
                "umask 077; f=$(mktemp); cat > \"$f\"; ssh-keygen -y -P \"$1\" -f \"$f\"; rc=$?; rm -f \"$f\"; exit $rc",
                "sh",
                passphrase,
            ],
            &private,
        );
        let expected = String::from_utf8(fixture_bytes(&format!("{name}.pub"))).unwrap();
        let two = |s: &str| s.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
        assert_eq!(two(&derived), two(&expected), "{name}");
        assert!(
            expected.trim_end().ends_with("shellfin-test"),
            "{name}: comment"
        );
    }
    // private_key_copy produces a readable private copy.
    let (_dir, path) = private_key_copy(KEY_RSA_PEM);
    assert_eq!(std::fs::read(&path).unwrap(), fixture_bytes(KEY_RSA_PEM));
    assert!(
        path.with_extension("pub").exists()
            || path.parent().unwrap().join("id_rsa_pem.pub").exists()
    );
}

// ---------------------------------------------------------------------------------------------
// Bare russh client: verifies the server-side auth setup the app's tests rely on.
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Default)]
struct Probe {
    banner: Arc<Mutex<Option<String>>>,
}

impl client::Handler for Probe {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        Ok(true) // infrastructure smoke test only; host-key logic is tested against the app
    }

    async fn auth_banner(
        &mut self,
        banner: &str,
        _s: &mut client::Session,
    ) -> Result<(), Self::Error> {
        *self.banner.lock().unwrap() = Some(banner.to_owned());
        Ok(())
    }
}

async fn connect(probe: Probe) -> client::Handle<Probe> {
    let s = sshd();
    let config = Arc::new(client::Config {
        inactivity_timeout: Some(Duration::from_secs(30)),
        ..Default::default()
    });
    client::connect(config, (s.host.as_str(), s.port), probe)
        .await
        .expect("ssh connect")
}

fn secret(name: &str, passphrase: Option<&str>) -> Arc<PrivateKey> {
    let pem = String::from_utf8(fixture_bytes(name)).unwrap();
    Arc::new(decode_secret_key(&pem, passphrase).expect("decode fixture key"))
}

fn key(name: &str, passphrase: Option<&str>) -> PrivateKeyWithHashAlg {
    PrivateKeyWithHashAlg::new(secret(name, passphrase), None)
}

fn prompts(r: &KeyboardInteractiveAuthResponse) -> Vec<String> {
    match r {
        KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
            prompts.iter().map(|p| p.prompt.clone()).collect()
        }
        other => panic!("expected InfoRequest, got {other:?}"),
    }
}

/// OpenSSH+PAM ends keyboard-interactive with an empty InfoRequest (0 prompts) after the last
/// answer; the client must respond with an empty list (§5.6). Returns the final response.
async fn finish_kbd(
    h: &mut client::Handle<Probe>,
    mut r: KeyboardInteractiveAuthResponse,
) -> KeyboardInteractiveAuthResponse {
    while matches!(&r, KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } if prompts.is_empty())
    {
        r = h
            .authenticate_keyboard_interactive_respond(vec![])
            .await
            .unwrap();
    }
    r
}

#[tokio::test]
async fn password_kbd_int_and_banner() {
    let probe = Probe::default();
    let mut h = connect(probe.clone()).await;
    // NOTE: no failed `password`-method attempt before keyboard-interactive here: OpenSSH+PAM
    // then drops the connection after a successful kbd-int login (see tests/sshd/pam-sshd).
    // A wrong kbd-int answer followed by a correct one is fine:
    let r = h
        .authenticate_keyboard_interactive_start(USER, None::<String>)
        .await
        .unwrap();
    assert_eq!(prompts(&r), ["Password: "]);
    let r = h
        .authenticate_keyboard_interactive_respond(vec!["wrong".into()])
        .await
        .unwrap();
    let r = finish_kbd(&mut h, r).await;
    assert!(
        matches!(r, KeyboardInteractiveAuthResponse::Failure { .. }),
        "{r:?}"
    );
    // Normal users: one "Password:" prompt over keyboard-interactive.
    let r = h
        .authenticate_keyboard_interactive_start(USER, None::<String>)
        .await
        .unwrap();
    assert_eq!(prompts(&r), ["Password: "]);
    let r = h
        .authenticate_keyboard_interactive_respond(vec![USER_PASSWORD.into()])
        .await
        .unwrap();
    let r = finish_kbd(&mut h, r).await;
    assert!(
        matches!(r, KeyboardInteractiveAuthResponse::Success),
        "{r:?}"
    );
    assert!(
        probe
            .banner
            .lock()
            .unwrap()
            .as_deref()
            .is_some_and(|b| b.contains(BANNER)),
        "banner"
    );
    // direct-tcpip to the internal target works (AllowTcpForwarding yes).
    let ch = h
        .channel_open_direct_tcpip(TARGET, 22, "127.0.0.1", 0)
        .await
        .expect("direct-tcpip to target");
    let mut stream = ch.into_stream();
    let mut buf = [0u8; 7];
    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"SSH-2.0");
}

#[tokio::test]
async fn password_auth_with_each_key_kind() {
    for (user, name, pass) in [
        (USER, KEY_RSA_OPENSSH, None),
        (USER, KEY_ED25519_ENC, Some(KEY_PASSPHRASE)),
        (USER_LOW_TRIES, KEY_RSA_OPENSSH, None),
        (USER_FISH, KEY_ED25519_ENC, Some(KEY_PASSPHRASE)),
        (USER_TCSH, KEY_RSA_OPENSSH, None),
    ] {
        let mut h = connect(Probe::default()).await;
        let hash = if name.starts_with("id_rsa") {
            h.best_supported_rsa_hash().await.unwrap().flatten()
        } else {
            None
        };
        let k = PrivateKeyWithHashAlg::new(secret(name, pass), hash);
        let r = h.authenticate_publickey(user, k).await.unwrap();
        assert!(matches!(r, AuthResult::Success), "{user} {name}: {r:?}");
    }
    let mut h = connect(Probe::default()).await;
    let r = h.authenticate_password(USER, USER_PASSWORD).await.unwrap();
    assert!(matches!(r, AuthResult::Success), "{r:?}");
}

#[tokio::test]
async fn two_factor_password_then_totp() {
    let mut h = connect(Probe::default()).await;
    let r = h
        .authenticate_publickey(USER_2FA, key(KEY_ED25519, None))
        .await
        .unwrap();
    match r {
        AuthResult::Failure {
            partial_success, ..
        } => assert!(partial_success),
        other => panic!("expected partial success, got {other:?}"),
    }
    let r = h
        .authenticate_keyboard_interactive_start(USER_2FA, None::<String>)
        .await
        .unwrap();
    assert_eq!(prompts(&r), ["Password: "]);
    let r = h
        .authenticate_keyboard_interactive_respond(vec![USER_2FA_PASSWORD.into()])
        .await
        .unwrap();
    assert_eq!(prompts(&r), ["Verification code: "]);
    let r = h
        .authenticate_keyboard_interactive_respond(vec![totp_now(TOTP_SECRET)])
        .await
        .unwrap();
    let r = finish_kbd(&mut h, r).await;
    assert!(
        matches!(r, KeyboardInteractiveAuthResponse::Success),
        "{r:?}"
    );
}

#[tokio::test]
async fn unauthorized_keys_are_rejected() {
    let mut h = connect(Probe::default()).await;
    let r = h
        .authenticate_publickey(USER, key(AGENT_EXTRA_KEYS[0], None))
        .await
        .unwrap();
    assert!(matches!(r, AuthResult::Failure { .. }), "{r:?}");
}

/// Mutates shared server state (new host keys for `target`), so it only runs on request:
/// `cargo test --features it --test it_infra regenerate -- --ignored`.
#[test]
#[ignore = "changes the target's host keys"]
fn regenerate_target_host_keys() {
    sshd();
    let before = host_public_keys(TARGET);
    regenerate_host_keys(TARGET);
    let after = host_public_keys(TARGET);
    assert_ne!(before, after);
    assert!(target_banner_from_bastion().starts_with("SSH-2.0-OpenSSH"));
}
