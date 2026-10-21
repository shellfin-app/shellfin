# Contributing to Shellfin

Thank you for your interest. Shellfin is in early development (pre-alpha), so the architecture is still moving.
For anything bigger than a small fix, please open an issue or a discussion first so we can agree on the approach
before you invest time.

By participating you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md). Do **not** report security
problems in public issues; follow [SECURITY.md](SECURITY.md).

## Setting up

Prerequisites are listed in the [README](README.md#building-from-source): stable Rust (MSRV 1.90), Node.js 24.22.1 or
newer (24 LTS; `.nvmrc` and `engines` in `package.json` enforce it) and the Tauri platform dependencies.

```sh
npm ci
npm run tauri dev
```

`npm run dev` runs only the Vite frontend against the in-memory mock backend, which is enough for most UI work.
Read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) before touching the Rust core or the IPC layer; its decisions
are binding and are changed through a pull request to that file.

## Quality gates

CI runs the following; a pull request must pass all of them. Run them locally before pushing.

Rust (from the repository root):

```sh
cd src-tauri
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --all-targets --features it -- -D warnings
cargo clippy --locked --all-targets --features e2e -- -D warnings
cargo test --locked
cargo test --locked --features e2e --lib e2e::
```

Frontend:

```sh
npm run lint          # ESLint, zero warnings allowed
npm run typecheck
npm run test          # Vitest
npm run i18n:check    # translation keys match the code
npm run build         # type check + production bundle + "no mock backend in the bundle" check
npm run test:freeze   # bundle still works with Object.prototype frozen (as Tauri does at runtime)
```

`npm run bindings` regenerates the TypeScript bindings (`npm run bindings:check` also fails on a diff, a new
untracked binding or a binding whose Rust type is gone). `npm run rust:check` runs fmt, the default clippy and the
tests. If you use an AI coding agent, point it at [AGENTS.md](AGENTS.md).

Also checked in CI: bindings drift (see below), `npm run notices:check` (the third-party licence notices are
regenerated from the lockfiles; after a dependency change run `npm run notices`, which needs
`cargo install cargo-about --version 0.9.2 --locked --features cli` and `npm ci`), `cargo check` on the minimum
Rust version, `actionlint`, `zizmor`, `gitleaks`, `cargo deny`, `cargo audit` and `npm audit`.

Integration tests against a Docker-based `sshd` (a target host plus a bastion for ProxyJump) are behind the Cargo
feature `it`. They need a running Docker engine; the test harness starts the fixtures from `src-tauri/tests/sshd/`
itself:

```sh
cd src-tauri
cargo test --locked --features it
# afterwards, remove the fixture containers:
docker compose -p shellfin-it -f tests/sshd/compose.yml down -v
```

The private keys under `src-tauri/tests/` are throwaway test fixtures, not real credentials. The strategy is
described in section 10.2 of [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Rules

- **No hard-coded user-facing text.** Every string the user can see goes through i18n (`t("...")` with a key in
  `src/i18n/locales/en/translation.json`). ESLint (`i18next/no-literal-string`) enforces this. Backend errors are
  codes plus parameters, never prose. Technical data (hosts, paths, `known_hosts`) is passed as interpolation
  parameters.
- **IPC types are generated from Rust.** The TypeScript files in `src/ipc/bindings/` come from the Rust DTOs in
  `src-tauri/src/ipc/dto.rs` via [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Never edit them by hand.
  After changing a DTO run `npm run bindings` and commit the result; CI fails if the committed bindings differ
  from what `cargo test` generates.
- **Security-sensitive areas need extra care and tests.** This covers SSH and host-key handling, authentication,
  SFTP and local path handling, secret storage, the Tauri ACL/capabilities, the CSP and anything that renders
  server-controlled text. Read section 9 of the architecture document, add tests for the new behaviour
  (including hostile input), and do not widen the webview's permissions without discussing it first. Never log
  secrets, terminal data or file contents.
- **No `unwrap()`/`expect()`** in non-test Rust code (denied by clippy), and no `std::process::Command`.
- **Dependencies** are pinned to exact versions and the lockfiles are committed. Adding one needs a short
  justification in the pull request, and `cargo deny` must stay green.
- **No secrets or personal data** in code, tests, fixtures, logs or screenshots. Use `example.com` and similar
  placeholders.

## Commits and pull requests

- Use [Conventional Commits](https://www.conventionalcommits.org/): `type(scope): summary`, for example
  `feat(sftp): add atomic save` or `fix(ui): keep terminal scrollback on tab switch`. Common types: `feat`,
  `fix`, `docs`, `refactor`, `test`, `build`, `ci`, `chore`.
- Keep pull requests focused and small enough to review. Link the issue they address.
- There is no Contributor License Agreement and no DCO sign-off requirement.
- Fill in the pull request template. In short, before requesting review: all gates above pass, new behaviour has
  tests, user-facing text is translated via i18n, bindings are regenerated if Rust DTOs changed, documentation
  and `CHANGELOG.md` (under `[Unreleased]`) are updated where relevant.

## Releasing

Maintainers only. The version comes from `package.json` (`tauri.conf.json` reads it). Bump it (also in
`src-tauri/Cargo.toml` and the lockfiles), rename `[Unreleased]` in `CHANGELOG.md` to `## [X.Y.Z] - YYYY-MM-DD` (add a
fresh empty `[Unreleased]` above it and the link definitions at the bottom), and merge that to `main` first. Wait for
the CI run of the merge commit to finish: the release refuses a commit without a green CI run.

1. Tag the merge commit: `git tag -s v0.1.0 && git push origin v0.1.0`. The tag must equal
   `v<package.json version>` (the workflow fails otherwise).
2. `.github/workflows/release.yml` first checks that the tagged commit is on `main` and that its CI run succeeded
   (`verify`). The build jobs then run in the protected `release` environment; a reviewer has to approve them.
   For each platform (Windows x64 and Windows ARM64 with an NSIS installer only, no MSI; Linux x64; macOS arm64; no
   32-bit Windows, no Intel Mac) the order is fixed: **build, then checks, then upload**. The checks: no
   `SHELLFIN_E2E_ROOT` marker in any release binary and no `e2e` feature in the build arguments, no
   `__shellfin_mock_backend__` marker in the embedded `dist/`, `VITE_BACKEND` unset. Binaries are built with a pinned
   compiler (`src-tauri/rust-toolchain.toml`), `--remap-path-prefix` and `SOURCE_DATE_EPOCH`, so they contain no
   runner paths. Only when every build passed does the last job create `SHA256SUMS`, GitHub build provenance
   attestations and a **draft** release. Without Apple secrets the macOS app is ad-hoc signed (see the README,
   Install, for what users see); remove those notes once Developer ID signing and notarization are active.
3. Review the draft (NSIS installers for x64 and ARM64, DMG, `.app.tar.gz`, AppImage, deb, rpm, the `.sig` files,
   `latest.json` and the two CycloneDX SBOMs `shellfin-rust.cdx.json` and `shellfin-npm.cdx.json`), verify with
   `gh attestation verify <file> -R shellfin-app/shellfin` and `sha256sum -c SHA256SUMS`, check that `latest.json`
   names this version and points to this tag, and read the notes. The notes are the `CHANGELOG.md` section of the
   version (generated notes when there is none, as for pre-releases); edit them if needed, then publish manually.
   The workflow never publishes.
4. **Publishing is what ships the update.** A draft reaches no user. When you publish, `update-channels.yml` runs
   and copies the release's `latest.json` into the permanent `updater` release: a stable release updates
   `stable.json` and `experimental.json`, a pre-release only `experimental.json`. It never replaces a channel file
   with an older or equal version, so republishing an old release is harmless. Check the run in the Actions tab.
   If it failed (or to redo it), run "Update channels" by hand with the tag. Never delete the `updater` release
   (a pre-release titled "Update channels — do not delete"): installed apps read
   `https://github.com/shellfin-app/shellfin/releases/download/updater/stable.json` and `.../experimental.json`.

### Experimental (pre-release) builds

Use a SemVer pre-release version, by convention `X.Y.Z-experimental.N` (for example `0.3.0-experimental.1`), in
`package.json` and `src-tauri/Cargo.toml`, and tag `vX.Y.Z-experimental.N`. The release workflow builds only the
AppImage on Linux (plus the same installers as for a stable release elsewhere) and creates the draft already marked as
a **pre-release**; keep that mark when you publish. Never mark a pre-release as "latest". Publishing then updates `experimental.json` only. Users on the
experimental channel also get every stable release that is newer than their build (`0.3.0` is newer than
`0.3.0-experimental.4`); users switching back to stable never get a downgrade, they wait for the next stable release.

### Publish-day checklist (once, before the first push and tag)

Settings of the `shellfin-app/shellfin` repository that the files in this repository rely on. Nothing here can be
committed.

- **Security:** enable "Private vulnerability reporting" (`SECURITY.md` links to `/security/advisories/new`). Make sure
  `security@shellfin.app` and `conduct@shellfin.app` (Code of Conduct reports) receive mail, for example as forwards.
  If GitHub push protection flags the throwaway keys in `src-tauri/tests/fixtures/keys/` on the first push, bypass it
  as "used in tests".
- **Community:** enable Discussions (issue template links and the first lines of this file point to it). Create the
  team `@shellfin-app/maintainers` with write access; otherwise `.github/CODEOWNERS` is silently ignored.
- **Branch protection** for `main` (ruleset): pull request required, linear history, and the required status checks
  `checks`, `msrv`, `homebrew-cask`, `workflows`, `unit-native`, `integration`, `security` and `build` (every CI job
  except the schedule-only run).
- **Homebrew tap:** create the public, empty repository `shellfin-app/homebrew-tap`. Create a fine-grained personal
  access token (or a GitHub App token) with **Contents: read and write on that repository only**, and add it as the
  secret `HOMEBREW_TAP_TOKEN` of the `release` environment. `.github/workflows/homebrew-tap.yml` then pushes
  `Casks/shellfin.rb` (stable) or `Casks/shellfin@experimental.rb` (pre-release) when you publish a release, after
  verifying the attestation of `SHA256SUMS`; it creates the tap README on first use and never downgrades. Without the
  secret it only prints a notice. Once releases are notarized, run the renderer with `--notarized true` (edit the
  workflow step) to drop the "Open Anyway" caveat.
- **Tag protection:** a tag ruleset for `v*` (only maintainers create it; no deletion, no force update).
- **`release` environment** (Settings, Environments): required reviewers, "Prevent self-review", deployment tags
  limited to `v*`. Secrets of that environment: `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`
  (below); optionally the Apple (`APPLE_*`), Azure (`AZURE_*`) and GPG (`GPG_*`) secrets listed further down.
- **Actions:** workflows run with a read-only default token (Settings, Actions, General); the workflows grant what
  they need per job.

### Updater signing key (required)

Updates are signed with a minisign key (Tauri updater). The public key is committed in
`src-tauri/tauri.conf.json` (`plugins.updater.pubkey`, key id `559240A91983C276`). The release workflow of this
repository fails without the private key; forks build without updater artifacts.

1. The key pair is created once by the maintainer, offline: `npm run tauri signer generate -- -w <path outside the
   repository>`, with a strong password. Never commit the private key or put it anywhere in the repository.
2. Add two secrets to the `release` environment (Settings, Environments, `release`, "Environment secrets"), before
   pushing the first tag: `TAURI_SIGNING_PRIVATE_KEY` (the content of the private key file, the single base64 line)
   and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (its password). Only the Tauri build step receives them; pull requests,
   forks and `ci.yml` never do.
3. **Back the key up**: the private key file and its password, encrypted, in at least two offline places (for
   example a password manager plus an encrypted USB stick). If the key is lost, installed copies can never be
   updated automatically again: users would have to download the next version by hand, which ships a new public
   key.
4. If the key may be compromised: delete both secrets at once, do not publish pending drafts, and follow
   `SECURITY.md`. Anyone with the key and a way to serve a manifest can push signed updates to every installation
   (ARCHITECTURE §9.8 explains the trust model and the recovery).

Platform signing is optional. A step is active only when all of its secrets exist in the `release` environment;
unsigned builds still work.

- macOS (Developer ID + notarization): `APPLE_CERTIFICATE` (base64 .p12, created with `base64 -w0`), `APPLE_CERTIFICATE_PASSWORD`,
  `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD` (app-specific), `APPLE_TEAM_ID`.
- Windows (Authenticode via Azure Artifact/Trusted Signing): `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`,
  `AZURE_TENANT_ID`, `AZURE_SIGNING_ENDPOINT`, `AZURE_SIGNING_ACCOUNT`, `AZURE_SIGNING_PROFILE`.
- Checksums (optional): `GPG_PRIVATE_KEY` (armored) and `GPG_PASSPHRASE` produce `SHA256SUMS.asc`. To sign by hand
  instead, run `gpg --armor --detach-sign SHA256SUMS` and attach the `.asc` to the draft. Publish the key
  fingerprint in the README.

## Licensing of contributions

Shellfin is dual-licensed under MIT OR Apache-2.0. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in this project by you, as defined in the Apache-2.0 license, shall be dual
licensed as above, without any additional terms or conditions (inbound = outbound). Only submit code you have the
right to contribute under these terms.

## Adding a translation

The UI ships in English (default and fallback) and Ukrainian (`uk`). Translations into other languages are welcome;
please open an issue first. `src/i18n/locales/uk/translation.json` is the reference for how a second language
looks, and `docs/i18n-glossary-uk.md` shows how a glossary is kept (add one for your language).

1. Copy `src/i18n/locales/en/translation.json` to `src/i18n/locales/<lng>/translation.json` (a BCP-47 language
   code such as `de`) and translate the values. Keep the keys, the `{{placeholders}}` and the plural
   suffixes (`_one`, `_few`, `_many`, `_other` as required by the language's CLDR plural rules) intact.
2. Do not translate technical terms such as `known_hosts`, `ssh-agent`, `SHA256` or the product name. Keep
   terminology consistent with the glossary of your language (Ukrainian: `docs/i18n-glossary-uk.md`).
3. Register the language: add it to `locales` in `i18next.config.ts` and to `SUPPORTED_LANGUAGES` and
   `LANGUAGE_ENDONYMS` in `src/i18n/index.ts` (locale files are lazy-loaded chunks). The language then appears in
   Settings > General > Language and the macOS menu follows it.
4. Run `npm run i18n:check`, `npm run test` and `npm run lint`. The parity test `src/i18n/parity.test.ts` checks
   that your file has the same keys as English (plural-aware), the plural forms of the language, the same
   `{{tokens}}` and no empty strings. In dev builds a pseudo-locale (`en-XA`) is
   available to spot text that does not go through i18n or does not fit its layout.
5. Open a pull request describing the language and your confidence level; a second reviewer who speaks the
   language is appreciated.
