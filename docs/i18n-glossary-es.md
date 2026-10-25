# Spanish glossary (es)

Reference for `src/i18n/locales/es/translation.json`. Keep new strings consistent with it; change a term here first,
then everywhere it is used. The target is neutral international Spanish that reads naturally in Spain and in Latin
America (one `es` file; `es-*` system languages resolve to it). Terms follow the Spanish Windows and macOS UI
(Explorador de archivos, Finder, Terminal, Llavero), VS Code (Remote-SSH) and the Spanish WinSCP / FileZilla
localizations. Where Spain and Latin America differ, prefer the word both understand (no "ordenador" / "computadora":
"equipo"; no "fichero": "archivo"; no "pinchar" / "ingresar": "hacer clic" / "escribir").

## Style

- **Impersonal and infinitive by default.** Buttons, menu items, labels and settings are infinitives or nouns:
  "Guardar", "Conectar", "Subir archivos…", "Reiniciar terminal". Descriptions use impersonal or passive forms
  ("Se pregunta cada vez…", "Las contraseñas nunca se importan.").
- **"tú" where the user must be addressed** (instructions, validation messages, questions), as current Microsoft,
  Apple and Google Spanish UIs do: "Escribe un host.", "Elige un archivo de clave privada.", "tus marcadores". Never
  "usted" and never voseo. Do not mix: one sentence never switches between impersonal and "tú" for the same subject.
- Sentence case: only the first word and proper names are capitalized ("Nueva conexión…", "Cerrar conexión"). No
  English title case. Language, day and month names are lowercase.
- Questions and exclamations take the opening sign: "¿Guardar los cambios en {{name}}?". The `{{token}}` stays
  inside the signs.
- Menu items that open a dialog keep the ellipsis "…" (U+2026), exactly where English has it. No space before it.
- Quotes: curly double quotes “…” (U+201C, U+201D), as in the English source and the Apple and Latin American
  convention, for UI element names and examples ("“prod (2)”", "“Copiar detalles”"). No «…», no straight quotes in
  prose.
- Messages are full sentences with a period; labels, column headers and status texts have none.
- Units: a space between number and unit ("5 s", "12 px", "1 GiB/s"). Sizes use the international symbols
  `B`, `KB`, `MB`, `GB`, `KiB`, `MiB`, `GiB`; `units.*` stay as in English. Decimal comma and thousands separator
  come from `Intl` (`i18n/format.ts`); never format numbers by hand. Spain writes a decimal comma, Mexico a decimal
  point: in fixed texts prefer whole numbers ("entre 1 y 2") over "1,0" / "1.0".
- Keyboard shortcut labels: "Ctrl+Mayús" is **not** used; keys stay English (see "Keep in English").
- Possessives: "tu"/"tus" only where English has "your" and it matters ("tu versión"); otherwise the article
  ("las conexiones recientes").
- Avoid word-by-word concatenation: every enum family below is written for its host sentence.
- Keep strings short. Spanish runs 20 to 30 % longer than English; see "Length" below.

### Inserted times and dates

- `formatRelativeTime` gives "ahora", "hace 5 minutos", "ayer"; `formatDateTime` gives "25 oct 2026, 17:12".
- A relative or absolute time inserted at the end of a label follows a colon, so both forms read correctly:
  "Guardado en el servidor: {{time}}", "Actualizado: {{time}}", "Modificado: {{time}}", "Conexión perdida: {{time}}".
- Inside a sentence only with a date: "Publicada el {{date}}" ("el 25 oct 2026"). Never "a las {{time}}" (it would
  be "a la 1:05" for one o'clock).

## Plurals

i18next uses CLDR (`Intl.PluralRules("es")`): Spanish needs exactly `_one`, `_many` and `_other`.

| Form | Numbers | Example |
| --- | --- | --- |
| `_one` | 1 | 1 marcador, 1 línea, 1 archivo |
| `_many` | 1 000 000, 2 000 000, … (exact millions) | 1.000.000 de archivos, 2.000.000 de entradas |
| `_other` | 0, 2…, 1,5, 1000, 1.000.001 | 0 archivos, 5 líneas, 1000 entradas |

- `_many` adds "de" between the number and a noun that follows it directly ("{{count}} de marcadores añadidos").
  Where the number is not followed by a noun ("Terminales ({{count}})", "{{count}} terminales siguen…" when the noun
  comes first), `_many` is the `_other` text, adjusted only for "de".
- `_one` may drop `{{count}}` where English does ("Cerrar terminal") and say "un/una" instead.
- Participles agree with the noun: "1 marcador añadido", "3 entradas rechazadas".

## Terms

| English | Spanish | Notes |
| --- | --- | --- |
| connection (noun) | conexión (f.) | "Nueva conexión…", "Cerrar conexión" |
| connect / connecting | conectar / conectando | button "Conectar"; always "con" before the target: "Conectar con {{name}}", "Conectando con {{host}}"; state "Conectando" |
| session states | masculine adjectives | "Conectado", "Desconectado", "Cerrado" (they describe the state, not "la conexión") |
| mouse | mouse | Microsoft and Latin American usage; not "ratón" |
| user@host:port (in prose, hints, placeholders) | usuario@host:puerto | a pattern, not a literal; real values stay as typed |
| disconnect / disconnected | desconectar / desconectado | "Desconectar", state "Desconectado" |
| reconnect / reconnecting | reconectar / reconectando | "Reconectar", "Reconectar ahora", "Reconectando en 5 s", "Intentos de reconexión" |
| session | sesión | |
| bookmark | marcador (m.) | "Mis marcadores", "Guardar como marcador…"; not "favorito" (Firefox and Chrome use "marcadores") |
| group (of bookmarks) | grupo | |
| host / host name | host (m., pl. hosts) / nombre de host | not "anfitrión", not "equipo" |
| jump host (ProxyJump) | host de salto | ProxyJump stays English |
| hop (in a jump chain) | salto | "salto 2" |
| host key | clave de host | |
| known hosts | hosts conocidos | file name `known_hosts` as is |
| fingerprint | huella digital | short "huella" only in tight labels |
| private / public key | clave privada / clave pública | |
| key file | archivo de clave | |
| passphrase | frase de contraseña | never "contraseña" for a key |
| password | contraseña | |
| authentication / authenticate | autenticación / autenticar | state "Autenticando" |
| login prompt / server prompt | solicitud de inicio de sesión / solicitud del servidor | |
| agent | agente | `ssh-agent` stays as is |
| system keychain | llavero del sistema | product names (Keychain, Credential Manager, Secret Service) stay English |
| SSH config | configuración de SSH | `~/.ssh/config` as is |
| upload (verb / noun) | subir / subida | always local → server; "Subir archivos…", "Límite de subida" |
| download (verb / noun) | descargar / descarga | always server → local; "Límite de descarga" |
| load (into memory) | cargar | "Cargando…", "No se pudo cargar la clave" |
| transfer(s) | transferencia(s) | panel title "Transferencias" |
| speed limit / bandwidth | límite de velocidad / ancho de banda | short "Límite" |
| port forwarding | reenvío de puertos | |
| file tree | árbol de archivos | |
| folder / directory | carpeta / directorio | "carpeta" in UI; "directorio" where English says directory |
| home folder | carpeta personal | `~` |
| symbolic link | enlace simbólico | |
| permissions / owner / group | permisos / propietario / grupo | |
| file manager | administrador de archivos | |
| clipboard | portapapeles | |
| editor | editor | |
| encoding | codificación | |
| line endings | finales de línea | LF, CRLF as is |
| active line | línea activa | |
| whitespace | espacios en blanco | |
| code folding / fold / unfold | plegado de código / plegar / desplegar | |
| sort ascending / descending | orden ascendente / descendente | "Ordenar por tamaño" |
| colours | azul, gris, verde, naranja, morado, rojo, verde azulado (teal), amarillo, rosa | |
| terminal | terminal (m.) | "el terminal", "Nuevo terminal", pl. "terminales" (VS Code and Microsoft gender) |
| shell | shell (m.) | "el shell" |
| output | salida | |
| tab | pestaña | "Cerrar pestaña" |
| sidebar / status bar / activity bar | barra lateral / barra de estado / barra de actividad | |
| command palette | paleta de comandos | |
| keyboard shortcuts | atajos de teclado | shorter than "métodos abreviados", used by Apple and Google |
| settings | Configuración | Windows and VS Code wording ("Configuración > General"); not "Preferencias" / "Ajustes" |
| default | predeterminado | "Predeterminado del sistema" |
| timeout | tiempo de espera; errors "se agotó el tiempo de espera" | |
| keepalive | keepalive | kept as the SSH term |
| match (search) | coincidencia | |
| match case / regular expression | coincidir mayúsculas y minúsculas / expresión regular | |
| overwrite / replace | sobrescribir / reemplazar | |
| discard (changes) | descartar | "No guardar" in the close dialog |
| dismiss (notice) | descartar / cerrar | |
| notification | notificación | |
| update (noun / verb) | actualización / actualizar | "Buscar actualizaciones…", "Actualizar y reiniciar" |
| release / release notes | versión / notas de la versión | "página de versiones" for the GitHub release page; section "Novedades" |
| skip this version | omitir esta versión | "omitir" for skip everywhere |
| restart | reiniciar | |
| quick connect | conexión rápida | |
| start screen / Home | pantalla de inicio / Inicio | tab label "Inicio" |
| start directory / startup command | directorio inicial / comando de inicio | |
| log / log level | registro / nivel de registro | "Abrir carpeta de registros" |
| scrollback | historial | "Líneas de historial" |
| unsaved changes | cambios sin guardar | |
| trust | confiar | "Confiar y conectar" |
| computer | equipo | neutral between "ordenador" and "computadora" |
| enter (text) | escribir | "Escribe un host."; not "introducir" / "ingresar" |

## Keep in English

SSH, SFTP, ProxyJump, ProxyCommand, Include, Match, ssh-agent, ssh-add, sshd, known_hosts, `~/.ssh/config`, key type
names, SHA256, LF/CRLF, VPN, DNS, PowerShell, OpenSSH, Shellfin, JetBrains Mono, WCAG, ANSI, ASCII, POSIX, TERM,
`xterm-256color`, tmux, vim, Emacs, keepalive, keyboard-interactive (the SSH method name, shown as "Keyboard-interactive"),
keyboard keys (**Ctrl, Shift, Alt, Enter, Esc, Del, F2, F6**: decision 6 of #17, so no "Mayús", "Supr", "Intro";
shortcut labels come from `actionIds.ts` and must match the text), paths, commands (`reset`), URLs,
and the language's own name in the language picker ("Español").

## Menu bar mnemonics

Archivo / Edición / Ver / Conexión / Terminal / Ayuda. Archivo and Ayuda both start with A, so Ayuda is marked
`"Ay&uda"` (the Spanish Windows convention): Alt+A, E, V, C, T, U. Accents are ignored by the check, so "Edición" is
E and "Conexión" is C. Alt+F, Alt+H (no Spanish menu starts with them) still open Archivo and Ayuda through the
English physical-key fallback.

## Length

Spanish text is 20 to 30 % longer than English. In tight places use the short form:

- Menu bar titles, tab labels, status bar texts, buttons in dialog footers, column headers, settings section names.
- File tree column headers: "Nombre", "Tamaño", "Modificado" (`files.column.*`).
- Status bar cursor: "Lín. 12, col. 5".
- Dropdown values in Settings (~200 px): short noun or infinitive phrases ("Reabrir las conexiones anteriores",
  "Cerrar si el shell terminó bien", "La carpeta personal").
- Segmented controls (theme): one word ("Sistema", "Oscuro", "Claro").
- Host key dialog labels (70 px column, at most two lines): "Del servidor", "Anterior", "Registrada en",
  "En PowerShell:".
- Palette placeholder: the long one fits ~480 px ("Hosts, archivos, comandos: > comandos, @ hosts, / ruta"), the
  short one keeps the "(> @ /)" hint.
- Move commands for list items are "Mover arriba" / "Mover abajo", never "Subir" / "Bajar" ("subir" means upload).
- Allowed short forms (only in labels, never in sentences): "Lín." / "col." (status bar cursor), "Máx." (setting
  labels), "regex" and "mayúsculas/minúsculas" (CodeMirror search panel toggles). No other abbreviations.
- Check with the pseudo-locale (`?xa=30`) and in Spanish at 580 px.

## Parameter families

Some keys are inserted into other sentences; translate them for their host sentence:

- `entity.*` → noun with article, lowercase, used in `errors.notFound` "No se encontró {{what}}."
  ("el marcador", "el grupo", "el host de la configuración de SSH", "la transferencia").
- `ioOp.*` → infinitive phrase, lowercase, used in `errors.io` "Error al {{op}}." ("leer la configuración de SSH",
  "escribir known_hosts", "abrir el administrador de archivos").
- `timeoutOp.*` → noun phrase without article, lowercase, inside parentheses in `errors.timeout`
  "Se agotó el tiempo de espera ({{op}})." ("conexión", "listado de una carpeta", "espera de tu respuesta",
  "solicitud SFTP"); no gerunds.
- `keyLoadReason.*` → lowercase main clause after a colon in `errors.keyLoadFailed` ("el archivo está dañado").
- `importInvalidReason.*` → lowercase clause inside parentheses in `errors.importInvalidFile`.
- `importInvalidCode.*` / `exportSkipCode.*` → predicate after a field name in `<code>` ("`host` falta",
  "`notes` no se puede guardar en este formato"): third person singular, lowercase.
- `importEntry.*` → "Marcador {{n}}" / "Grupo {{n}}", followed by ": `field` predicate".
- `authMethod.*` → nouns, joined into a list by `Intl.ListFormat` ("Contraseña y Clave pública" stays capitalized
  like the English list).
- `editor.cm.*` → CodeMirror phrases; keep their lowercase style and the `$` placeholder ("$ coincidencias
  reemplazadas"). "Folded lines" / "Unfolded lines" + N + "to" + M read "Líneas plegadas 3 a 7."; "current match" +
  text + "on line" + N reads "coincidencia actual. … en la línea 12.".
