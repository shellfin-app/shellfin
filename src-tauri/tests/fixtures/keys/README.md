# Test-only SSH keys — never use them anywhere else

These keys exist **only** for Shellfin's tests: the Docker integration suite (`src-tauri/tests/`,
feature `it`) and, under `unit/`, the unit tests (compiled in via `include_str!` from
`src-tauri/src/ssh/keys.rs`). They were generated for this repository with `ssh-keygen` and are
public by design.

- They grant access only to the throwaway `tester*` users in the containers built from
  `src-tauri/tests/sshd/` (`docker compose -p shellfin-it …`), which listen on `127.0.0.1` only.
- They are not, and never were, real credentials. Every OpenSSH-format key carries the comment
  `shellfin-test` (PEM keys have no comment field).
- Passphrases are test constants: `KEY_PASSPHRASE` in `src-tauri/tests/support/mod.rs` (integration
  suite) and `fixtures::PASSPHRASE` in `src-tauri/src/ssh/keys.rs` (`unit/`).
- Every private key needs a sibling `<name>.pub` with the comment `shellfin-test`. The unit test
  `ssh::keys::tests::fixture_keys_are_all_test_keys` (run in every CI unit job) checks the
  content: the key must open with a known test passphrase and match its `.pub`, and it must
  carry the embedded comment `shellfin-test` (OpenSSH format) or a pinned fingerprint (PEM).
- Secret scanners are told to ignore this directory: `.gitleaks.toml` (repository root) and
  `.github/secret_scanning.yml`. Do not add any other key material here, and never put a real
  key into the repository.
