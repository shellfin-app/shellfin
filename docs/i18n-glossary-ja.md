# Japanese glossary (ja)

Reference for `src/i18n/locales/ja/translation.json`. Keep new strings consistent with it; change a term here first,
then everywhere it is used. Terms follow the Japanese Windows and macOS UI (エクスプローラー, Finder, ターミナル,
キーチェーンアクセス), the Microsoft Japanese style guide, VS Code (Remote-SSH) and the Japanese WinSCP / FileZilla /
PuTTY-jp / Tera Term localizations.

## Style

- Messages, hints and questions are polite **です/ます** sentences that end with `。` (or `？` for a question):
  「接続が切断されました。」, 「変更を保存しますか？」. Never plain だ/である.
- Labels, column headers, setting names, tab titles, status texts and section names are **noun style**, without a
  final `。`: 「接続のタイムアウト（秒）」, 「名前」, 「サイズ」, 「更新日時」.
- Buttons and menu items are a noun or a verb phrase in dictionary form (Windows style): 「保存」, 「接続」,
  「キャンセル」, 「閉じる」, 「接続を閉じる」, 「ファイルをアップロード…」. Not 「保存します」.
- Toggle commands: 「Xの表示/非表示」 (「サイドバーの表示/非表示」).
- The ellipsis `…` (U+2026) stays exactly where English has it (the item opens a dialog), with no space before it.
- Punctuation in Japanese prose is full-width: `。` `、` `（）` `「」` `：` `？`. No `！` in UI text.
  - Quotes: 「…」 for UI element names, examples and literal values in prose (「接続を編集」, 「prod (2)」,
    「.」); 『…』 only for a quote nested inside 「…」.
  - Parentheses: full-width （） around Japanese text and after Japanese text (「接続のタイムアウト（秒）」,
    「{{name}}（コピー）」). Purely technical strings keep ASCII: `{{name}} ({{n}})`, `{{file}}:{{line}}`,
    `user@host:port`, `#RRGGBB`.
  - Colon: full-width `：` with no space after a Japanese label (「所有者：{{owner}}」, 「エンコード：{{encoding}}」).
- **Spacing** (Microsoft style): one half-width space between Japanese text and Latin letters, digits or a
  `{{token}}` (「SSH 設定」, 「{{count}} 件のブックマーク」, 「{{name}} を開いています…」), but no space next to full-width
  punctuation (「（{{count}}）」, 「「{{name}}」」, 「{{reason}}。」) and none between Japanese characters. A token that is
  always replaced by Japanese text (the enum families below: `{{what}}`, `{{op}}`) is joined without a space
  (「{{what}}が見つかりませんでした。」). Katakana compounds are
  written without spaces (ステータスバー, コマンドパレット, ファイルツリー, ホストキー).
- Long vowels follow the Microsoft rule: フォルダー, サーバー, ユーザー, エディター, マネージャー, フィルター,
  ブラウザー; but ディレクトリ, メモリ, エントリ, ライブラリ as in Windows.
- Numbers, sizes and dates come from `Intl` (`i18n/format.ts`); never write them into the text. Units after a number
  keep a half-width space: 「5 秒」, 「12 px」, 「1 GiB/s」.
- Seconds are 秒 in text (「{{seconds}} 秒後に再接続」); setting labels with "(s)" become 「（秒）」.
- A relative or absolute time at the end of a label follows a full-width colon, because `{{time}}` may be 「今」,
  「5 分前」 or a date: 「サーバーに保存：{{time}}」, 「更新：{{time}}」. Inside a sentence with an absolute date use
  「{{date}} にリリース」 / 「{{time}} に切断」.
- Keep strings short: CJK glyphs are about two Latin widths, so a Japanese label is roughly as wide as the English one
  (see "Length").

## Plurals

Japanese has one CLDR category: only `_other`, for every key that English pluralizes. The `_other` text must contain
`{{count}}` wherever a number is meant, and must read correctly for 1 (「1 件のブックマーク」 is fine). An English `_one`
text without a number ("Close terminal", "this file") becomes count-bearing or count-free wording that also fits 1.

Counters (助数詞), one per noun, always with a half-width space after the number:

| Counter | Used for |
| --- | --- |
| 件 | bookmarks, entries, warnings, transfers, connections, errors, hosts, includes, matches, results |
| 個 | files, terminals, keys |
| 行 | lines |
| 回 | attempts |
| 秒 | seconds |

## Terms

| English | Japanese | Notes |
| --- | --- | --- |
| connection (noun) | 接続 | 「新しい接続…」, 「接続を閉じる」 |
| connect / connecting | 接続 / 接続中 | button 「接続」; step 「{{host}} に接続中」 |
| disconnect / disconnected | 切断 / 切断済み | 「切断」, state 「切断」 |
| reconnect / reconnecting | 再接続 / 再接続中 | 「今すぐ再接続」, 「{{seconds}} 秒後に再接続」 |
| confirm before… (setting) | …前に確認 | 「接続前に確認」 |
| session | セッション | |
| bookmark | ブックマーク | 「マイブックマーク」 |
| group (of bookmarks) | グループ | |
| host / host name | ホスト / ホスト名 | |
| jump host (ProxyJump) | 踏み台ホスト | the term Japanese ops teams use; ProxyJump itself stays in English. Not ジャンプホスト |
| hop (in a jump chain) | ホップ | 「ホップ {{hop}}」 |
| host key | ホストキー | Microsoft キー; not ホスト鍵 |
| known hosts | 既知のホスト | file name `known_hosts` stays as is |
| fingerprint | フィンガープリント | |
| private / public key | 秘密キー / 公開キー | Microsoft wording, consistent with ホストキー |
| key file | キーファイル | |
| passphrase | パスフレーズ | never パスワード for a key |
| password | パスワード | |
| authentication | 認証 | 「認証中」 |
| login prompt / server prompt | ログインプロンプト / サーバーからの入力要求 | |
| agent | エージェント | `ssh-agent` stays as is |
| system keychain | システムの資格情報ストア | product names (Keychain, Credential Manager, Secret Service) stay in English |
| SSH config | SSH 設定 | `~/.ssh/config` as is |
| upload | アップロード | always local → server |
| download | ダウンロード | always server → local |
| load (into memory) | 読み込み | 「読み込み中…」, 「キーを読み込めませんでした」 |
| transfer(s) | 転送 | panel title 「転送」 |
| speed limit | 速度制限 | 「アップロードの速度制限」 |
| port forwarding | ポートフォワーディング | |
| settings | 設定 | Settings > General: 「設定 > 全般」 |
| file tree | ファイルツリー | |
| folder / directory | フォルダー / ディレクトリ | フォルダー in UI; ディレクトリ where English says directory |
| home folder | ホームフォルダー | `~` |
| symbolic link | シンボリックリンク | |
| permissions / owner / group | パーミッション / 所有者 / グループ | Unix permissions on the server (WinSCP ja) |
| file manager | ファイルマネージャー | |
| clipboard | クリップボード | |
| editor | エディター | |
| encoding | エンコード | VS Code wording |
| line endings | 改行コード | LF, CRLF stay as is |
| whitespace | 空白文字 | |
| code folding / fold / unfold | コードの折りたたみ / 折りたたむ / 展開 | |
| sort ascending / descending | 昇順 / 降順 | 「名前で並べ替え」 |
| terminal | ターミナル | |
| shell | シェル | |
| output | 出力 | |
| tab | タブ | |
| sidebar / status bar / activity bar | サイドバー / ステータスバー / アクティビティバー | |
| command palette | コマンドパレット | |
| keyboard shortcuts | キーボードショートカット | |
| default | 既定 | 「システムの既定」 (Windows wording; not デフォルト) |
| timeout | タイムアウト | |
| keepalive | キープアライブ | |
| match (search) | 一致 | 「次の一致」 |
| match case / regular expression | 大文字と小文字を区別 / 正規表現 | |
| overwrite / replace | 上書き / 置換 (text), 置き換え (files) | |
| skip | スキップ | |
| discard (changes) | 破棄 | 「保存しない」 in the close dialog (Windows wording) |
| dismiss (notice) | 閉じる | |
| notification | 通知 | |
| update (noun) / refresh | アップデート / 更新 | 「アップデートを確認…」; 「更新」 reloads a list |
| release / release notes | リリース / リリースノート | 「新機能」 for "What's new" |
| restart | 再起動 | |
| quick connect | クイック接続 | |
| start screen / Home | スタート画面 / ホーム | |
| start directory / startup command | 開始ディレクトリ / 起動時のコマンド | |
| log / log level | ログ / ログレベル | |
| scrollback | スクロールバック | |
| unsaved changes | 未保存の変更 | |
| trust | 信頼 | 「信頼して接続」 |
| colours | 青, 灰色, 緑, オレンジ, 紫, 赤, ティール, 黄, ピンク | |

## Keep in English

SSH, SFTP, ProxyJump, ProxyCommand, Include, Match, ssh-agent, ssh-add, sshd, known_hosts, `~/.ssh/config`, key type
names, SHA256, LF/CRLF, VPN, DNS, PowerShell, OpenSSH, Shellfin, JetBrains Mono, WCAG, ANSI, POSIX, TERM,
`xterm-256color`, tmux, vim, Emacs, keyboard keys (**Ctrl, Shift, Alt, Option, Enter, Esc, Del, F2, F6**; decision 6
of #17), `user@host:port`, paths, commands (`reset`), URLs, GitHub, and the language's own name in the language picker
(「日本語」).

## Menu bar mnemonics

ファイル(&F) / 編集(&E) / 表示(&V) / 接続(&C) / ターミナル(&T) / ヘルプ(&H): the Windows convention, a Latin letter in
parentheses with the English letters F, E, V, C, T, H (`npm run i18n:check` enforces both). The macOS native menu and
plain-title places (shortcuts dialog headings, aria labels) drop the `(&X)` group (「ファイル」). Items inside the
menus have no mnemonics.

## Length

CJK glyphs are wide; keep tight labels short:

- File tree column headers: 「名前」, 「サイズ」, 「更新日時」 (`files.column.*`).
- Status bar: 「行 {{line}}、列 {{col}}」 (VS Code), state labels are short nouns (「接続中」, 「切断」).
- Palette placeholders: short noun lists (「ホスト、ファイル、コマンド…」). Put the essential part of a placeholder
  first, so a cut-off still shows it (Quick connect: 「user@host:port、ブックマーク名」).
- Buttons and titles with a count put it in parentheses at the end (「ターミナルを閉じる（{{count}} 個）」).
- Settings dropdown values: noun phrases (「前回の接続を開く」, 「スタート画面を表示」).
- Prefer nouns over clauses in labels (「ファイルが既にある場合」).

## Parameter families

Japanese is head-final, so tokens usually move to the front of the sentence; check each key in the code before
writing it. The enum families read inside their host sentences:

- `entity.*` → bare noun before a particle in `errors.notFound` 「{{what}}が見つかりませんでした。」 (ブックマーク, グループ,
  SSH 設定のホスト, 転送).
- `ioOp.*` → noun phrase (…の読み込み / 書き込み / 保存 / 起動) before 中 in `errors.io`
  「{{op}}中にエラーが発生しました。」.
- `timeoutOp.*` → noun phrase inside full-width parentheses in `errors.timeout` 「操作がタイムアウトしました（{{op}}）。」
  (接続, フォルダーの一覧取得, 応答待ち, SFTP 要求).
- `keyLoadReason.*` → clause without a final 。 after the colon in `errors.keyLoadFailed`
  (「ファイルが破損しています」).
- `importInvalidReason.*` → clause without 。 inside parentheses in `errors.importInvalidFile`.
- `importInvalidCode.*` / `exportSkipCode.*` → predicate that starts with a particle, after a field name in `<code>`
  and a space (「`host` がありません」, 「`notes` はこの形式では保存できません」).
- `authMethod.*` → nouns, joined by `Intl.ListFormat` (「、」).
- `editor.cm.*` → CodeMirror phrases; keep the `$` placeholder (「$ 件の一致を置換しました」). `editor.cm.to` joins a
  line range (「3 ～ 7」), `editor.cm.onLine` precedes a line number (「行 5」).
