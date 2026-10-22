# AGENTS.md

Instructions for AI coding agents (Claude Code, Codex, Copilot, Cursor, Gemini CLI, ...). Humans: see
[CONTRIBUTING.md](CONTRIBUTING.md). Nested files `src-tauri/AGENTS.md` and `src/AGENTS.md` add area-specific rules;
the closest file to the code you edit wins, and an explicit user instruction wins over all of them.

## What this is

**Shellfin** is an open-source, cross-platform desktop SSH client: connections as tabs, terminals (xterm.js), a
remote SFTP file tree, a built-in editor (CodeMirror 6) with conflict-safe saves, bookmarks plus read-only
`~/.ssh/config` hosts, host-key verification against OpenSSH `known_hosts`, secrets in the OS keychain.
Stack: Tauri 2, Rust (edition 2024, MSRV 1.90, russh with `ring`), React 19 + TypeScript + Vite. Licence
MIT OR Apache-2.0. Targets: Windows x64 and ARM64, Linux x64, macOS Apple Silicon (no 32-bit Windows, no Intel Mac).
Repository: `shellfin-app/shellfin`, site <https://shellfin.app>, app id `app.shellfin.desktop`. The UI ships in English (default, fallback) and Ukrainian.

## Repo map

| Path | What |
|---|---|
| `src-tauri/src/` | Rust core. `commands/` thin Tauri commands (+ `names.rs` command list), `ssh/` connect/auth/agent/prompts, `session/` supervisor, state machine, reconnect, `terminal/`, `sftp/` (listing, save, transfer, throttle), `sshconfig/`, `known_hosts/`, `storage/` (bookmarks/settings/recents JSON), `platform/` (local path allowlist, file trust, Win32 security), `ipc/dto.rs` (all DTOs), `error.rs`, `updater.rs` (signed self-update), `secrets.rs` (OS keychain), `watcher.rs` (ssh config watcher), `logging.rs`, `notify.rs`, `external_link.rs`, `app_state.rs`, `window.rs`, `quit.rs`, `app_menu.rs`, `e2e.rs` |
| `src-tauri/tests/` | Integration tests (`it_*.rs`, Docker sshd in `tests/sshd/`), memory tests, throwaway key fixtures |
| `src-tauri/capabilities/main.json` | The one capability: event listen/unlisten plus one `allow-<command>` per app command |
| `src/features/` | UI by feature: `shell`, `hosts`, `files`, `editor`, `terminal`, `session`, `transfers`, `palette`, `keys`, `settings`, `menu`, `actions`, `updates`, `welcome` |
| `src/ipc/` | `backend.ts` (interface), `tauriBackend.ts`, `mock/` (MockBackend, scenarios, FakeFs), `bindings/` (**generated**) |
| `src/state/`, `src/lib/`, `src/components/`, `src/i18n/` | Zustand slices, pure helpers, shared components, i18next + `locales/{en,uk}/translation.json` |
| `scripts/`, `vite-plugins/` | Build-time guards (`assert-no-mock.mjs`, `freeze-check.mjs`, `bindings-check.mjs`, `menu-mnemonics-check.mjs`, `freezeSafe.ts`), the woff-dropping font plugin (`woff2Only.ts`), the licence notice generator (`notices.mjs`), the updater manifest tool (`updater-manifest.mjs`) and the Homebrew cask generator (`homebrew-cask.mjs`) |
| `THIRD_PARTY_NOTICES.txt`, `src-tauri/installer/` | **Generated** licence notices shipped with the app and the installer licence page; `npm run notices` regenerates them, never hand-edit |
| `assets/brand/` | Logo and brand images; `docs/` architecture, smoke-run notes and the Ukrainian glossary (`i18n-glossary-uk.md`) |

Deep reference: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) (§4 IPC, §5 connection lifecycle, §6 SFTP/editor,
§7 storage, §8 frontend, §9 security, §10 testing, §11 build/release). Read the relevant section before changing
that area. Security policy: [SECURITY.md](SECURITY.md).

## Setup and gates

Prerequisites: Rust (MSRV 1.90; `src-tauri/rust-toolchain.toml` pins the compiler CI uses), Node.js >= 24.22.1 (24 LTS,
`.nvmrc`), Tauri platform dependencies (see README). Then `npm ci`.

```sh
npm run tauri dev          # app with hot reload (Vite dev server on port 5173)
npm run dev                # frontend only, in a browser, against MockBackend (?scenario=<name>)
```

Run **all** gates before you finish; CI runs the same ones plus a few that need extra tools (`cargo check` on the
MSRV, `actionlint`, `zizmor`, `gitleaks`, `cargo deny`, `cargo audit`, `npm audit`, the e2e marker build). Run `cargo`
from `src-tauri/` (the ts-rs environment in `src-tauri/.cargo/config.toml` is relative to it).

```sh
cd src-tauri
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --all-targets --features it -- -D warnings
cargo clippy --locked --all-targets --features e2e -- -D warnings
cargo test --locked                       # unit tests, ACL test, regenerates ts-rs bindings
cargo test --locked --features e2e --lib e2e::   # e2e isolation tests
cargo test --locked --features it         # Docker integration tests (needs Docker; starts tests/sshd/compose.yml)
docker compose -p shellfin-it -f tests/sshd/compose.yml down -v    # cleanup afterwards
cd ..
npm run lint               # ESLint, zero warnings
npm run typecheck
npm run test               # Vitest
npm run i18n:check         # translation keys match the code, menu bar mnemonics are unique
npm run notices:check      # THIRD_PARTY_NOTICES.txt matches the lockfiles (needs cargo-about, see CONTRIBUTING)
npm run build              # tsc + production bundle + "no mock backend in the bundle" check
npm run test:freeze        # bundle works with Object.prototype frozen (Tauri freezePrototype)
npm run bindings           # regenerate src/ipc/bindings after any DTO change; commit the result
```

`npm run rust:check` = fmt + clippy + test. `npm run bindings:check` = regenerate and fail on a diff, an untracked
binding or an orphan. After any dependency change run `npm run notices` and commit the regenerated notices.
The Docker integration suite is only needed when you touch `ssh/`, `session/`, `terminal/`, `sftp/`.

## Hard rules

1. **Always through i18n.** No literal user-facing strings in `src/features`, `src/components`,
   `src/state` (ESLint `i18next/no-literal-string` enforces it). Add keys to `src/i18n/locales/en/translation.json`
   **and** `src/i18n/locales/uk/translation.json` (terms: `docs/i18n-glossary-uk.md`; `src/i18n/parity.test.ts` checks
   keys, plural forms and `{{tokens}}`), run `npm run i18n:extract` then `npm run i18n:check`. Backend errors are
   codes plus params, never prose.
2. **IPC types are generated by ts-rs.** Never hand-edit `src/ipc/bindings/**`; change the DTO in
   `src-tauri/src/ipc/dto.rs`, run `npm run bindings`, commit the output. CI has a drift check.
3. **The webview never supplies local filesystem paths.** Files come from native dialogs or OS-drop tokens
   (`DropRegistry`) handled in Rust; every local open goes through `platform::local_path`.
4. **Remote and server-controlled strings** (file names, banners, prompts, titles, error details) are rendered only
   through `displayName` / `displayPath` / `displayServerText` (`src/lib/serverText.ts`). Never `innerHTML`,
   `dangerouslySetInnerHTML`, `<Trans>` with server data (ESLint blocks the first two).
5. **Keep the CSP strict** (no inline scripts, no new origins). `freezePrototype` stays on; xterm is made
   compatible by the `freezeSafe` Vite plugin. Do not turn either off to fix a crash.
6. **No `e2e` feature and no MockBackend in release builds.** The marker checks in CI and `npm run build` exist for
   this; do not weaken them.
7. **Least-privilege ACL.** Every new command needs: an entry in `src-tauri/src/commands/names.rs`, the handler in
   `commands/mod.rs` (`app_commands!`), the permission in `capabilities/main.json`, the frontend wrapper and test
   entry, and the ACL test (`commands::acl_tests`) must pass. Never add `core:window:*`, `fs:`, `shell:`,
   `dialog:` or `opener:` permissions to the webview.
8. **Security-sensitive areas need tests, including hostile input:** `ssh/`, `known_hosts/`, `sftp/save` and
   `sftp/transfer`, `platform/local_path` and `file_trust`, `window.rs` and the ACL, `storage/`. Never log secrets,
   terminal data or file contents. No `unwrap()`/`expect()` in non-test Rust, no `std::process::Command`.
9. **Edit files with editor tools**, not ad-hoc `sed`/Python/PowerShell one-liners. Never write code containing
   backslashes (regexes, Windows paths) inside a shell heredoc.
10. **No personal data or local paths in tracked files** (user names, home directories, machine names, tokens).
    Use `example.com` and similar placeholders. Process material (reviews, plans, agent logs) stays out
    of git; keep such files in a local-only folder listed in `.git/info/exclude`.
11. **Conventional Commits** (`feat(sftp): ...`, `fix(ui): ...`). Do not commit unless the user asks.
12. Dependencies are pinned to exact versions with committed lockfiles; adding one needs a justification and
    `cargo deny` must stay green.

## Gotchas

- **HTML5 drag and drop does not work on Windows** (Tauri's OS drop handler replaces WebView2's drop target).
  In-app drag uses pointer events (`src/features/hosts/pointerDrag.ts`). OS file drops arrive as `drop:*` events.
- The Vite dev server uses port **5173** (`strictPort`); `devUrl` in `src-tauri/tauri.conf.json` must match.
- **Never open UI (dialogs, menus, palette) as a side effect of focus changes**; focus restoration then loops.
  Open UI only from explicit user actions.
- **Terminal shortcut passthrough:** in a focused terminal on Windows/Linux the shell owns almost every key. Only
  `Ctrl+Shift+K` (palette), `Ctrl+Shift+F` (find), `Ctrl`+backquote and `Ctrl+Shift`+backquote (terminal panel),
  `Ctrl+1..9` and the clipboard keys reach the app. Shortcuts match layout-independently (`normKey`) and must never fire on AltGr typing. See
  `src/lib/keybindings.ts` and ARCHITECTURE §8.7 before adding a shortcut.
- Terminal output must never block the SSH session loop: the reader always drains, the backlog is bounded
  (`terminal/flow.rs`, ARCHITECTURE §4.5).
- The window is frameless with an in-app menu bar; there is no native context menu or browser accelerator
  (guard script + WebView2 settings). All menu/palette/shortcut behaviour goes through the action registry
  (`src/features/actions/registry.ts`, ids in `src/lib/actionIds.ts`).
- A Linux or macOS build of the Rust security code is not a substitute for Windows: ACL/path code only runs on
  Windows (CI `unit-native` job covers it).

## Definition of done

All gates above are green; new behaviour has tests; user-facing text is in `translation.json`; bindings are
regenerated when DTOs changed; `docs/ARCHITECTURE.md` is updated when a decision changed (it is binding);
`CHANGELOG.md` `[Unreleased]` is updated for user-visible changes; no secrets or personal data were added.
