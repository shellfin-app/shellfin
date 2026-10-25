# Brazilian Portuguese glossary (pt-BR)

Reference for `src/i18n/locales/pt-BR/translation.json`. Keep new strings consistent with it; change a term here
first, then everywhere it is used. Terms follow the Brazilian Portuguese Windows and macOS UI (Explorador de
Arquivos, Finder, Terminal, Gerenciador de Credenciais), GNOME (Chaveiro), VS Code (Remote-SSH) and the pt-BR
FileZilla / WinSCP localizations. Every Portuguese system locale (`pt`, `pt-PT`, `pt-AO`, …) gets this translation
(decision 4 of #17), so prefer wording that a European Portuguese reader also understands where both are natural.

## Style

- **Impersonal**: no “você”, and no possessives “seu”/“sua” addressed to the reader. Instructions use the imperative
  that Windows and macOS use (“Digite um host.”, “Escolha um arquivo de chave.”, “Tente novamente.”); facts are
  stated impersonally or in the passive (“Não foi possível abrir {{name}}.”, “Senhas nunca são exportadas.”).
  English “your version” becomes “versão local”, “your user” becomes “o usuário atual”, “you can still try” becomes
  “ainda assim, é possível tentar”, “if you trust …” becomes “se confiar …”.
- Buttons and menu items are **infinitives**: “Salvar”, “Conectar”, “Fechar guia”, “Enviar arquivos…”,
  “Reiniciar terminal”. Short labels prefer a noun or an infinitive over a sentence.
- Sentence case: only the first word and proper names are capitalized (“Nova conexão…”, “Exportar favoritos…”). No
  English title case.
- Toggles: “Mostrar/ocultar X” (“Mostrar/ocultar barra lateral”).
- Menu items that open a dialog keep the ellipsis “…” (U+2026) exactly where English has it, no space before it.
- Quotes: curly double quotes “…” (U+201C, U+201D) for UI elements and examples in prose (“Copiar detalhes”,
  “prod (2)”). No guillemets, no straight quotes.
- Messages are full sentences with a period; labels, column headers and status texts have none.
- Units: a space between number and unit (“5 s”, “12 px”, “1 GiB/s”); `units.*` stay as in English. Decimal comma
  and thousands dot come from `Intl` (`i18n/format.ts`); never write numbers into the text (except fixed examples
  such as “1,0 a 2,0”, written with a decimal comma).
- “Go to …”: “Ir para a pasta…”, “Ir para a linha”.
- Keep strings short: Portuguese runs about 20–30 % longer than English; see "Length".

## Inserted times and dates

`formatRelativeTime` gives “agora”, “há 5 minutos”, “ontem”; `formatDateTime` gives “25 de out. de 2026 14:05”.

- A relative time follows the verb directly, without a preposition: “Salvo no servidor {{time}}” (“Salvo no servidor
  há 5 minutos”, “… agora”, “… ontem”), “Atualizado {{time}}”, “Alterações não salvas · salvo {{time}}”.
- An absolute date takes “em”: “Lançada em {{date}}”, “{{size}}, modificado em {{time}}”; a clock time takes “às”:
  “Perdida às {{time}}”.
- A time that may be either (file tree tooltip, date setting dependent) follows a colon: “Modificado: {{time}}”.

## Plurals

i18next uses CLDR (`Intl.PluralRules("pt-BR")`): **`_one`, `_many`, `_other`**.

| Form | Numbers | Example |
| --- | --- | --- |
| `_one` | 0, 1 and fractions below 2 (1,5) | 0 favorito, 1 favorito, 1 linha |
| `_many` | exact millions (1.000.000, 2.000.000) | 1.000.000 de favoritos, 1.000.000 de linhas |
| `_other` | 2, 5, 21, 1.000 … | 2 favoritos, 21 linhas |

- `_one` covers **0** too, so it keeps `{{count}}` wherever English shows a number. English `_one` texts without
  `{{count}}` (“Fechar terminal”, “Um terminal ainda está executando um shell.”) keep that wording only where the
  dialog never opens with zero items.
- `_many` is the `_other` text with “de” between the number and the noun that follows it (“{{count}} de arquivos”).
  When the number is not directly followed by a noun (“Terminais ({{count}})”, “Substituir selecionados
  ({{count}})”, “e mais {{count}}”), `_many` equals `_other`.

## Terms

| English | pt-BR | Notes |
| --- | --- | --- |
| connection | conexão (f.) | “Nova conexão…”, “Fechar conexão” |
| connect / connecting | conectar / conectando | step “Conectando a {{host}}”; state “Conectando”; timeout “conexão” |
| disconnect / disconnected | desconectar / desconectado | session states are masculine (“Conectado”, “Fechado”) |
| reconnect / reconnecting | reconectar / reconectando | “Reconectar agora”, “Reconectando em 5 s”, “Tentativas de reconexão” |
| session | sessão | |
| bookmark | **favorito** (m.) | as in Chrome, Edge and Windows; not “marcador” (a tag or a highlighter in pt-BR) |
| group (of bookmarks) | grupo | |
| host / host name | host (m., pl. hosts) / nome do host | |
| jump host (ProxyJump) | host de salto | ProxyJump stays English |
| hop | salto | “salto 2” |
| host key | chave do host | |
| known hosts | hosts conhecidos | file name `known_hosts` as is (“o known_hosts”) |
| fingerprint | impressão digital | |
| private / public key | chave privada / pública | |
| key file | arquivo de chave | |
| passphrase | **frase secreta** | never “senha” for a key (GnuPG / Seahorse pt-BR wording) |
| password | senha | |
| authentication | autenticação / autenticar | |
| login prompt / server prompt | solicitação de login / solicitação do servidor | |
| agent | agente | `ssh-agent` as is |
| system keychain | chaveiro do sistema | GNOME “Chaveiro”; product names (Keychain, Credential Manager, Secret Service) stay English |
| SSH config | configuração SSH | `~/.ssh/config` as is |
| upload (verb / noun) | **enviar / envio** | always local → server; “Enviar arquivos…”, “Limite de envio” |
| download (verb / noun) | **baixar / download** | always server → local; “Baixar…”, “Limite de download” |
| load (into memory) | carregar | “Carregando…”, “Não foi possível carregar a chave” |
| transfer(s) | transferência(s) (f.) | “Cancelada”, “Concluída” |
| file tree | árvore de arquivos | |
| folder / directory | pasta / diretório | “pasta” in the UI; “diretório” where English says directory |
| home folder | pasta pessoal | `~` |
| symbolic link | link simbólico | |
| permissions / owner / group | permissões / proprietário / grupo | |
| file manager | gerenciador de arquivos | |
| clipboard | área de transferência | |
| encoding | codificação | |
| line endings | fins de linha | LF, CRLF as is |
| whitespace | espaço em branco | |
| fold / unfold / code folding | dobrar / desdobrar / dobramento de código | VS Code pt-BR |
| find / replace | localizar / substituir | Microsoft wording |
| overwrite | sobrescrever | distinct from “substituir” (replace) |
| skip | ignorar | “Ignorar todos”, “Ignorar esta versão” |
| discard | descartar | “Não salvar” in the close dialog |
| dismiss | dispensar | |
| sort | classificar | “Classificar por nome”, “em ordem crescente” |
| tab | **guia** (f.) | Windows and VS Code pt-BR; not “aba” |
| terminal / shell | terminal (m.) / shell (m.) | “o shell” |
| output | saída | |
| sidebar / status bar / activity bar | barra lateral / barra de status / barra de atividades | |
| command palette | paleta de comandos | |
| keyboard shortcuts | atalhos de teclado | |
| settings | **Configurações** | Windows 10/11; Settings > General: “Configurações > Geral” |
| default | padrão | “Padrão do sistema” |
| timeout | tempo limite | “excedeu o tempo limite” in errors |
| keepalive | keepalive | SSH term |
| match (search) | correspondência | |
| supported / not supported | compatível / não compatível | Microsoft wording |
| notification | notificação | |
| update | atualização / atualizar | “Verificar atualizações…” |
| release / release notes | versão / notas da versão | “página de lançamentos” for the GitHub releases page; section “Novidades” |
| restart | reiniciar | |
| quick connect | Conexão rápida | |
| start screen / Home | tela inicial / Início | |
| startup command / start directory | comando inicial / diretório inicial | |
| log / log level | log / nível de log | “Abrir pasta de logs”; Debug = “Depuração” |
| scrollback | histórico de rolagem | “Linhas do histórico de rolagem” |
| unsaved changes | alterações não salvas | |
| trust | confiar | “Confiar e conectar” |
| colours | Azul, Cinza, Verde, Laranja, Roxo, Vermelho, Azul-petróleo (teal), Amarelo, Rosa | |

### Upload and download

“Enviar” / “baixar” was chosen over “carregar” / “baixar”: “carregar” already means *load* (“Carregando…”,
“carregar a chave no agente”), so using it for upload would give one verb two meanings. “Enviar” is the verb pt-BR
FileZilla and Google Drive use for local → server, and “baixar” is the established verb for server → local. The
nouns are “envio” and “download” (common pt-BR usage; “baixa” is not used for a file).

## Keep in English

SSH, SFTP, ProxyJump, ProxyCommand, Include, Match, User, ssh-agent, ssh-add, sshd, known_hosts, `~/.ssh/config`,
key type names, SHA256, LF/CRLF, VPN, DNS, PowerShell, OpenSSH (and the Windows service name “OpenSSH Authentication
Agent”), Shellfin, JetBrains Mono, WCAG, ANSI, POSIX, TERM, `xterm-256color`, tmux, vim, Emacs, bracketed paste,
AppImage, keyboard-interactive (as the SSH method name “Keyboard-interactive”), keyboard keys (**Ctrl, Shift, Alt,
Enter, Esc, Del, F2, F6, Option, Meta**; decision 6 of #17), host, log, keepalive, `user@host:port`, paths,
commands (`reset`), URLs, and the endonym “Português (Brasil)”.

## Menu bar mnemonics

Arquivo / Editar / E&xibir / Conexão / Terminal / Aj&uda → Alt+A, E, X, C, T, U, as in Windows pt-BR (Notepad,
Explorer). Editar and Exibir both start with E, Arquivo and Ajuda both start with A, so Exibir is marked on X and
Ajuda on U. Alt+F, Alt+V and Alt+H still open Arquivo, Exibir and Ajuda through the English physical-key fallback.

## Length

- Menu bar titles, tab labels, status bar texts, dialog footer buttons, column headers, settings section names:
  short forms only.
- File tree column headers: “Nome”, “Tamanho”, “Modificado” (`files.column.*`).
- Status bar cursor: “Ln {{line}}, Col {{col}}” (VS Code pt-BR).
- Palette placeholder in the short form: “Hosts, arquivos, comandos: > comandos, @ hosts, / caminho”.
- Settings dropdowns (about 200 px): “Reabrir conexões anteriores”, “Fechar se o shell terminar normalmente”, “A pasta
  aberta na árvore”, “Com conexões abertas”.
- Allowed abbreviations (labels only): “Ln” / “Col”, “máx.”, “regex” (CodeMirror search panel).

## Parameter families

- `entity.*` → noun **with its definite article**, lowercase, in `errors.notFound` “Não foi possível encontrar
  {{what}}.” (o favorito, o grupo, o host da configuração SSH, a transferência); the article carries the gender, so
  the host sentence needs no agreement.
- `ioOp.*` → infinitive phrase after “ao” in `errors.io` “Falha ao {{op}}.” (ler a configuração SSH, gravar o
  known_hosts, abrir o gerenciador de arquivos).
- `timeoutOp.*` → noun phrase without article in parentheses in `errors.timeout` (conexão, listagem de uma pasta,
  espera pela resposta, solicitação SFTP).
- `keyLoadReason.*` → lowercase clause after a colon in `errors.keyLoadFailed` (“o arquivo está danificado”).
- `importInvalidReason.*` → lowercase clause in parentheses in `errors.importInvalidFile`.
- `importInvalidCode.*` / `exportSkipCode.*` → predicate after a field name in `<code>`, third person singular,
  lowercase (“`host` está ausente”, “`notes` não pode ser armazenado neste formato”).
- `authMethod.*` → nouns, joined by `Intl.ListFormat`.
- `editor.cm.*` → CodeMirror phrases; keep their lowercase style and the `$` placeholder (“$ correspondências
  substituídas”); `to` is the range word in “Linhas dobradas 3 a 7” (“a”).
