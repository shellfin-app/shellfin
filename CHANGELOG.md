# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- The file tree can show size and modification date columns and sort by name, size or date. The columns appear when
  the sidebar is wide enough (size first, then the date), with a header to sort by clicking; the View menu and the
  command palette sort too. Folders stay first, and the choice is remembered. The date follows the date format setting
  in a short form, and the columns can be turned off in Settings → Files.
- German user interface (beta), including the Windows installer. Settings > General > Language lists it as
  "Deutsch (beta)"; "System" picks it on a German system. Wording problems can be reported through the link below the
  language setting.
- Japanese user interface (beta), including the Windows installer. Settings > General > Language lists it as
  "日本語 (beta)"; "System" picks it on a Japanese system. On Linux, install Noto Sans CJK (`fonts-noto-cjk`) for the
  Japanese text to show. Wording problems can be reported through the link below the language setting.
- Importing bookmarks can now keep the startup commands from the file, but only when you explicitly choose to for that
  import. The bookmarks and their commands are listed before anything is saved. By default startup commands are still
  dropped, and a command that is not a single valid line is always dropped.
- Problems found in your SSH config are now shown: the Config card on the Home screen shows "N warnings" when there
  are any, and opens a list with the file, the line and what is wrong. Unreadable lines beyond the first 50 in a file
  are counted as "and N more" instead of being dropped silently.
- An error details dialog with a "Report on GitHub…" button. It opens by itself only for unexpected failures (and
  a bug inside Shellfin is now reported as one instead of leaving an action hanging); other errors offer it through
  "Details…", and Help → "Report a problem…" opens it without an error. It shows exactly the report that will be
  copied: the error, your Shellfin version, operating system, architecture and installation type, and, only if you
  tick the box, the last lines of the current log. Host names, IP addresses, user names, your home folder and
  remote paths are always replaced with placeholders. Nothing is sent automatically: the button copies the report
  and opens a pre-filled GitHub bug form in your browser for you to review and submit.
- `User` in `~/.ssh/config` now expands `%` tokens and `${ENV}` like OpenSSH 10 (`%u`, `%L`, `%l`, `%h`, `%n`, `%p`,
  `%k`, `%d`, `%i`, `%j`, `%%`; for example `User %u-admin`). An expanded user that is not a safe user name, or one
  that cannot be expanded, shows a warning, and connecting to that host asks you to set a user instead of trying your
  local user. A user you entered for the connection is used as is.
- File options in the SSH config (`UserKnownHostsFile`, `GlobalKnownHostsFile`, `IdentityFile`, `CertificateFile`,
  `IdentityAgent`) now support `%C` (the OpenSSH 9.6+ connection hash), `%l`, `%L`, `%i` and `%j`, so setups such as
  `UserKnownHostsFile ~/.ssh/known_hosts.d/%C` work.
- Windows 11 Snap Layouts on the maximize button: hovering it shows the snap layout flyout, like in other Windows
  apps (#14). Dialogs and menus stay clear of that corner, so their close buttons and items remain clickable in a
  small window.

### Changed

- The Windows installer is now also in Ukrainian; it follows the Windows display language.
- The menu bar switches to the single Menu button as soon as the menu names do not fit in the title bar (longer
  translations, a larger UI font size), not only below 960 px.
- Bookmarks with the same position are sorted by name in the UI language instead of the system's.
- The window can be made much smaller (minimum 580 × 500 instead of 960 × 600), so it fits half of the screen in
  Snap layouts on laptops with display scaling. Below 960 px wide the menu bar becomes one Menu button (same
  menus, same Alt/F10 keys), the window controls always stay at the right edge, and the status bar hides the
  transfer rates and the key type below 760 px.
- Editor whitespace markers (space dots and tab arrows) are now drawn faintly in both themes instead of in a colour
  close to the text, and "Show whitespace" has three choices: All, Active line only and None (the default stays
  None). A saved on/off choice carries over as All or None.
- In the editor, other occurrences of the selected text and the current search match are outlined instead of
  filled, so they no longer look stronger than the selection or make the text on it hard to read.
- On Windows, when no user is set for a connection (in the bookmark, the quick connect text or the SSH config),
  Shellfin now logs in with your local account name in lowercase (for example `smith` instead of `Smith`), as the
  OpenSSH client included with Windows does. A user you set yourself is used exactly as written.
- `%h` in SSH config file options and in `User` is now the host name in lowercase (and an IP address in its
  standard form), as in OpenSSH. For example, `UserKnownHostsFile ~/.ssh/kh/%h` with `HostName DB.Example.COM` now
  reads `~/.ssh/kh/db.example.com`.
- `Host` patterns in the SSH config are now case-sensitive, as in OpenSSH: `Host Lab` applies when you connect to
  `Lab`, but no longer to `lab` or `LAB`. `Match user` and `Match localuser` are case-sensitive too, while
  `Match host` and `Match originalhost` still ignore case. Hosts listed under `~/.ssh/config` connect as before;
  `Host Lab` and `Host lab` are now listed as two hosts. This applies to bookmarks too: a bookmark with the host `DB`
  no longer gets the settings of `Host db`. The bookmark dialog now warns when the host differs from a
  `~/.ssh/config` host only in case. Check your config and bookmarks if they relied on a different case.
- Saved passwords may be asked for once more after this update: on Windows, for bookmarks without a user of their
  own (the password was saved for `Smith`, the connection now uses `smith`) and for SSH config hosts (the keychain
  entry `sshconfig:Smith@host:22` is now looked up as `sshconfig:smith@host:22`), and for bookmarks whose SSH config
  settings no longer apply because of the case change above. Enter the password and save it again; the old keychain
  entry is not used any more.

### Removed

- The experimental update channel and its "Update channel" setting: there is one update channel, with stable
  releases only. A saved channel choice is ignored. Experimental builds (0.2.0-experimental.1) and 0.1.x installs set
  to "Experimental" move to the stable channel with the next release, 0.2.0, without any action. The Homebrew cask `shellfin@experimental` is no longer updated; switch
  with `brew uninstall --cask shellfin@experimental && brew install --cask shellfin-app/tap/shellfin`.

### Fixed

- On Windows, saving settings, bookmarks, recents or an exported file no longer fails with "Access is denied" when
  another program (an antivirus scan, the search indexer, a sync client, an editor) has the file open for a moment.
  The save waits up to about a second for the file to be released.
- Typing with an input method (Japanese, Chinese, Korean): Enter that confirms a conversion no longer renames a tab,
  creates a file or folder, or jumps to the next terminal search match, and Escape that cancels a conversion no
  longer closes the dialog, the command palette, the find bar or the rename field. Keys used by the input method no
  longer trigger shortcuts or open a menu.
- Selected text in the editor is clearly visible again: the current-line highlight no longer covers the selection.
  The selection is a tint of the accent colour, in both themes, with or without focus. To keep syntax colours readable
  on it, comments are a little lighter in the dark theme (and purple slightly lighter), and the light theme's syntax
  colours are slightly darker.

### Security

- Native dialogs, OS notifications and the file names of downloads now remove the same invisible characters as
  the app's own UI (for example the annotation characters U+FFF9 to U+FFFB, soft hyphens and tag characters) from
  server-provided names, so they cannot hide or reorder text there (#33).
- Host keys are now checked against the same known_hosts files OpenSSH uses when `UserKnownHostsFile` contains `%C`
  or `%l` (for example `~/.ssh/known_hosts.d/%C`). Before, Shellfin read the default files for such hosts and asked
  before trusting a key. If the file name still cannot be worked out exactly as OpenSSH does (no usable local host
  name, `CanonicalizeHostname` is on, an IP address written in a short form such as `127.1` or an IPv6 address with an embedded IPv4
  address such as `::ffff:1.2.3.4`, or on Windows a domain
  account or a local account name with non-ASCII characters), Shellfin keeps reading the default files, shows a
  warning, and asks instead of trusting a new key silently.
- Remote names that contain invisible variation selectors (U+FE00 to U+FE0D) are now shown with a visible marker,
  so two different names no longer look identical. The text and emoji presentation selectors (U+FE0E and U+FE0F)
  are shown with a marker too unless they really change how the preceding emoji looks, so emoji names keep working
  and names such as report.pdf and a report.pdf with a hidden selector stay distinguishable. "Copy path" copies such
  a path in its escaped, visible form.
- The SSH key arithmetic libraries are updated to crypto-bigint 0.7.8 and ctutils 0.4.5, which remove timing side
  channels in operations meant to run in constant time (RUSTSEC-2026-0387, RUSTSEC-2026-0385).

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
