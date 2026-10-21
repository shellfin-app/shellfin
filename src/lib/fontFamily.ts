// Terminal font family: validation (mirrors the backend rule), the CSS stack and detection of the
// monospace fonts installed on this machine. Local system fonts need no network, so the CSP is untouched.
export const BUNDLED_FONT = "JetBrains Mono";
const FALLBACK = `"${BUNDLED_FONT}", ui-monospace, monospace`;

export const FONT_FAMILY_MAX = 200;
/**
 * Same rule as the backend (`storage/settings.rs`): ASCII letters, digits, spaces and - _ ' " , only (names, no CSS
 * functions or separators), at most FONT_FAMILY_MAX bytes. ASCII-only keeps the length in characters equal to bytes.
 */
const FONT_FAMILY_RE = /^[A-Za-z0-9 \-_'",]+$/;

export type FontFamilyError = "empty" | "tooLong" | "badChars";

/** `null` = valid. The text is checked trimmed; a list needs at least one non-blank name. */
export function validateFontFamily(raw: string): FontFamilyError | null {
  const text = raw.trim();
  if (text.length === 0) return "empty";
  if (text.length > FONT_FAMILY_MAX) return "tooLong";
  if (!FONT_FAMILY_RE.test(text)) return "badChars";
  return text.split(",").every((n) => n.replace(/["']/g, "").trim() === "") ? "empty" : null;
}

const GENERIC = new Set(["monospace", "ui-monospace", "sans-serif", "serif", "system-ui", "cursive", "fantasy"]);

/** The CSS `font-family` for xterm: the chosen list first, always followed by the bundled font and `monospace`. */
export function terminalFontStack(family: string | null | undefined): string {
  const text = family?.trim();
  if (!text || text === BUNDLED_FONT || validateFontFamily(text) !== null) return FALLBACK;
  const names = text
    .split(",")
    .map((n) => n.trim())
    .filter((n) => n !== "")
    .map((n) => (/["']/.test(n) || GENERIC.has(n.toLowerCase()) ? n : `"${n}"`));
  return `${names.join(", ")}, ${FALLBACK}`;
}

export const FONT_CANDIDATES = [
  "Cascadia Code",
  "Cascadia Mono",
  "Consolas",
  "Fira Code",
  "Source Code Pro",
  "Ubuntu Mono",
  "DejaVu Sans Mono",
  "Menlo",
  "SF Mono",
  "Monaco",
  "Hack",
  "Iosevka",
  "Roboto Mono",
  "Courier New",
] as const;

/**
 * `document.fonts.check()` alone answers true for unknown families (nothing to load), so it is combined with a canvas
 * measurement against two generic fallbacks: an installed font changes the width of the probe text, a missing one
 * renders as the fallback. Without a canvas (jsdom) the check result alone is used.
 */
export function detectInstalledFonts(candidates: readonly string[] = FONT_CANDIDATES): string[] {
  const fonts = typeof document !== "undefined" ? document.fonts : undefined;
  if (!fonts || typeof fonts.check !== "function") return [];
  let ctx: CanvasRenderingContext2D | null = null;
  try {
    ctx = document.createElement("canvas").getContext("2d");
  } catch {
    ctx = null;
  }
  const probe = "mmmmmmmmmmlli1WO0@#";
  const width = (font: string): number => {
    if (!ctx) return 0;
    ctx.font = `72px ${font}`;
    return ctx.measureText(probe).width;
  };
  const bases = ctx ? (["monospace", "serif"] as const).map((b) => [b, width(b)] as const) : [];
  return candidates.filter((name) => {
    try {
      if (!fonts.check(`12px "${name}"`)) return false;
      if (!ctx) return true;
      return bases.some(([base, w]) => width(`"${name}", ${base}`) !== w);
    } catch {
      return false;
    }
  });
}
