// Release notes -> a small block tree (data, never HTML). The notes are untrusted server text
// (docs/ARCHITECTURE.md §9.5, §9.8); the caller renders the tree with React elements and runs every text node
// through displayServerText.
//
// Grammar (everything else is literal text):
//   block   := heading | list | paragraph
//   heading := 0-3 spaces, 1-3 '#', a space (or end of line), non-empty text (a lone '#' is literal text)
//   list    := items starting with '-' or '*' plus a space; the first bullet's indent sets the top level, an indent of
//              2+ more than that = one nested level (deeper indentation is flattened to that level); indented lines
//              directly after an item continue it
//   paragraph := consecutive other non-blank lines, joined with a space; a blank line ends it
//   inline  := `code` | **bold** (code and links inside) | [text](url) | literal text
//   url     := ends at the ')' that balances the opening '(' (nested pairs allowed, at most MAX_URL_PAREN_DEPTH deep);
//              it must contain no space or tab (so `[a](u "title")` is literal text)
// Known divergence from CommonMark: a link candidate wins over a code span that starts inside the link text. Harmless:
// the URL is still checked against the allowlist and the text is shown as is.
// Unmatched or empty delimiters are literal text. Parsing is a single hand-written scan: no regular expressions, and
// every delimiter search is cached (parentheses: matched once per text, see matchParens) so the work stays linear in
// the input size.

export type Inline =
  | { kind: "text"; text: string }
  | { kind: "code"; text: string }
  | { kind: "bold"; children: Inline[] }
  | { kind: "link"; text: string; url: string };

export interface ListItem {
  inline: Inline[];
  sub: Inline[][];
}

export type Block =
  | { kind: "heading"; level: 1 | 2 | 3; inline: Inline[] }
  | { kind: "paragraph"; inline: Inline[] }
  | { kind: "list"; items: ListItem[] };

export interface ParsedNotes {
  blocks: Block[];
  /** Input or structure exceeded a limit: the rest was dropped or shown as plain text. */
  truncated: boolean;
}

/** Hard input bound (UTF-16 units); the dialog already truncates to a smaller number of characters. */
export const MAX_NOTES_INPUT = 40_000;
/** Headings, paragraphs and list items (sub-items included); further blocks are dropped. */
export const MAX_BLOCKS = 400;
/** Inline nodes in the whole document; after this the rest of the text is kept as one plain text node. */
export const MAX_INLINE_NODES = 3000;
/** A link URL longer than this is never clickable (same bound as the backend's app_open_external). */
export const MAX_LINK_URL = 2048;
/** Nested '(' ... ')' pairs allowed inside a link URL; a deeper one makes the whole link literal text. */
export const MAX_URL_PAREN_DEPTH = 32;

const GITHUB_REPO_PATH = "/shellfin-app/shellfin";
const SITE_HOSTS: readonly string[] = ["shellfin.app", "www.shellfin.app"];

function isDigits(s: string): boolean {
  if (s.length === 0) return false;
  for (let i = 0; i < s.length; i++) {
    const c = s.charCodeAt(i);
    if (c < 0x30 || c > 0x39) return false;
  }
  return true;
}

/**
 * Paths on github.com that only the maintainers control: the repository root, releases (and their assets), the
 * default branch's files, an issue or a pull request page. Not allowed: `/raw/`, `/commit/<sha>`, `/blob/<sha>/`,
 * other branches, `/pull/<n>/files` and anything else, because a fork under the same repository path can serve content
 * anyone authored.
 */
function isAllowedGithubPath(p: string): boolean {
  const base = GITHUB_REPO_PATH;
  if (p === base || p === `${base}/`) return true;
  if (p === `${base}/releases` || p.startsWith(`${base}/releases/`)) return true;
  if (p.startsWith(`${base}/blob/main/`)) return p.length > `${base}/blob/main/`.length;
  if (p === `${base}/tree/main` || p.startsWith(`${base}/tree/main/`)) return true;
  if (p.startsWith(`${base}/issues/`)) return isDigits(p.slice(`${base}/issues/`.length));
  if (p.startsWith(`${base}/pull/`)) return isDigits(p.slice(`${base}/pull/`.length));
  return false;
}

/**
 * The link allowlist (a link outside it is shown as its text only). Returns the parsed URL, which the caller must open
 * (`href`), so that exactly what was checked is what is opened; `null` = not allowed. All of: https, no non-empty
 * credentials, no non-default port (the WHATWG parser drops `:443` and empty userinfo), at most 2048 characters, no
 * space, C0 control or DEL, no `\`, `%2F` or `%5C` (parsers disagree on them) and none of the `⟨` `⟩` escape markers of
 * `displayServerText`, and either
 *  - host exactly github.com with one of the paths of `isAllowedGithubPath` (query and fragment allowed), or
 *  - host exactly shellfin.app or www.shellfin.app (any path, query, fragment).
 * No other subdomains or look-alike hosts. The backend re-checks only scheme, credentials, length and host presence
 * and confirms before opening (§9.8).
 */
export function allowedReleaseLink(url: string): URL | null {
  if (url.length === 0 || url.length > MAX_LINK_URL) return null;
  for (let i = 0; i < url.length; i++) {
    const c = url.charCodeAt(i);
    if (c <= 0x20 || c === 0x7f) return null;
  }
  if (url.includes("\\") || url.includes("⟨") || url.includes("⟩")) return null;
  const lower = url.toLowerCase();
  if (lower.includes("%2f") || lower.includes("%5c")) return null;
  let u: URL;
  try {
    u = new URL(url);
  } catch {
    return null;
  }
  if (u.protocol !== "https:" || u.username !== "" || u.password !== "" || u.port !== "") return null;
  if (SITE_HOSTS.includes(u.hostname)) return u;
  return u.hostname === "github.com" && isAllowedGithubPath(u.pathname) ? u : null;
}

interface Budget {
  inline: number;
  truncated: boolean;
}

/** First index >= from of needle, remembering the last answer so repeated searches never rescan. */
function searcher(src: string, needle: string): (from: number) => number {
  let cached = -2;
  return (from) => {
    if (cached === -1) return -1;
    if (cached >= from) return cached;
    cached = src.indexOf(needle, from);
    return cached;
  };
}

interface ParenMatches {
  /** For each '(' its balancing ')' (or -1); unused for other characters. */
  close: Int32Array;
  /** For each '(' the deepest nesting of pairs inside it (0 = no inner pair). */
  inner: Int32Array;
}

/**
 * Pairs every '(' with its balancing ')' in one pass with a stack (an unmatched ')' is ignored, an unmatched '(' keeps
 * -1). Linear no matter how many links ask: a link candidate only looks its answer up, so thousands of `[a](` with
 * unbalanced parentheses never rescan the same characters. `inner` is built on the way (a closed pair raises its
 * parent's depth), so the depth limit is a lookup too.
 */
function matchParens(src: string): ParenMatches {
  const close = new Int32Array(src.length).fill(-1);
  const inner = new Int32Array(src.length);
  const stack: number[] = [];
  for (let i = 0; i < src.length; i++) {
    const c = src.charCodeAt(i);
    if (c === 0x28) {
      stack.push(i);
    } else if (c === 0x29) {
      const open = stack.pop();
      if (open === undefined) continue;
      close[open] = i;
      const parent = stack[stack.length - 1];
      const depth = (inner[open] ?? 0) + 1;
      if (parent !== undefined && depth > (inner[parent] ?? 0)) inner[parent] = depth;
    }
  }
  return { close, inner };
}

function parseInline(src: string, budget: Budget, allowBold: boolean): Inline[] {
  const out: Inline[] = [];
  const findTick = searcher(src, "`");
  const findBold = searcher(src, "**");
  const findMid = searcher(src, "](");
  const findSpace = searcher(src, " "); // the starts of successive link destinations only grow, so the caches hold
  const findTab = searcher(src, "\t");
  let parens: ParenMatches | null = null; // built on the first link candidate only
  let lit = 0;
  let i = 0;
  const flush = (end: number) => {
    if (end > lit) {
      budget.inline++;
      out.push({ kind: "text", text: src.slice(lit, end) });
    }
  };
  while (i < src.length) {
    if (budget.inline >= MAX_INLINE_NODES) {
      budget.truncated = true;
      break;
    }
    const ch = src.charCodeAt(i);
    if (ch === 0x60) {
      const end = findTick(i + 1);
      if (end > i + 1) {
        flush(i);
        budget.inline++;
        out.push({ kind: "code", text: src.slice(i + 1, end) });
        i = lit = end + 1;
        continue;
      }
    } else if (ch === 0x2a && allowBold && src.charCodeAt(i + 1) === 0x2a) {
      const end = findBold(i + 2);
      if (end > i + 2) {
        flush(i);
        budget.inline++;
        out.push({ kind: "bold", children: parseInline(src.slice(i + 2, end), budget, false) });
        i = lit = end + 2;
        continue;
      }
      i += 2;
      continue;
    } else if (ch === 0x5b) {
      const mid = findMid(i + 1);
      if (mid > i + 1) {
        parens ??= matchParens(src);
        const close = parens.close[mid + 1] ?? -1;
        // A space or tab inside the destination (e.g. a "title" part) makes the candidate literal, as in CommonMark.
        const sp = findSpace(mid + 2);
        const tab = findTab(mid + 2);
        const hasBlank = close >= 0 && ((sp !== -1 && sp < close) || (tab !== -1 && tab < close));
        if (close >= 0 && !hasBlank && (parens.inner[mid + 1] ?? 0) <= MAX_URL_PAREN_DEPTH) {
          flush(i);
          budget.inline++;
          out.push({ kind: "link", text: src.slice(i + 1, mid), url: src.slice(mid + 2, close) });
          i = lit = close + 1;
          continue;
        }
      }
    }
    i++;
  }
  // The rest (also what is left when the node budget ran out) is plain text.
  flush(src.length);
  return out;
}

const HEADING_RE_MAX_HASHES = 3;

/** Indent width of a line (tab = 4) and the index of its first non-blank character; -1 for a blank line. */
function measure(line: string): { indent: number; at: number } | null {
  let indent = 0;
  for (let i = 0; i < line.length; i++) {
    const c = line.charCodeAt(i);
    if (c === 0x20) indent++;
    else if (c === 0x09) indent += 4;
    else if (c === 0x0d || c === 0x0b || c === 0x0c) continue;
    else return { indent, at: i };
  }
  return null;
}

/** The text after "- " / "* " for a bullet line, else null. */
function bulletText(line: string, at: number): string | null {
  const c = line.charCodeAt(at);
  if (c !== 0x2d && c !== 0x2a) return null;
  const next = line.charCodeAt(at + 1);
  if (next !== 0x20 && next !== 0x09) return null;
  return line.slice(at + 2).trim();
}

function headingOf(line: string, at: number, indent: number): { level: 1 | 2 | 3; text: string } | null {
  if (indent > 3 || line.charCodeAt(at) !== 0x23) return null;
  let n = 0;
  while (line.charCodeAt(at + n) === 0x23) n++;
  if (n > HEADING_RE_MAX_HASHES) return null;
  const after = line.charCodeAt(at + n);
  if (!Number.isNaN(after) && after !== 0x20 && after !== 0x09) return null;
  return { level: n as 1 | 2 | 3, text: line.slice(at + n).trim() };
}

export function parseReleaseNotes(input: string): ParsedNotes {
  const budget: Budget = { inline: 0, truncated: false };
  let text = input;
  if (text.length > MAX_NOTES_INPUT) {
    text = text.slice(0, MAX_NOTES_INPUT);
    budget.truncated = true;
  }
  const blocks: Block[] = [];
  let blockCount = 0;
  const room = (): boolean => {
    if (blockCount >= MAX_BLOCKS) {
      budget.truncated = true;
      return false;
    }
    blockCount++;
    return true;
  };

  // Raw (unparsed) text of the open paragraph / list item, parsed once when it closes.
  let para: string[] = [];
  // `topIndent`: the indent of the list's first bullet; nesting is relative to it.
  let list: { topIndent: number; top: { parts: string[]; subs: string[][] }[] } | null = null;
  // The text part list the next continuation line joins, when the previous line belonged to an item.
  let target: string[] | null = null;

  const closeParagraph = () => {
    if (para.length > 0 && room()) {
      blocks.push({ kind: "paragraph", inline: parseInline(para.join(" "), budget, true) });
    }
    para = [];
  };
  const closeList = () => {
    if (list) {
      const items: ListItem[] = [];
      for (const it of list.top) {
        items.push({
          inline: parseInline(it.parts.join(" "), budget, true),
          sub: it.subs.map((s) => parseInline(s.join(" "), budget, true)),
        });
      }
      if (items.length > 0) blocks.push({ kind: "list", items });
    }
    list = null;
    target = null;
  };

  for (const line of text.split("\n")) {
    const m = measure(line);
    if (!m) {
      closeParagraph();
      // A blank line ends continuation of the current item (a later indented line starts a paragraph).
      target = null;
      continue;
    }
    const heading = headingOf(line, m.at, m.indent);
    // A lone "#" (no text) is not a heading: it stays literal text.
    if (heading && heading.text !== "") {
      closeParagraph();
      closeList();
      if (room()) blocks.push({ kind: "heading", level: heading.level, inline: parseInline(heading.text, budget, true) });
      continue;
    }
    const bullet = bulletText(line, m.at);
    if (bullet !== null) {
      closeParagraph();
      if (!list) list = { topIndent: m.indent, top: [] };
      if (!room()) {
        target = null;
        continue;
      }
      const parts = bullet === "" ? [] : [bullet];
      if (m.indent >= list.topIndent + 2 && list.top.length > 0) {
        const last = list.top[list.top.length - 1];
        if (last) {
          last.subs.push(parts);
          target = parts;
        }
      } else {
        list.top.push({ parts, subs: [] });
        target = parts;
      }
      continue;
    }
    const trimmed = line.trim();
    if (target && m.indent >= 1) {
      target.push(trimmed);
      continue;
    }
    closeList();
    para.push(trimmed);
  }
  closeParagraph();
  closeList();
  return { blocks, truncated: budget.truncated };
}
