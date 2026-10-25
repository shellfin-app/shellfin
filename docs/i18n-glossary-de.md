# German glossary (de)

Reference for `src/i18n/locales/de/translation.json`. Keep new strings consistent with it; change a term here first,
then everywhere it is used. Terms follow the German Windows and macOS UI (Explorer, Finder, Terminal, Schlüsselbund),
VS Code (Remote-SSH) and the German WinSCP / FileZilla localizations.

## Style

- Formal address **„Sie“** everywhere (standard for German desktop software): „Geben Sie einen Host ein.“,
  „Ihre Version“. Never „du“.
- Sentence case as in German grammar: the first word and every noun are capitalized, nothing else
  („Neue Verbindung…“, „Verbindung schließen“, „Lesezeichen exportieren…“). No English-style title case.
- Buttons and menu items are infinitives, usually object first, verb last: „Speichern“, „Verbinden“,
  „Datei hochladen…“, „Terminal neu starten“. Short labels prefer a noun or an infinitive over a full sentence.
- Toggles: „X ein-/ausblenden“ („Seitenleiste ein-/ausblenden“).
- Menu items that open a dialog keep the ellipsis „…“ (U+2026), exactly where English has it. No space before it.
- Quotes: German „…“ (U+201E, U+201C) for UI elements and examples in prose („Bearbeiten“, „prod (2)“).
- Messages are full sentences with a period; labels, column headers and status texts have none.
- Units: a space between number and unit („5 s“, „12 px“, „1 GiB/s“). Sizes use the international symbols
  `B`, `KB`, `MB`, `GB`, `KiB`, `MiB`, `GiB` (Windows Explorer uses them too); `units.*` stay as in English.
- Decimal comma and thousands dot come from `Intl` (`i18n/format.ts`); never write numbers into the text.
- A relative or absolute time inserted at the end of a label follows a colon: „Auf dem Server gespeichert: {{time}}“,
  „Aktualisiert: {{time}}“ (`{{time}}` may be „vor 5 Minuten“ or a date). Inside a sentence use „am“/„um“ instead
  („Veröffentlicht am {{date}}“, „Unterbrochen um {{time}}“).
- „Gehe zu …“ is the one exception to the infinitive rule for commands („Gehe zu Ordner…“, „Gehe zu Zeile“), as in
  Windows and VS Code.
- Avoid word-by-word concatenation: every enum family below is written for its host sentence.
- Keep strings short. German runs about 30 % longer than English; see "Length" below.

## Plurals

i18next uses CLDR: German needs exactly `_one` and `_other`, like English. `_one` is only the number 1; `_other`
covers 0, 2, 5, 21, 1,5 and 1.000.000.

| Form | Numbers | Example |
| --- | --- | --- |
| `_one` | 1 | 1 Lesezeichen, 1 Zeile, 1 Datei |
| `_other` | 0, 2…, 1,5, 1.000.000 | 0 Dateien, 5 Zeilen, 1.000.000 Einträge |

`_one` may drop `{{count}}` where English does („Terminal schließen“), and may say „ein/eine“ in that case.

## Terms

| English | German | Notes |
| --- | --- | --- |
| connection (noun) | Verbindung | „Neue Verbindung…“, „Verbindung schließen“ |
| connect / connecting | verbinden / Verbindungsaufbau | button „Verbinden“; step „Verbinden mit {{host}}“; state and `timeoutOp` „Verbindungsaufbau“ |
| disconnect / disconnected | trennen / getrennt | „Trennen“, state „Getrennt“ |
| reconnect / reconnecting | erneut verbinden / Neuverbindung | „Erneut verbinden“, „Jetzt neu verbinden“, „Neuverbindung in 5 s“, setting „Neuverbindungsversuche“ (not „Verbindungsversuche“) |
| confirmation question (Settings: „Confirm before…“) | Rückfrage | „diese Rückfrage ausschalten“; „Abfrage“ is only for a server or login prompt |
| session | Sitzung | |
| bookmark | Lesezeichen (n., pl. Lesezeichen) | „Meine Lesezeichen“, „Als Lesezeichen speichern…“ |
| group (of bookmarks) | Gruppe | |
| host / host name | Host (m., pl. Hosts) / Hostname | |
| jump host (ProxyJump) | Jump-Host | ProxyJump itself stays in English |
| hop (in a jump chain) | Hop | „Hop 2“ |
| host key | Hostschlüssel | |
| known hosts | bekannte Hosts | file name `known_hosts` stays as is |
| fingerprint | Fingerabdruck | |
| private / public key | privater / öffentlicher Schlüssel | |
| key file | Schlüsseldatei | |
| passphrase | Passphrase | never „Kennwort“ for a key |
| password | Passwort | not „Kennwort“ (macOS and current Windows wording) |
| authentication / authenticate | Authentifizierung / authentifizieren | |
| login prompt | Anmeldeabfrage | server prompt: „Serverabfrage“ |
| agent | Agent | `ssh-agent` stays as is |
| system keychain | Passwortspeicher des Systems | product names (Keychain, Credential Manager, Secret Service) stay in English |
| SSH config | SSH-Konfiguration | `~/.ssh/config` as is |
| upload (verb / noun) | hochladen / Upload | always local → server; „Dateien hochladen…“, „Upload-Limit“ |
| download (verb / noun) | herunterladen / Download | always server → local; „Download-Limit“ |
| load (into memory) | laden | „Wird geladen…“, „Schlüssel konnte nicht geladen werden“ |
| transfer(s) | Übertragung(en) | panel title „Übertragungen“ |
| speed limit / bandwidth | Geschwindigkeitslimit / Bandbreite | short: „Limit“ |
| file tree | Dateibaum | |
| folder / directory | Ordner / Verzeichnis | „Ordner“ in UI; „Verzeichnis“ where English says directory |
| home folder | Home-Ordner | `~` |
| symbolic link | symbolischer Link | |
| permissions / owner / group | Berechtigungen / Besitzer / Gruppe | |
| file manager | Dateimanager | |
| clipboard | Zwischenablage | |
| editor | Editor | |
| encoding | Codierung | Microsoft and VS Code spelling |
| line endings | Zeilenenden | LF, CRLF stay as is |
| active line | aktive Zeile | |
| whitespace | Leerraum | VS Code: „Leerraum rendern“ |
| code folding / fold / unfold | Codefaltung / einklappen / ausklappen | „Zeile einklappen“, „Eingeklappte Zeilen“ |
| sort ascending / descending | aufsteigend / absteigend sortieren | „Nach Größe sortieren“ |
| colours | Blau, Grau, Grün, Orange, Lila, Rot, Petrol (teal), Gelb, Pink | „Petrol“ for teal, „Lila“ for purple |
| terminal | Terminal (n.) | „das Terminal“, pl. „Terminals“ |
| shell | Shell (f.) | „die Shell“ |
| output | Ausgabe | |
| tab | Tab (m.) | „Tab schließen“ |
| sidebar / status bar / activity bar | Seitenleiste / Statusleiste / Aktivitätsleiste | |
| command palette | Befehlspalette | |
| keyboard shortcuts | Tastenkombinationen | |
| settings | Einstellungen | Settings > General: „Einstellungen > Allgemein“ |
| default | Standard | „Systemstandard“, not „Voreinstellung“ |
| timeout | Zeitlimit; „Zeitüberschreitung“ in errors | |
| keepalive | Keepalive | kept as the SSH term |
| match (search) | Treffer | |
| match case / regular expression | Groß-/Kleinschreibung beachten / regulärer Ausdruck | |
| overwrite / replace | überschreiben / ersetzen | |
| discard (changes) | verwerfen | „Nicht speichern“ in the close dialog (Windows wording) |
| dismiss (notice) | schließen / ausblenden | |
| notification | Benachrichtigung | |
| update (noun / verb) | Update / aktualisieren | „Nach Updates suchen…“, „Aktualisieren und neu starten“ |
| release / release notes | Version / Versionshinweise | „Release-Seite“ for the GitHub release page; section „Neuigkeiten“ |
| skip this version | diese Version überspringen | |
| restart | neu starten | |
| installation | Installation | |
| quick connect | Schnellverbindung | |
| start screen / Home | Startseite / Start | tab label „Start“ |
| start directory / startup command | Startverzeichnis / Startbefehl | |
| log / log level | Protokoll / Protokollstufe | „Protokollordner öffnen“ |
| scrollback | Scrollback | „Scrollback-Zeilen“ |
| unsaved changes | ungespeicherte Änderungen | |
| trust | vertrauen | „Vertrauen und verbinden“ |

## Keep in English

SSH, SFTP, ProxyJump, ProxyCommand, Include, Match, ssh-agent, ssh-add, sshd, known_hosts, `~/.ssh/config`, key type
names, SHA256, LF/CRLF, VPN, DNS, PowerShell, OpenSSH, Shellfin, JetBrains Mono, WCAG, ANSI, POSIX, TERM,
`xterm-256color`, tmux, vim, Emacs, keyboard-interactive (the SSH method name), keyboard keys (**Ctrl, Shift, Alt,
Enter, Esc, Del, F2, F6**: decision 6 of #17, so no „Strg“, „Umschalt“, „Entf“; shortcut labels come from
`actionIds.ts` and must match the text), `user@host:port`, paths, commands (`reset`), URLs, and the language's own
name in the language picker („Deutsch“).

## Menu bar mnemonics

Datei / Bearbeiten / Ansicht / Verbindung / Terminal / Hilfe → Alt+D, B, A, V, T, H (Windows convention). All
first letters are unique, so no `&` marker is needed. The German letter wins where it exists: Alt+V opens
„Verbindung“, not „Ansicht“. Alt+F, Alt+E and Alt+C (no German menu starts with them) still open Datei, Bearbeiten and
Verbindung through the English physical-key fallback; Alt+T and Alt+H are the same in both languages.

## Length

German text is about 30 % longer than English. In tight places use the short form:

- Menu bar titles, tab labels, status bar texts, buttons in dialog footers, column headers, settings section names.
- Column headers of the file tree: keep each under 10 characters.
- Dropdown values in Settings: short noun phrases („Letzte Verbindungen öffnen“, „Bei normalem Shell-Ende schließen“,
  „Home-Ordner“).
- Status bar: „Z. 12, Sp. 5“ for the cursor position; state labels are nouns („Verbindungsaufbau“, „Getrennt“).
- Allowed short forms (only in labels, never in sentences): „Z.“ / „Sp.“ (status bar cursor), „Max.“ (setting
  labels), „Regex“ and „Groß/klein“ (CodeMirror search panel), „Limit“ for Geschwindigkeitslimit. No other
  abbreviations: „Übertragungen“ stays written out (the status bar button shares `transfers.title` with the popover).
- File tree column headers: „Name“, „Größe“, „Geändert“ (`files.column.*`).
- Prefer nouns over subordinate clauses in labels („Bei vorhandener Datei“ rather than „Wenn die Datei bereits
  existiert“).
- Check with the pseudo-locale (`?xa=40`) and in German at 580 px.

## Parameter families

Some keys are inserted into other sentences; translate them for their host sentence:

- `entity.*` → noun without article, capitalized, used in `errors.notFound` „{{what}} nicht gefunden.“
  (Lesezeichen, Gruppe, Host aus der SSH-Konfiguration, Übertragung).
- `ioOp.*` → nominalized infinitive with genitive or „von“ (neuter), used in `errors.io` „Fehler beim {{op}}.“
  (Lesen der SSH-Konfiguration, Schreiben von known_hosts, Öffnen des Dateimanagers).
- `timeoutOp.*` → noun phrase in nominative inside parentheses in `errors.timeout`
  (Verbindungsaufbau, Auflisten eines Ordners, SFTP-Anfrage).
- `keyLoadReason.*` → lowercase main clause after a colon in `errors.keyLoadFailed` („die Datei ist beschädigt“).
- `importInvalidReason.*` → lowercase clause inside parentheses in `errors.importInvalidFile`.
- `importInvalidCode.*` / `exportSkipCode.*` → predicate after a field name in `<code>` („`host` fehlt“,
  „`notes` kann in diesem Format nicht gespeichert werden“): third person singular, lowercase.
- `authMethod.*` → nouns, joined into a list by `Intl.ListFormat`.
- `editor.cm.*` → CodeMirror phrases; keep their lowercase style and the `$` placeholder („$ Treffer ersetzt“).
