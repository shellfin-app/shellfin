// Menu bar mnemonics. Pure helpers, shared by the menu bar (Alt+letter, the underlined letter), the macOS native menu
// labels and the `npm run i18n:check` guard (`scripts/menu-mnemonics-check.mjs`), so all of them resolve a mnemonic
// the same way.
// WARNING: imported directly by Node (`scripts/menu-mnemonics-check.mjs`, type stripping): erasable TypeScript syntax
// only (no enums, namespaces or parameter properties), and value imports need explicit `.ts` extensions.
//
// Title syntax (Windows convention, ARCHITECTURE §8.2.2): `&` marks the next character as the mnemonic ("Ay&uda",
// "ファイル(&F)"), `&&` is a literal ampersand. A title without a marker uses its first character (English, Ukrainian).

/** Problems of a title's `&` markers; the check script reports them, the UI tolerates them. */
export type MarkerProblem = "multiple" | "trailing" | "beforeSpace";

export interface ParsedMenuTitle {
  /** Display text before the mnemonic character (markers removed, `&&` unescaped). */
  before: string;
  /** The mnemonic character as written ("" for an empty title). */
  mnemonic: string;
  /** Display text after the mnemonic character. */
  after: string;
  /** The whole display text: `before + mnemonic + after`. */
  plain: string;
  /** Whether the mnemonic comes from an explicit `&` marker (else it is the first character). */
  marked: boolean;
  problems: MarkerProblem[];
}

const segmenter = typeof Intl.Segmenter === "function" ? new Intl.Segmenter(undefined, { granularity: "grapheme" }) : null;

/**
 * User-perceived characters (grapheme clusters), so a base letter keeps its combining marks ("e" + U+0301) in the
 * underline span and in the mnemonic. Falls back to code points where `Intl.Segmenter` is missing.
 */
function graphemes(s: string): string[] {
  return segmenter ? Array.from(segmenter.segment(s), (g) => g.segment) : Array.from(s);
}

/** Splits a menu title into the display text and its mnemonic character (see the syntax above). */
export function parseMenuTitle(title: string): ParsedMenuTitle {
  const chars = graphemes(title);
  let text = "";
  let markAt = -1;
  let mark = "";
  const problems: MarkerProblem[] = [];
  for (let i = 0; i < chars.length; i += 1) {
    const ch = chars[i] ?? "";
    if (ch !== "&") {
      text += ch;
      continue;
    }
    const next = chars[i + 1];
    if (next === "&") {
      text += "&";
      i += 1;
    } else if (next === undefined) {
      if (!problems.includes("trailing")) problems.push("trailing");
    } else if (/\s/u.test(next)) {
      if (!problems.includes("beforeSpace")) problems.push("beforeSpace");
    } else if (markAt >= 0) {
      if (!problems.includes("multiple")) problems.push("multiple");
    } else {
      // The first marker wins (a second one is a problem, its character stays plain text).
      markAt = text.length;
      mark = next;
      text += next;
      i += 1;
    }
  }
  if (markAt >= 0) {
    return { before: text.slice(0, markAt), mnemonic: mark, after: text.slice(markAt + mark.length), plain: text, marked: true, problems };
  }
  const first = graphemes(text)[0] ?? "";
  return { before: "", mnemonic: first, after: text.slice(first.length), plain: text, marked: false, problems };
}

/** The title as plain text (markers removed): aria-labels, help dialog headings, anything that is not the menu bar. */
export function plainMenuTitle(title: string): string {
  return parseMenuTitle(title).plain;
}

/**
 * The title for a heading or a menu without mnemonics: markers removed and a CJK-style "(F)" mnemonic group (ASCII or
 * full-width parentheses) dropped entirely, so "ファイル(&F)" becomes "ファイル" (what Japanese macOS menus read).
 * The result is display text: a label for the macOS native menu still needs `escapeNativeLabel`.
 */
export function nativeMenuTitle(title: string): string {
  const p = parseMenuTitle(title);
  const open = p.before.at(-1);
  const close = p.after[0];
  if (p.marked && ((open === "(" && close === ")") || (open === "（" && close === "）"))) {
    const rest = `${p.before.slice(0, -1).trimEnd()}${p.after.slice(1)}`;
    if (rest.trim() !== "") return rest;
  }
  return p.plain;
}

/**
 * Display text as a label for the macOS native menu. muda (the menu crate) reads `&` as a mnemonic marker on macOS
 * too and strips it (`&&` shows one `&`, a single `&` disappears), so every literal `&` is doubled. At most `max`
 * code points after escaping (the backend's limit counts Rust `char`s), never ending in half of an `&&` pair.
 */
export function escapeNativeLabel(text: string, max = Number.POSITIVE_INFINITY): string {
  let out = "";
  let length = 0;
  for (const ch of Array.from(text)) {
    const size = ch === "&" ? 2 : 1;
    if (length + size > max) break;
    out += ch === "&" ? "&&" : ch;
    length += size;
  }
  return out;
}

/**
 * Normal form of a mnemonic or a typed key for comparison, lower-cased. Latin letters lose their accents ("É" and
 * "e" match: Alt+E opens "Édition" on AZERTY; "İ" and the Turkish dotless "ı" both become "i"). Other scripts keep
 * their letters (Cyrillic "й" is not "и", "ї" is not "і"), only composed (NFC).
 */
export function foldMnemonic(ch: string): string {
  const composed = ch.normalize("NFC");
  if (!/^\p{Script=Latin}/u.test(composed)) return composed.toLowerCase();
  return composed.normalize("NFD").replace(/\p{M}+/gu, "").toLowerCase().replaceAll("ı", "i");
}

/** The mnemonic of a menu title in its folded form (`foldMnemonic`). An empty title has none. */
export function mnemonicOf(title: string): string | undefined {
  const { mnemonic } = parseMenuTitle(title);
  return mnemonic === "" ? undefined : foldMnemonic(mnemonic);
}

/** The mnemonic character as written (the one the menu bar underlines); "" for an empty title. */
export function mnemonicChar(title: string): string {
  return parseMenuTitle(title).mnemonic;
}

/**
 * Whether Alt+letter can open a menu with this title. The menu bar's Alt handler accepts only keys with
 * `e.key.length === 1`, so the folded mnemonic must be a single UTF-16 unit, and it must be a letter or a digit (not
 * an empty title, an astral character, punctuation or a space).
 */
export function isMnemonicReachable(title: string): boolean {
  const m = mnemonicOf(title);
  return m !== undefined && m.length === 1 && /^[\p{L}\p{N}]$/u.test(m);
}

/** Whether a mnemonic (folded) is an ASCII letter: required for CJK titles (`scripts/menu-mnemonics-check.mjs`). */
export function isAsciiLetterMnemonic(title: string): boolean {
  return /^[a-z]$/.test(mnemonicOf(title) ?? "");
}

/** Han, Hiragana, Katakana or Hangul anywhere in the title: such a title needs an explicit ASCII-letter marker. */
export function hasCjkScript(title: string): boolean {
  return /[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Hangul}]/u.test(title);
}

export interface MnemonicCollision {
  letter: string;
  /** The menu ids (in the given order) whose titles resolve to `letter`. */
  ids: string[];
}

/**
 * Menus of one bar (id -> translated title) that share a mnemonic (after folding, so "Édition" and "Extras" collide).
 * Alt+letter opens the first of them, so the others are unreachable by keyboard. A title with no mnemonic (empty)
 * cannot collide.
 */
export function mnemonicCollisions(titles: Readonly<Record<string, string>>): MnemonicCollision[] {
  const byLetter = new Map<string, string[]>();
  for (const [id, title] of Object.entries(titles)) {
    const letter = mnemonicOf(title);
    if (letter === undefined) continue;
    byLetter.set(letter, [...(byLetter.get(letter) ?? []), id]);
  }
  return [...byLetter].filter(([, ids]) => ids.length > 1).map(([letter, ids]) => ({ letter, ids }));
}
