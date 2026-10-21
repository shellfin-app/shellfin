// Untrusted server content (docs/ARCHITECTURE.md §9.5). Every server-originated string shown in the UI goes
// through displayServerText: C0/C1 controls and bidi controls become visible escapes, then it is truncated.

// Code point ranges: C0 (0-1F), DEL + C1 (7F-9F), plus the characters that render as nothing or reorder text (the
// same set as the Rust `is_deceptive_char`, ssh/shellquote.rs, minus the variation selectors U+FE00-FE0F, which emoji
// need): soft hyphen, combining grapheme joiner, Hangul/Khmer/Mongolian fillers and selectors, zero-width space and
// joiners, LRM/RLM, bidi embeddings and isolates, line/paragraph separators, word joiner and invisible operators,
// BOM, interlinear annotation marks and Unicode tag characters.
// Listed as numbers (not as literal or escaped characters in a regex) so editors/formatters cannot strip or
// rewrite invisible characters.
const CONTROL_RANGES: readonly (readonly [number, number])[] = [
  [0x00, 0x1f],
  [0x7f, 0x9f],
  [0x00ad, 0x00ad],
  [0x034f, 0x034f],
  [0x061c, 0x061c],
  [0x115f, 0x1160],
  [0x17b4, 0x17b5],
  [0x180b, 0x180f],
  [0x200b, 0x200f],
  [0x2028, 0x202e],
  [0x2060, 0x206f],
  [0x3164, 0x3164],
  [0xfeff, 0xfeff],
  [0xffa0, 0xffa0],
  [0xfff0, 0xfffb],
  [0xe0000, 0xe0fff],
];

const CONTROL_RE = new RegExp(
  `[${CONTROL_RANGES.map(([a, b]) => `${String.fromCodePoint(a)}-${String.fromCodePoint(b)}`).join("")}]`,
  "gu",
);

const BACKSLASH = String.fromCharCode(92);

function escapeChar(ch: string): string {
  const code = ch.codePointAt(0) ?? 0;
  // C0, DEL and C1 -> a visible backslash-x escape; the invisible and bidi characters -> U+XXXX in angle brackets.
  if (code < 0xa0) return `${BACKSLASH}x${code.toString(16).padStart(2, "0")}`;
  return `⟨U+${code.toString(16).toUpperCase().padStart(4, "0")}⟩`;
}

/** Make a server string safe to render as text. `keepNewlines` keeps newline and tab (banners, multi-line text). */
export function displayServerText(s: string, limit: number, keepNewlines = false): string {
  const escaped = s.replace(CONTROL_RE, (ch) => (keepNewlines && (ch === "\n" || ch === "\t") ? ch : escapeChar(ch)));
  const chars = Array.from(escaped);
  return chars.length > limit ? `${chars.slice(0, Math.max(0, limit - 1)).join("")}…` : escaped;
}

/** A remote file name / label for display: controls and bidi escaped, bounded (§9.5). */
export function displayName(name: string): string {
  return displayServerText(name, 256);
}

/** A remote path for display. */
export function displayPath(path: string): string {
  return displayServerText(path, 1024);
}
