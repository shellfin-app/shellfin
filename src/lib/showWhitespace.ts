// `editor.showWhitespace` modes (docs/ARCHITECTURE.md §8.11). Earlier builds stored a boolean; the backend converts it
// on load, and this helper does the same for any value that still reaches the UI (an older payload, a test fixture).
import type { ShowWhitespace } from "../ipc/types";
import { SHOW_WHITESPACE } from "./identifiers";

export const SHOW_WHITESPACE_MODES: readonly ShowWhitespace[] = [SHOW_WHITESPACE.all, SHOW_WHITESPACE.activeLine, SHOW_WHITESPACE.none];

/** The mode for a stored value: a mode stays itself, `true` = all, `false` and anything else = none (the default). */
export function showWhitespaceMode(value: unknown): ShowWhitespace {
  if (value === true) return SHOW_WHITESPACE.all;
  return SHOW_WHITESPACE_MODES.find((mode) => mode === value) ?? SHOW_WHITESPACE.none;
}
