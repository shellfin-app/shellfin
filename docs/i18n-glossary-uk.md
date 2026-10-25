# Ukrainian glossary (uk)

Reference for `src/i18n/locales/uk/translation.json`. Keep new strings consistent with it; change a term here first,
then everywhere it is used.

## Style

- Sentence case; buttons and menu items are imperative infinitives: «Підключитися», «Зберегти», «Вивантажити файли…».
- Menu items that open a dialog keep the ellipsis «…» (U+2026), exactly where English has it.
- Quotes: «…» when referring to a UI element in prose («Редагувати з'єднання», «Швидке підключення»).
- Apostrophe: ASCII `'` (з'єднання, ім'я) everywhere, so search and keyboard input match.
- Never make a verb agree in gender with a placeholder. Prefer a noun before it («Файл {{name}} змінено»,
  «Хост {{host}}:{{port}} відхилив з'єднання») or an impersonal form («{{path}} не знайдено»).
- Units: «с» for seconds, «КБ/МБ/ГБ» for sizes (`units.*`), «КіБ/с», «МіБ/с», «ГіБ/с» where English says KiB/MiB/GiB.

## Plurals

i18next uses CLDR: Ukrainian needs `_one`, `_few`, `_many`, `_other` wherever English has `_one`/`_other`.

| Form | Numbers | Example |
| --- | --- | --- |
| `_one` | 1, 21, 31, 101… | 1 рядок, 21 файл |
| `_few` | 2–4, 22–24… | 3 рядки |
| `_many` | 0, 5–20, 25–30… | 5 рядків |
| `_other` | fractions | 1,5 рядка (genitive singular) |

`_one` also covers 21, 31…, so it must not say "this file" / "один": include `{{count}}` or use wording that works
for 21 (e.g. «Незбережені зміни в {{count}} файлі»). A Ukrainian plural form may use `{{count}}` even where English
`_one` does not.

## Terms

| English | Ukrainian | Notes |
| --- | --- | --- |
| connection (noun) | з'єднання | «Нове з'єднання», «Закрити з'єднання» |
| connect / connecting | підключитися / підключення | «Підключитися до {{name}}», «Швидке підключення» |
| disconnect / disconnected | від'єднатися / від'єднано | |
| reconnect / reconnecting | перепідключитися / перепідключення | |
| session | сеанс | |
| bookmark | закладка | |
| group (of bookmarks) | група | |
| host | хост | ім'я хоста (host name) |
| jump host (ProxyJump) | проміжний хост | ProxyJump itself stays in English |
| hop (in a jump chain) | вузол | «вузол {{hop}}» |
| host key | ключ хоста | |
| known hosts | відомі хости | file name `known_hosts` stays as is |
| fingerprint | відбиток | |
| private / public key | закритий / відкритий ключ | |
| passphrase | парольна фраза | |
| password | пароль | |
| authentication | автентифікація | |
| agent | агент | `ssh-agent` stays as is |
| system keychain | системне сховище паролів | product names (Keychain, Credential Manager, Secret Service) stay in English |
| SSH config | конфігурація SSH | |
| upload | вивантажити / вивантаження | always local → server (FileZilla/WinSCP uk usage) |
| download | завантажити / завантаження | always server → local |
| load (read into memory) | завантажити / завантаження | «Завантаження…», «Не вдалося завантажити ключ» |
| transfer(s) | передавання | gen. pl. «передавань»; panel title «Передавання» |
| bandwidth limit | обмеження швидкості | |
| file tree | дерево файлів | |
| folder / directory | папка / каталог | «папка» in UI; «каталог» where English says directory |
| symbolic link | символьне посилання | |
| permissions | дозволи | |
| editor | редактор | |
| encoding | кодування | |
| line endings | кінці рядків | LF, CRLF stay as is |
| active line (editor) | поточний рядок | the line with the cursor |
| whitespace (spaces and tabs) | пробіли | «Показувати пробіли»; tabs are covered by the same word |
| terminal | термінал | gen. «термінала» (device sense) |
| shell | оболонка | |
| output (terminal) | вивід | |
| tab | вкладка | |
| sidebar / status bar / activity bar | бічна панель / рядок стану / панель дій | |
| command palette | палітра команд | |
| keyboard shortcuts | комбінації клавіш | |
| toggle X | показати/сховати X | |
| settings | налаштування | |
| default | типовий / типово | not «за замовчуванням» |
| timeout | тайм-аут; «час очікування … вичерпано» in errors | |
| keepalive | keepalive | kept as the SSH term |
| match (search) | збіг | |
| match case / regular expression | враховувати регістр / регулярний вираз | |
| overwrite / replace | перезаписати / замінити | |
| discard (changes) | відкинути | |
| dismiss (notice) | закрити / прибрати | |
| notification | сповіщення | |
| update (noun / verb) | оновлення / оновити(ся) | «Перевірити оновлення…», «Оновити й перезапустити» |
| release (a version) / pre-release | випуск / попередній випуск | «сторінка випуску» |
| release notes | примітки до випуску | dialog section title «Що нового» |
| skip this version | пропустити цю версію | |
| restart (the app) | перезапустити | |
| installation (an installed copy) | інсталяція | «Ця інсталяція не може оновитися сама» |

## Keep in English

SSH, SFTP, ProxyJump, ProxyCommand, Include, Match, ssh-agent, ssh-add, sshd, known_hosts, `~/.ssh/config`, key type
names, SHA256, LF/CRLF, VPN, DNS, PowerShell, OpenSSH, Shellfin, JetBrains Mono, keyboard keys (Ctrl, Shift, Alt,
Enter, Esc, Del, F2, F6), `user@host:port`, paths, commands (`reset`), URLs, and the language's own name in the
language picker.

## Parameter families

Some keys are inserted into other sentences; translate them in the grammatical case the host sentence needs:

- `entity.*` → accusative, used in `errors.notFound` «Не знайдено {{what}}.» (закладку, групу).
- `ioOp.*` → genitive, used in `errors.io` «Помилка під час {{op}}.» (читання…, запису…).
- `timeoutOp.*` → nominative inside parentheses in `errors.timeout`.
- `keyLoadReason.*` → lowercase clause after a colon in `errors.keyLoadFailed`.
- `editor.cm.*` → CodeMirror phrases; keep their lowercase style and the `$` placeholder.
