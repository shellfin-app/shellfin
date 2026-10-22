// Untrusted server content (docs/ARCHITECTURE.md §9.5). Every server-originated string shown in the UI goes
// through displayServerText: C0/C1 controls and bidi controls become visible escapes, then it is truncated.

// Code point ranges: C0 (0-1F), DEL + C1 (7F-9F), plus the characters that render as nothing or reorder text (the
// same set as the Rust `is_deceptive_char`, ssh/shellquote.rs, with two differences: the frontend also escapes
// U+FFF9-FFFB, which the Rust set leaves out of U+FFF0-FFF8, and U+FE0E/U+FE0F are context-aware, see below).
// The variation selectors U+FE00-FE0D (VS1-14) are always escaped: they are rarely used in names (legitimate in
// standardized variation sequences such as CJK compatibility or math symbols; such names show the marker) and they
// make two different names look identical. The supplementary ones U+E0100-E01EF fall inside the tag range below.
// Covered: soft hyphen, combining grapheme joiner, Hangul/Khmer/Mongolian fillers and selectors, zero-width space
// and joiners, LRM/RLM, bidi embeddings and isolates, line/paragraph separators, word joiner and invisible
// operators, BOM, interlinear annotation marks and Unicode tag characters.
// U+FE0E (text) and U+FE0F (emoji presentation) are kept only where they change how the previous character
// renders (see keepPresentationSelector); anywhere else they are invisible and escaped.
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
  [0xfe00, 0xfe0d],
  [0xfeff, 0xfeff],
  [0xffa0, 0xffa0],
  [0xfff0, 0xfffb],
  [0xe0000, 0xe0fff],
];

const CONTROL_RE = new RegExp(
  `[${CONTROL_RANGES.map(([a, b]) => `${String.fromCodePoint(a)}-${String.fromCodePoint(b)}`).join("")}]`,
  "gu",
);

// The controls above plus the two presentation selectors, which are decided per occurrence.
const SCAN_RE = new RegExp(`${CONTROL_RE.source}|[${String.fromCodePoint(0xfe0e)}${String.fromCodePoint(0xfe0f)}]`, "gu");

const EMOJI_RE = /\p{Emoji}/u;
const EMOJI_PRESENTATION_RE = /\p{Emoji_Presentation}/u;
const KEYCAP_BASE_RE = /[0-9#*]/u;
const VS_TEXT = 0xfe0e;
const VS_EMOJI = 0xfe0f;
const KEYCAP = 0x20e3;

/** The code point that ends right before `index` (handles surrogate pairs), or undefined at the start. */
function codePointBefore(s: string, index: number): number | undefined {
  if (index <= 0) return undefined;
  const low = s.charCodeAt(index - 1);
  if (low >= 0xdc00 && low <= 0xdfff && index >= 2) {
    const high = s.charCodeAt(index - 2);
    if (high >= 0xd800 && high <= 0xdbff) return s.codePointAt(index - 2);
  }
  return low;
}

/** Keep U+FE0E/FE0F only when the selector really changes the presentation of the previous character. */
function keepPresentationSelector(sel: number, s: string, index: number): boolean {
  const prev = codePointBefore(s, index);
  if (prev === undefined || prev === VS_TEXT || prev === VS_EMOJI) return false;
  const base = String.fromCodePoint(prev);
  if (!EMOJI_RE.test(base)) return false;
  if (KEYCAP_BASE_RE.test(base)) return sel === VS_EMOJI && s.codePointAt(index + 1) === KEYCAP;
  return sel === VS_EMOJI ? !EMOJI_PRESENTATION_RE.test(base) : EMOJI_PRESENTATION_RE.test(base);
}

const BACKSLASH = String.fromCharCode(92);

function escapeChar(ch: string): string {
  const code = ch.codePointAt(0) ?? 0;
  // C0, DEL and C1 -> a visible backslash-x escape; the invisible and bidi characters -> U+XXXX in angle brackets.
  if (code < 0xa0) return `${BACKSLASH}x${code.toString(16).padStart(2, "0")}`;
  return `⟨U+${code.toString(16).toUpperCase().padStart(4, "0")}⟩`;
}

/** Make a server string safe to render as text. `keepNewlines` keeps newline and tab (banners, multi-line text). */
export function displayServerText(s: string, limit: number, keepNewlines = false): string {
  const escaped = s.replace(SCAN_RE, (ch, offset: number) => {
    if (keepNewlines && (ch === "\n" || ch === "\t")) return ch;
    const code = ch.codePointAt(0) ?? 0;
    if ((code === VS_TEXT || code === VS_EMOJI) && keepPresentationSelector(code, s, offset)) return ch;
    return escapeChar(ch);
  });
  const chars = Array.from(escaped);
  if (chars.length <= limit) return escaped;
  const kept = chars.slice(0, Math.max(0, limit - 1));
  // A cut between a kept FE0F and its keycap would leave the FE0F orphaned: a second pass over the (already escaped)
  // result would then escape it. Drop it together with the keycap so the function stays idempotent.
  const last = kept.length - 1;
  if (
    kept[last] === String.fromCodePoint(VS_EMOJI) &&
    chars[kept.length] === String.fromCodePoint(KEYCAP) &&
    KEYCAP_BASE_RE.test(kept[last - 1] ?? "")
  ) {
    kept.pop();
  }
  return `${kept.join("")}…`;
}

/** A remote file name / label for display: controls and bidi escaped, bounded (§9.5). */
export function displayName(name: string): string {
  return displayServerText(name, 256);
}

/** A remote path for display. */
export function displayPath(path: string): string {
  return displayServerText(path, 1024);
}
