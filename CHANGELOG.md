# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Importing bookmarks can now keep the startup commands from the file, but only when you explicitly choose to for that
  import. The bookmarks and their commands are listed before anything is saved. By default startup commands are still
  dropped, and a command that is not a single valid line is always dropped.
- Problems found in your SSH config are now shown: the Config card on the Home screen shows "N warnings" when there
  are any, and opens a list with the file, the line and what is wrong. Unreadable lines beyond the first 50 in a file
  are counted as "and N more" instead of being dropped silently.

### Removed

- The experimental update channel and its "Update channel" setting: there is one update channel, with stable
  releases only. A saved channel choice is ignored. Experimental builds (0.2.0-experimental.1) and 0.1.x installs set
  to "Experimental" move to the stable channel with the next release, 0.2.0, without any action. The Homebrew cask `shellfin@experimental` is no longer updated; switch
  with `brew uninstall --cask shellfin@experimental && brew install --cask shellfin-app/tap/shellfin`.

### Security

- Remote names that contain invisible variation selectors (U+FE00 to U+FE0D) are now shown with a visible marker,
  so two different names no longer look identical. The text and emoji presentation selectors (U+FE0E and U+FE0F)
  are shown with a marker too unless they really change how the preceding emoji looks, so emoji names keep working
  and names such as report.pdf and a report.pdf with a hidden selector stay distinguishable. "Copy path" copies such
  a path in its escaped, visible form.

## [0.1.1] - 2026-10-22

### Changed

- Connection tabs with the same name are told apart: the copies show "name (2)", "name (3)", and a closed number is
  reused. A tab you renamed keeps only its name.
- The update dialog shows the release notes with simple formatting (headings, lists, code, and links to the Shellfin
  repository and website).
- The bookmark dialog shows the User from your SSH config as the placeholder of the User field when the host is an
  SSH config host.

### Fixed

- Syntax colours in the editor now meet a 4.5:1 contrast on the selection and the current line in the light theme, and
  comments do in the dark theme.
- "Reconnect to apply changes" no longer opens a second connection when the same bookmark is opened from the sidebar
  while it reconnects, and a duplicated tab keeps its place, name and number.

## [0.1.0] - 2026-10-22

First release. Supported platforms: Windows 10/11 x64 and ARM64 (NSIS installer), Linux x64 (AppImage, deb,
rpm) and macOS 11+ on Apple Silicon (DMG). 32-bit Windows and Intel Macs are not supported.

### Connections

- Multiple SSH connections as tabs, direct or through `ProxyJump` chains of any length.
- "Test connection" before saving a bookmark.
- Automatic reconnect with backoff that keeps editor buffers and terminal scrollback; manual disconnect and reconnect;
  edits of a bookmark are applied to open connections where possible.
- Reopen the connections that were open at the last quit (setting, off by default).
- A second launch of Shellfin focuses the running window instead of starting another instance.
- Disconnect also stops a pending automatic reconnect. A flapping server cannot cause an endless reconnect or
  notification loop: attempts carry over until the connection has stayed up for 30 seconds, and a connection-lost
  notification is sent at most once a minute per connection.
- Agent keys that Shellfin added are removed first when quitting, even if a transfer or terminal is stuck.
- OS notifications, sent by the app and never by the web view, when a connection drops or transfers finish while the
  window is in the background (separate switches).

### Hosts and bookmarks

- `~/.ssh/config` support (`Include`, wildcards, negation, first-match-wins, a subset of `Match`, `ProxyJump`,
  `IdentityFile`, `IdentityAgent`, `known_hosts` options, including the `%k` token) with live reload. Config files with
  a UTF-8 byte-order mark are read, and a hostile config cannot flood the warnings list. Config hosts appear read-only next to your
  own bookmarks; a setting hides them or stops reading the file altogether.
- Own bookmarks with groups, tags, colours, drag-and-drop ordering, a start directory, per-bookmark bandwidth limits,
  "Confirm before connecting", a startup command (sent once to the first terminal) and notes. Deleting a group asks
  where its bookmarks go.
- Recent connections (clearable) and quick connect with `user@host:port`; a default user for quick connect and new
  bookmarks.
- Import (JSON) and export (JSON or OpenSSH config) from the File menu and the Bookmarks panel, with a summary of added,
  replaced, skipped and rejected entries and a choice to skip, replace or duplicate on name conflicts. Files are
  validated field by field. Passwords are never exported or imported; startup commands are exported but never imported
  (the summary names the affected bookmarks).

### Authentication

- Key files (including encrypted keys, RSA with SHA-2), ssh-agent (Windows OpenSSH agent, Pageant through its OpenSSH
  pipe, `SSH_AUTH_SOCK`; agent endpoints are owner-verified), password, and keyboard-interactive including 2FA.
- Host-key verification against OpenSSH `known_hosts`: an interactive decision for unknown hosts and a hard failure with
  no override for changed, revoked, CA-managed and reconnect-mismatched keys. A `known_hosts` file that exists but is
  ignored (untrusted owner, too large, unreadable) is reported, and new keys are then confirmed instead of being
  accepted silently; so are hosts already known under another key type.
- Encrypted keys with extreme key-derivation settings are refused instead of hanging the app.
- Passwords are stored in the OS keychain only, bound to the destination; a setting offers or suppresses saving them.

### Terminals

- Large pastes always arrive in order; input is sent as raw bytes with little overhead. On Linux, Ctrl+Shift+V and
  Shift+Insert paste exactly once. A "Screen reader support" setting turns on xterm's screen reader mode.
- Several terminals per connection (xterm.js with WebGL), flow control that never blocks the SSH session, find, save
  output, safe paste (control characters stripped, optional multi-line confirmation), links that need confirmation,
  "Open terminal here", restart after the shell ended.
- Terminal panel that can be maximized over the editor area (drag the splitter to the top, double-click it, the panel
  header button, View menu or command palette); restoring returns to the previous height and keeps editor tabs and
  unsaved changes.
- Settings: cursor style and blink, line height, letter spacing, minimum contrast ratio, word separators, scroll speed,
  scrollback of up to 1 000 000 lines, `TERM` type, folder of new terminals (the folder open in the file tree, falling
  back to home), opening a terminal on connect, what happens when the shell ends, visual or audible bell, right-click
  action, Option as Meta (macOS), mouse reporting for programs, hiding the pointer while typing, confirmation before
  closing running terminals.

### Files and transfers

- Lazy, virtualised SFTP tree; new file or folder, rename, move, delete (with optional confirmation); optional
  auto-refresh of expanded folders; folders first; owner, group and permissions in the tree; hints for files you
  probably cannot write.
- Transfers use pipelined SFTP requests (up to 64 × 32 KiB per transfer, within a per-connection memory bound), so
  they are much faster on high-latency links; a lost connection or a cancel is noticed within seconds, and failed
  transfers that can be retried survive Disconnect.
- Named pipes, sockets and devices are refused instead of hanging a session or filling the disk. Very large or slow
  directories show a partial listing instead of failing. Downloads of long multi-byte file names work.
- Multi-file upload and download (picker and OS drag and drop) with retry and replace. Parallel transfers per connection
  are configurable from 1 to 64 (default 3); a setting decides what happens when the target exists (ask, overwrite,
  skip). A download saved through the Save dialog is never skipped, because the dialog already confirmed the target.
- Optional global and per-bookmark bandwidth limits; keep file timestamps on upload and download; binary (KiB) or
  decimal (KB) sizes; relative or absolute dates; double-click opens or downloads.
- Downloads get Mark-of-the-Web (Windows) or quarantine (macOS) marks.

### Editor

- CodeMirror 6 with language support, encodings and line-ending handling, and atomic, conflict-safe saves that preserve
  mode and ownership, with a conflict dialog. A same-size edit made on the server within the same second is detected
  by its content hash. Saves requested while offline run after reconnecting.
- Files of up to 256 MiB (setting). Options: line numbers, active-line highlight, indentation detection and tab size,
  trim whitespace and final newline on save, line endings of new files, auto-close and matching brackets, show
  whitespace, code folding, font and font ligatures (off by default).

### Appearance and settings

- Dark, light or system theme (the light theme is checked for WCAG contrast), six accent colours, interface text size
  from 12 to 16 px, reduce motion, a terminal background (follow the theme, a swatch or any `#RRGGBB`) that switches the
  terminal text palette by the background's brightness, and configurable fonts with live previews. Changes preview live
  in the Settings dialog.
- A fixed-size Settings dialog with one section at a time and one setting per row; short help text with longer
  explanations behind a (?) button; "Reset all settings".
- Sidebar that can be dragged up to 80% of the window width.
- A damaged settings file loses only the bad values: every other setting is kept and the original is set aside.
- Log level, with debug logging on demand. The log directory is size-capped, and server text cannot forge log lines.

### Desktop UX

- Frameless window with an in-app menu bar (native menu on macOS), command palette, one action registry,
  layout-independent keyboard shortcuts and a Keys dialog, non-blocking toasts.
- Confirm before quitting (always, with open connections, or never; unsaved editor changes always ask).
- Escape closes dialogs from the innermost widget outwards. Toasts pause while hovered or focused, and errors stay for
  20 seconds. The interface language is applied before the first paint, so Ukrainian never flashes English.

### Updates

- Signed updates for Windows (NSIS, passive install), macOS and the Linux AppImage. Shellfin checks about 15 seconds
  after startup and then once a day (Settings > General > "Check for updates automatically", on by default), and on
  demand with Help > "Check for updates…" (also in the command palette and the macOS app menu).
- The update dialog shows the new version and its release notes and offers "Update and restart", "Later" and "Skip this
  version". Restarting goes through the normal quit confirmation, so unsaved changes are never lost. Every download is
  verified against the Shellfin release key before it is installed. `.deb` and `.rpm` installs get a "Download" button
  for the release page instead.
- Homebrew on Apple Silicon: `brew install --cask shellfin-app/tap/shellfin` (stable) or `shellfin@experimental`;
  the casks are updated automatically when a release is published.
- An "Update channel" setting chooses Stable or Experimental (pre-releases, plus stable releases when they are newer);
  switching back to Stable never installs an older version.

### Localization

- English and Ukrainian UI with a language switcher (default: the system language), lazy-loaded locale files,
  locale-aware number and date formatting, localized macOS menu, a Ukrainian glossary, a translation parity test, a
  lint-enforced "no hard-coded text" rule and a development pseudo-locale.

### Security

- Least-privilege Tauri ACL (an explicit command allow-list, no plugin permissions for the web view), strict content
  security policy, frozen `Object.prototype`, locked navigation, WebView2 hardening (no native context menu, browser
  accelerators or zoom; clipboard read only for the app origin).
- A local path allowlist and file-trust checks; the web view never supplies local file paths. Untrusted server text is
  escaped; secrets stay in the Rust process and are zeroised.
- Third-party licence notices ship with the app (`THIRD_PARTY_NOTICES.txt`), and the installer shows both licences of
  the dual licence.

### Engineering

- TypeScript bindings generated from Rust DTOs with a drift check; unit, Docker-based integration and frontend tests.
- CI for formatting, lint, types, tests, the minimum Rust version, workflow linting, secret scanning, dependency audits
  and builds on all supported platforms.
- A protected release workflow that requires a green CI run for the tagged commit, builds with a pinned toolchain,
  produces checksums, SBOMs and build provenance attestations, and creates a draft release; guards keep the mock backend
  and the test-only `e2e` feature out of release builds.
- Dual licensing under MIT OR Apache-2.0, contribution guide, security policy, code of conduct and `AGENTS.md`
  onboarding for AI coding agents.

[Unreleased]: https://github.com/shellfin-app/shellfin/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/shellfin-app/shellfin/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/shellfin-app/shellfin/releases/tag/v0.1.0
