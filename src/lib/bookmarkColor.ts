// Bookmark colors (ARCHITECTURE §8.5): the backend sends a name, the page owns the palette (`--bm-<name>` in tokens.css).
// Color is never the only signal: every swatch/stripe carries the translated color name (aria-label / title).
import type { BookmarkColor } from "../ipc/types";

export const BOOKMARK_COLORS: readonly BookmarkColor[] = ["red", "orange", "yellow", "green", "teal", "blue", "purple", "gray"];

/** CSS value of a color name (a custom property from tokens.css). */
export function colorVar(color: BookmarkColor): string {
  return `var(--bm-${color})`;
}
