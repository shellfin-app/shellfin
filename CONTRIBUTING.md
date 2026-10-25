# Contributing to Shellfin

Thank you for your interest. Shellfin is at an early release stage (0.x), so the architecture is still moving.
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
npm run i18n:check    # translation keys match the code, menu bar mnemonics are unique
npm run build         # type check + production bundle + "no mock backend in the bundle" check
npm run test:freeze   # bundle still works with Object.prototype frozen (as Tauri does at runtime)
```

`npm run bindings` regenerates the TypeScript bindings (`npm run bindings:check` also fails on a diff, a new
untracked binding or a binding whose Rust type is gone). `npm run rust:check` runs fmt, the default clippy and the
tests. If you use an AI coding agent, point it at [AGENTS.md](AGENTS.md).

Also checked in CI: bindings drift (see below), `npm run notices:check` (the third-party licence notices are
regenerated from the lockfiles; after a dependency change run `npm run notices`, which needs
`cargo install cargo-about --version 0.9.2 --locked --features cli` and `npm ci`), `cargo check` on the minimum
Rust version, `actionlint`, `zizmor`, `gitleaks`, `cargo deny`, `cargo audit`, `npm audit`, a guard that `rsa`
resolves to exactly one 0.10.x, no inline `<script>`/`<style>` in `dist/index.html`, and a rendered and
`brew style`-checked Homebrew cask (`scripts/homebrew-cask.mjs`). On a pull request CI
skips the jobs your changes cannot affect (for example the Rust and installer jobs for a change under `src/` only; the
build configuration such as `vite.config.ts`, `tsconfig*.json` or `package.json` still runs the installer builds; rules
in `.github/scripts/ci-changes.sh`) and runs again when you change the pull request's base branch; `ci-ok` sums up the
run. Pushes to `main` always run everything, and there `ci-ok` fails if any job was skipped.

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
- Open every pull request against `main` (see [Branches](#branches)).
- There is no Contributor License Agreement and no DCO sign-off requirement.
- Fill in the pull request template. In short, before requesting review: all gates above pass, new behaviour has
  tests, user-facing text is translated via i18n, bindings are regenerated if Rust DTOs changed, documentation
  and `CHANGELOG.md` (under `[Unreleased]`) are updated where relevant.

## Branches

- There is one long-lived branch, `main`. Every release (`vX.Y.Z`) is tagged on it, and there is one update channel.
- Features and fixes: branch off `main` (for example `feat/<issue>-<slug>` or `fix/<issue>-<slug>`) and open the pull
  request against `main`. Dependabot opens its pull requests against `main` too.
- Merge pull requests with **Rebase and merge** (the repository allows other methods, so choose it explicitly). The
  repository deletes merged head branches.

## Releasing

Maintainers only. The version comes from `package.json` (`tauri.conf.json` reads it). Releases are tagged on `main`
only, and there are no pre-releases. Bump the version to `X.Y.Z` (also in `src-tauri/Cargo.toml` and the lockfiles;
run `npm run notices`, the notices name the version), rename `[Unreleased]` in `CHANGELOG.md` to
`## [X.Y.Z] - YYYY-MM-DD` (add a fresh empty `[Unreleased]` above it and the link definitions at the bottom), and
merge that to `main` through a pull request first. Wait for the CI run of that push to `main` to finish: the release
only accepts a commit with a successful CI run of a push to `main`, that is the commit that was pushed as the tip of
`main`, never a commit from inside a merged pull request.

1. Tag exactly the tip of `main` whose push CI run succeeded. Pull requests are rebased onto `main`, so only the last
   rebased commit was pushed as the tip and has a push run; the commits before it have none and are refused.

   ```sh
   git fetch origin
   git rev-parse origin/main   # must be the head SHA of the green run listed below
   gh run list --workflow ci.yml --branch main --event push --limit 1 --json headSha,conclusion
   git tag -s vX.Y.Z origin/main && git push origin vX.Y.Z
   ```

   The tag must equal `v<package.json version>` and have exactly the form `vX.Y.Z` (decimal numbers without leading
   zeros, no pre-release suffix); the workflow fails otherwise.
2. `.github/workflows/release.yml` first checks the tag's form, that the tagged commit is on `main` and that a CI run
   of a push to `main` succeeded for that commit (`verify`). The `frontend` job then builds `dist/` once, without
   any secret (the only job that runs `npm ci` and npm devDependencies), and uploads it with a SHA-256 manifest. The
   build jobs, which run no npm package, verify every file of it and run `tauri build` with the prebuilt CLI of
   `@tauri-apps/cli` (the platform package's tarball, checked against the sha512 in `package-lock.json`). The
   manifest protects only the hand-over: what the devDependencies put into `dist/` ships. Then the four installer
   artifacts must have exactly the expected file names. The build jobs run in the `release` environment,
   which holds the signing secrets and accepts only `v*` tags. With a single maintainer there is no reviewer gate:
   `verify` and the tag ruleset are the automated gates, and the draft is reviewed and published by hand (steps 3
   and 4). For each platform (Windows x64 and Windows ARM64 with an NSIS installer only, no MSI; Linux x64; macOS
   arm64; no 32-bit Windows, no Intel Mac) the order is fixed: **build, then checks, then upload**. The checks: no
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
   version (generated notes when there is none); edit them if needed, then publish manually (do not mark it as a
   pre-release). The workflow never publishes.
4. **Publishing is what ships the update.** A draft reaches no user. When you publish, `update-channels.yml` runs
   and copies the release's `latest.json` into the permanent `updater` release, as `stable.json` (the manifest the
   app reads) and as `experimental.json`. It never replaces either file with an older or equal version, so
   republishing an old release is harmless, and it refuses a release marked as a pre-release or a tag other than
   `vX.Y.Z`. It also fails when a manifest the `updater` release lists cannot be downloaded or read (it never treats
   such a file as missing). Check the run in the Actions tab. If it failed (or to redo it), run "Update channels" by
   hand with the tag. Never delete the `updater` release (a pre-release titled "Update channels — do not delete"):
   installed apps read `https://github.com/shellfin-app/shellfin/releases/download/updater/stable.json`.
   `experimental.json` is a legacy mirror of `stable.json` for every install of the former experimental channel
   (the `0.2.0-experimental.1` build, and 0.1.0/0.1.1 installs set to "Experimental"), which still read
   `.../updater/experimental.json`; never delete it either, or those installs stop receiving updates.

### Repository settings checklist

Settings of the `shellfin-app/shellfin` repository that the files in this repository rely on. This is the intended
configuration, not a record of the live state: some items may still have to be configured, so check them in Settings
before relying on them. Nothing here can be committed.

- **Security:** enable "Private vulnerability reporting" (`SECURITY.md` links to `/security/advisories/new`). Make sure
  `security@shellfin.app` and `conduct@shellfin.app` (Code of Conduct reports) receive mail, for example as forwards.
  If GitHub push protection flags the throwaway keys in `src-tauri/tests/fixtures/keys/` on the first push, bypass it
  as "used in tests".
- **Community:** enable Discussions (issue template links and the first lines of this file point to it). Create the
  team `@shellfin-app/maintainers` with write access; otherwise `.github/CODEOWNERS` is silently ignored.
- **Branch ruleset** for `main` only: no deletion, no force push. Pull requests are merged with **Rebase and merge**
  (see [Branches](#branches)); the other merge methods may be turned off. Recommended, not required by the
  workflows: a pull request for every change and the single required status check `ci-ok`. It fails
  when any CI job failed or was cancelled; on a pull request it passes when the jobs passed or were skipped by change
  detection, on a push it needs every job to pass (a skipped job fails it). Pull requests skip the jobs their changes
  cannot affect (a documentation-only change runs only `changes`, `workflows` and `security`), so do not require the
  individual jobs.
- **Homebrew tap:** create the public, empty repository `shellfin-app/homebrew-tap`. Create a fine-grained personal
  access token (or a GitHub App token) with **Contents: read and write on that repository only**, and add it as the
  secret `HOMEBREW_TAP_TOKEN` of the `release` environment. `.github/workflows/homebrew-tap.yml` then pushes
  `Casks/shellfin.rb` when you publish a release, after verifying the attestation of `SHA256SUMS`; it creates the
  tap README on first use and never downgrades. Without the secret it only prints a notice. Once releases are
  notarized, run the renderer with `--notarized true` (edit the workflow step) to drop the "Open Anyway" caveat.
- **Retiring the experimental branch model (once):** remove `experimental` from the existing branch ruleset (or from
  its branch condition); delete `origin/experimental` and any other branch of the experimental model (the
  tag `v0.2.0-experimental.1` keeps its commits reachable); in the tap repository, delete
  `Casks/shellfin@experimental.rb` and its line in the README. Keep `experimental.json` on the `updater` release.
- **Tag protection:** a tag ruleset for `v*` (only maintainers create it; no deletion, no force update).
- **`release` environment** (Settings, Environments): deployment tags limited to `v*`. With a single maintainer it
  has no required reviewers (GitHub does not let you approve your own run once "Prevent self-review" is set); the
  gates are `verify`, the tag ruleset and publishing the draft by hand. When there is a second maintainer, add
  required reviewers with "Prevent self-review". Secrets of that environment: `TAURI_SIGNING_PRIVATE_KEY` and
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (below); optionally the Apple (`APPLE_*`), Azure (`AZURE_*`) and GPG (`GPG_*`)
  secrets listed further down.
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
3. Mark the menu bar mnemonics (see below).
4. Register the language: add it to `locales` in `i18next.config.ts`; to `SUPPORTED_LANGUAGES`,
   `LANGUAGE_ENDONYMS` and `LANGUAGE_STATUS` (`"beta"` for a new translation) in `src/i18n/index.ts`; to
   `bundle.windows.nsis.languages` in `src-tauri/tauri.conf.json` (the NSIS name, for example `German`,
   `PortugueseBR`, `SimpChinese`; Tauri ships the installer strings) and to `CFBundleLocalizations` in
   `src-tauri/Info.plist` (Apple's identifier: `zh-Hans` for `zh-CN`, `pt-BR` as is). `src/i18n/i18n.test.ts` fails until all of
   them agree. If the language needs a region or script rule (like `pt-*` -> `pt-BR`, `zh-Hant` -> none), extend
   `LOCALE_RULES` in `src/i18n/index.ts` with a test row. The language then appears in Settings > General > Language,
   "System" picks it when it is the OS language, and the macOS menu follows it.
5. Run `npm run i18n:check`, `npm run test` and `npm run lint`. The parity test `src/i18n/parity.test.ts` checks
   that your file has the same keys as English (plural-aware), the plural forms of the language, the same
   `{{tokens}}` and no empty strings. In dev builds the pseudo-locale `en-XA` (`?xa=1`, or `?xa=40` for 40 %
   longer text, about German) shows text that does not go through i18n or does not fit its layout.
6. Open a pull request describing the language and your confidence level; a second reviewer who speaks the
   language is appreciated. Attach screenshots at 580 px and 1280 px window width (Home, connect dialog, host key
   dialog, file tree context menu, settings, conflict dialog).

### Glossary

Write `docs/i18n-glossary-<lng>.md` before translating, after the pattern of `docs/i18n-glossary-uk.md`: **Style**
(tone, buttons and menu items, quotes, punctuation, units), **Plurals** (the categories below with examples),
**Terms** (one translation per concept: connection vs session, upload vs download, host key vs fingerprint,
passphrase vs password, bookmark, known hosts), **Keep in English** (SSH, SFTP, `known_hosts`, `ssh-agent`,
`ProxyJump`, `SHA256`, LF/CRLF, key type names, keyboard keys `Ctrl`/`Shift`/`Alt`, which stay English in every
language) and **Parameter families** (how the enum keys `entity.*`, `ioOp.*`, `timeoutOp.*`, `keyLoadReason.*`
read inside their host sentences, for example `errors.notFound`). Take OS terms from the platform's own
localization (Windows, macOS Finder / Terminal / Keychain Access, GNOME / KDE) and SSH terms from WinSCP, FileZilla
and VS Code Remote-SSH in your language.

### Plural categories

i18next uses the CLDR categories of `Intl.PluralRules` (Node 24): every key that English pluralizes needs exactly
the suffixes of your language, and the parity test fails on a missing or an extra one.

| Language | Suffixes |
|---|---|
| ja, ko, zh-CN | `_other` only: it must contain `{{count}}` where a number is meant |
| de, en | `_one`, `_other` |
| fr, es, pt-BR | `_one`, `_many` (millions: "1 000 000 de fichiers"), `_other`; in fr and pt `_one` also covers 0 |
| uk | `_one`, `_few`, `_many`, `_other` |

### Menu bar mnemonics

The six menu titles (`menubar.file`, `edit`, `view`, `connection`, `terminal`, `help`) open with Alt+letter. Without
a marker the first letter is the mnemonic (English, Ukrainian). Put `&` before another letter when two titles start
with the same one or the first letter is hard to type: `"Ay&uda"`, `"E&xibir"`. Write `&&` for a literal `&`.
Chinese, Japanese and Korean titles append the English letter in parentheses, the Windows convention:
`"ファイル(&F)"`, `"编辑(&E)"`, `"보기(&V)"`; the letters must be F, E, V, C, T, H as in English. The menu bar
underlines the marked letter; aria labels show the title without `&`, and the macOS menu drops the `(&F)` group
("ファイル"). `npm run i18n:check` (`scripts/menu-mnemonics-check.mjs`) fails on two markers in one title, a
trailing `&`, `&` before a space, a mnemonic that is not a letter or digit, a CJK title without an ASCII marker or
with another letter than English, and two titles with the same mnemonic (accents ignored: "Édition" and "Extras"
collide). The items inside the menus have no mnemonics.

### Beta and review

A new language ships with `LANGUAGE_STATUS` `"beta"`: the picker shows "(beta)" and a link to the translation
issues (label `i18n`). It becomes `"complete"` once a native speaker has reviewed it in a merged pull request.
Review checklist (for the translator, the review agent and human reviewers):

- Terms match the glossary; one term per concept across the whole file.
- OS concepts use the native OS terms (Settings, Keychain / Credential Manager, Trash, Downloads folder).
- Security texts are exact and not softened: host key changed, unknown or revoked, ignored `known_hosts`, agent
  warnings, "Trust and continue", update verification failures.
- Every `{{token}}` is kept; technical tokens are not translated.
- Plural forms read correctly for 0, 1, 2, 5, 21 and 1,000,000 (CJK: the `_other` text contains the number).
- Parameter families agree with their host sentences (case, gender).
- Menu titles: mnemonic marked, unique, ASCII for CJK.
- The ellipsis `…` exactly where English has one (the item opens a dialog).
- Menu titles, buttons, tab labels, status bar texts and column headers fit at 580 px.
- Punctuation and typography of the language (French spaces before `: ; ? !`, CJK full-width punctuation in prose,
  German quotes „…“, Spanish ¿ ¡).
- No HTML in values; nothing that a server sends is placed inside `<Trans>`.

### Fonts

No CJK font is bundled. The UI uses per-language system font stacks (`src/styles/tokens.css`, `:lang()`); on Linux
install Noto Sans CJK (`fonts-noto-cjk` on Debian/Ubuntu, `google-noto-sans-cjk-fonts` on Fedora), otherwise
Chinese, Japanese and Korean text shows empty boxes.
