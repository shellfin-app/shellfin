# French glossary (fr)

Reference for `src/i18n/locales/fr/translation.json` (French as used in France). Keep new strings consistent with it;
change a term here first, then everywhere it is used. Terms follow the French Windows and macOS UI (Explorateur de
fichiers, Finder, Terminal, Trousseau d'accès), the Microsoft French style guide, VS Code (Remote-SSH) and the French
FileZilla / WinSCP localizations.

## Style

- Formal address **« vous »** everywhere: « Saisissez un hôte. », « Votre version ». Never « tu ».
- Sentence case: only the first word and proper nouns are capitalized (« Nouvelle connexion… », « Fermer la
  connexion », « Exporter les signets… »). No English-style title case.
- Buttons and menu items are infinitives (« Enregistrer », « Se connecter », « Envoyer des fichiers… »,
  « Redémarrer le terminal »). Short labels prefer a noun or an infinitive over a full sentence.
- Toggles: « Afficher/masquer X » (« Afficher/masquer la barre latérale »).
- Menu items that open a dialog keep the ellipsis « … » (U+2026), exactly where English has it. No space before it.
- Messages are full sentences with a period; labels, column headers and status texts have none.
- Apostrophe: ASCII `'` (l'hôte, d'accès) everywhere, so search and keyboard input match (as in the uk glossary).
- Quotes: French guillemets « … » for UI elements and examples in prose (« Modifier la connexion », « Connexion
  rapide », « prod (2) »), always with U+00A0 inside. English “…” become « … », and a UI name that English writes
  without quotes is quoted too.
- Prefer the active voice (« vous confirmez vous-même les nouvelles clés d'hôte », not « doivent être confirmées
  par vous »).

### Spaces (typography)

- **One character for every French space: U+00A0 (no-break space)**, as the Microsoft French style guide does. It
  goes before `:` `;` `?` `!`, after `«` and before `»`, and between a number or a `{{token}}` and its unit
  (« 5 s », « {{seconds}} s », « 12 px », « 1 Gio/s »). U+202F (narrow no-break space) is not used in the translation file, so a single search finds every case; `Intl`
  may still insert U+202F as the thousands separator in formatted numbers, which is fine.
- No space before `.` `,` `…` `)`, none inside parentheses. A colon that comes from code (`importEntry.*` +
  `: <code>field</code>`) cannot be changed; accept the English spacing there.
- Text that is followed by a value in the code (`conflict.onServer`, `conflict.inYours`) keeps the trailing space.

### Units and numbers

- Sizes use the French byte symbols: `o`, `Ko`, `Mo`, `Go`, `To` (decimal) and `Kio`, `Mio`, `Gio`, `Tio` (binary),
  as in French Windows (« Ko », « Mo »). `units.*` carry them; texts that name a unit use the same symbols
  (« Kio/s », « 1 Gio/s », « Taille max. des fichiers (Mio) »). `units.perSecond` stays `{{size}}/s`.
- Decimal comma and the thousands space come from `Intl` (`i18n/format.ts`); never write numbers into the text,
  except fixed limits that English also writes (« 1,0 à 2,0 », « 4,5 » use the French decimal comma).
- Seconds: « s » (« Délai de connexion (s) »).

### Times and dates inserted into text

- **Relative times** (`formatRelativeTime`: « il y a 5 minutes », « hier », « maintenant ») are inserted after a
  colon, because « maintenant » and « il y a … » do not read well after a past participle without one:
  « Enregistré sur le serveur : {{time}} », « Mis à jour : {{time}} », « Modifié : {{time}} » (the file tree tooltip,
  which can also get an absolute date).
- **Absolute dates** inside a sentence use « le » (« Publiée le {{date}} », « modifié le {{time}} » for date + time);
  a time of day alone uses « à » (« Perdue à {{time}} »).

### Inserted fragments

- Never make a past participle or an adjective agree with a placeholder whose gender is unknown. Put a noun in
  front (« Le fichier {{name}} a été modifié sur le serveur ») or use a word with one form for both genders
  (« {{path}} est introuvable »). Paths and file names (`{{path}}`, `{{file}}`, `{{name}}` of a file) count as
  « le fichier » and take the masculine (« {{file}} a été ignoré »). See "Parameter families" below.
- Lists from `formatList` (`{{files}}`) use « et » (`Intl.ListFormat`); a sentence that starts with one takes the
  verb in the plural only in the `_many`/`_other` forms.

## Plurals

i18next uses CLDR: French needs `_one`, `_many` and `_other` wherever English has `_one`/`_other`.

| Form | Numbers | Example |
| --- | --- | --- |
| `_one` | 0, 1 (and 1,5) | 0 signet ajouté, 1 ligne |
| `_many` | 1 000 000, 2 000 000… (exact millions) | 1 000 000 de signets ajoutés |
| `_other` | 2…999 999, 1 000 001… | 5 lignes, 21 fichiers |

- `_one` also covers 0, so it must read for « 0 » too: use `{{count}}` with a singular noun (« {{count}} signet
  ignoré »). Where English `_one` drops the number (« Close terminal »), French may drop it too.
- `_many` is the `_other` text with « de » (or « d' ») between the number and a noun directly after it
  (« {{count}} de lignes »); when no noun follows the number (« Terminaux ({{count}}) », « et {{count}} autres »), it
  equals `_other`.

## Terms

| English | French | Notes |
| --- | --- | --- |
| connection (noun) | connexion | « Nouvelle connexion… », « Fermer la connexion » |
| connect / connecting | se connecter / connexion | button « Se connecter »; step « Connexion à {{host}} »; state « Connexion » |
| disconnect / disconnected | se déconnecter (menu « Déconnecter ») / déconnecté | state « Déconnecté » |
| reconnect / reconnecting | se reconnecter / reconnexion | « Se reconnecter », « Reconnexion dans 5 s », setting « Tentatives de reconnexion » |
| session | session | |
| bookmark | signet (m.) | « Mes signets », « Enregistrer comme signet… »; not « favori » or « marque-page » (browser terms; « signet » is the term of FileZilla-style site lists and Office) |
| group (of bookmarks) | groupe | |
| host / host name | hôte (m.) / nom d'hôte | « l'hôte », « des hôtes » |
| jump host (ProxyJump) | hôte de rebond | ProxyJump itself stays in English |
| hop (in a jump chain) | saut | « saut 2 » |
| host key | clé d'hôte | « la clé d'hôte de {{host}}:{{port}} » |
| known hosts | hôtes connus | file name `known_hosts` stays as is |
| fingerprint | empreinte | |
| private / public key | clé privée / clé publique | spelling « clé », not « clef » |
| key file | fichier de clé | |
| passphrase | phrase secrète | never « mot de passe » for a key (Microsoft and GnuPG French wording) |
| password | mot de passe | |
| authentication / authenticate | authentification / s'authentifier | state « Authentification » |
| login prompt / server prompt | demande de connexion / demande du serveur | |
| agent | agent | `ssh-agent` stays as is |
| system keychain | trousseau du système | product names (Keychain, Credential Manager, Secret Service) stay in English |
| SSH config | configuration SSH | `~/.ssh/config` as is |
| upload (verb / noun) | envoyer / envoi | always local → server (FileZilla fr). Not « téléverser » (Canadian usage, unusual in France) and not « charger » (taken by *load*) |
| download (verb / noun) | télécharger / téléchargement | always server → local |
| load (into memory) | charger / chargement | « Chargement… », « Impossible de charger la clé » |
| transfer(s) | transfert(s) | panel title « Transferts » |
| speed limit / bandwidth | limite de vitesse / bande passante | short: « Limite » |
| file tree | arborescence | « l'arborescence des fichiers » where needed |
| folder / directory | dossier / répertoire | « dossier » in UI; « répertoire » where English says directory |
| home folder | dossier personnel | `~` |
| symbolic link | lien symbolique | |
| permissions / owner / group | permissions / propriétaire / groupe | Unix sense, as FileZilla fr |
| file manager | gestionnaire de fichiers | |
| clipboard | presse-papiers | |
| editor | éditeur | |
| encoding | encodage | VS Code fr |
| line endings | fins de ligne | LF, CRLF stay as is |
| active line | ligne active | |
| whitespace | espaces | « Afficher les espaces » (covers tabs too) |
| code folding / fold / unfold | repli de code / replier / déplier | |
| sort ascending / descending | tri croissant / décroissant | « Trier par taille » |
| colours | bleu, gris, vert, orange, violet, rouge, bleu canard (teal), jaune, rose | |
| terminal | terminal (m., pl. terminaux) | |
| shell | shell (m.) | « le shell » |
| output | sortie | |
| tab | onglet | « Fermer l'onglet » |
| sidebar / status bar / activity bar | barre latérale / barre d'état / barre d'activité | |
| command palette | palette de commandes | |
| keyboard shortcuts | raccourcis clavier | |
| settings | paramètres | Windows wording on every platform (the app uses one word; macOS « Réglages » is not used). Settings > General: « Paramètres > Général » |
| default | par défaut | « Valeur système par défaut » |
| timeout | délai; « a expiré » in errors | « Délai de connexion (s) » |
| keepalive | keepalive | kept as the SSH term |
| match (search) | occurrence | |
| match case / regular expression | respecter la casse / expression régulière | |
| overwrite / replace | écraser / remplacer | |
| discard (changes) | abandonner | « Ne pas enregistrer » in the close dialog (Windows wording) |
| dismiss (notice) | fermer / ignorer | |
| notification | notification | |
| update (noun / verb) | mise à jour / mettre à jour | « Rechercher des mises à jour… », « Mettre à jour et redémarrer » |
| release / release notes | version / notes de version | « page de la version » for the GitHub release page; section « Nouveautés » |
| skip this version | ignorer cette version | |
| restart | redémarrer | |
| installation | installation | |
| quick connect | connexion rapide | |
| start screen / Home | écran d'accueil / Accueil | tab label « Accueil » |
| start directory / startup command | répertoire de départ / commande de démarrage | |
| log / log level | journal (pl. journaux) / niveau de journalisation | « Ouvrir le dossier des journaux » |
| scrollback | historique | « Lignes d'historique » |
| unsaved changes | modifications non enregistrées | |
| save | enregistrer | never « sauvegarder » (that is *backup*) |
| trust | faire confiance | « Faire confiance et se connecter » |
| port forwarding | redirection de port | |

## Keep in English

SSH, SFTP, ProxyJump, ProxyCommand, Include, Match, ssh-agent, ssh-add, sshd, known_hosts, `~/.ssh/config`, key type
names, SHA256, LF/CRLF, VPN, DNS, PowerShell, OpenSSH, Shellfin, JetBrains Mono, WCAG, ANSI, POSIX, TERM,
`xterm-256color`, tmux, vim, Emacs, keyboard-interactive (the SSH method name), keyboard keys (**Ctrl, Shift, Alt,
Option, Enter, Esc, Del, F2, F6**: decision 6 of #17, so no « Maj », « Entrée », « Suppr »; shortcut labels come from
`actionIds.ts` and must match the text), `user@host:port`, paths, commands (`reset`), URLs, and the language's own
name in the language picker (« Français »).

## Menu bar mnemonics

Fichier / Édition / Affichage / Connexion / Terminal / Aide. « Affichage » and « Aide » both start with A, so Help
carries a marker: **`Ai&de`** (Alt+D). The others use their first letter: Alt+F, Alt+E (« Édition » folds to e, so
Alt+E works on AZERTY without a dead key), Alt+A, Alt+C, Alt+T. Not `Aid&e`: e would collide with « Édition » after
accent folding. Alt+H still opens Aide through the English physical-key fallback, and Alt+V opens Affichage.

## Length

French runs about 20–30 % longer than English. In tight places use the short form:

- Menu bar titles, tab labels, status bar texts, buttons in dialog footers, column headers, settings section names
  (legends are uppercase with letter spacing: one or two words).
- File tree column headers: « Nom », « Taille », « Modifié » (`files.column.*`), each under 10 characters.
- Status bar: « Ln {{line}}, Col {{col}} » (VS Code fr); state labels are nouns (« Connexion », « Déconnecté »).
- Palette placeholder: at most about 80 characters (it must fit in about 480 px at a 960 px window), the short one
  about 30.
- Dropdown values in Settings (about 200 px): short noun or infinitive phrases (« Rouvrir les connexions
  précédentes », « Le dossier personnel »).
- Allowed short forms (labels only, never in sentences): « max. » (setting labels), « Ln » / « Col » (status bar),
  « regex » (CodeMirror search panel), « Limite ».
- Host key dialog fact labels (a column of about 70 px): « Type », « Déjà vue », « Proposée », « Enregistrée dans »,
  « Dans PowerShell : », « Dans un terminal : ».
- File tree date column (14ch, longest French value « il y a 10 min », 13): French gets the short relative style (« il y a 5 min », « dans 3 h », « hier »),
  because the narrow one writes « -5 min » (`formatFileDateShort`).
- Short forms used: « Limite de téléchargement » / « Limite d'envoi » (not « Limite de vitesse de … »),
  « Fermer si fin normale du shell », « Dossier personnel », « Taille de tabulation », « Comportement » (the
  terminal behaviour legend, inside the Terminal page).
- Check with the pseudo-locale (`?xa=30`) and in French at 580 px.

## Parameter families

Some keys are inserted into other sentences; translate them for their host sentence:

- `entity.*` → capitalized noun without article, in `errors.notFound` « {{what}} introuvable. » (Signet, Groupe,
  Hôte de la configuration SSH, Transfert). « introuvable » has one form for both genders, so a future feminine
  entity needs no change; keep every value singular.
- `ioOp.*` → noun phrase with its article after « lors de » in `errors.io` « Échec lors de {{op}}. »
  (« la lecture de la configuration SSH », « l'écriture de known_hosts », « l'ouverture du gestionnaire de
  fichiers »). Every value starts with « la » or « l' », which « de » leaves unchanged; never start one with « le »
  or « les » (« de le », « de les » are wrong).
- `timeoutOp.*` → noun phrase without article inside parentheses in `errors.timeout` « L'opération a expiré
  ({{op}}). » (« connexion », « listage d'un dossier », « attente de votre réponse », « requête SFTP »).
- `keyLoadReason.*` → lowercase main clause after a colon in `errors.keyLoadFailed` (« le fichier est endommagé »).
- `importInvalidReason.*` → lowercase clause inside parentheses in `errors.importInvalidFile`.
- `importInvalidCode.*` / `exportSkipCode.*` → predicate after a field name in `<code>`, third person singular,
  lowercase. A field name stands for « le champ », so participles are masculine singular (« `host` est manquant »,
  « `notes` ne peut pas être enregistré dans ce format »). The colon before `<code>` comes from the code.
- `authMethod.*` → nouns with a capital, joined into a list by `Intl.ListFormat`.
- `disconnectKind.*`, `saveStrategy.*`, `hostKeyVerification.*` → stand-alone sentences or labels.
- `editor.cm.*` → CodeMirror phrases; keep their lowercase style and the `$` placeholder (« $ occurrences
  remplacées »).
