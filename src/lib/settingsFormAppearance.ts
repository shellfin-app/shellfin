// Settings form <-> contract for `appearance.*` and `terminal.background` (docs/ARCHITECTURE.md §8.5).
import type { AppearanceSettings, Settings } from "../ipc/types";
import { clampUiFontSize, isHexColor } from "./theme";

/** The form part owned by the Appearance section and the terminal background field. */
export interface AppearanceForm {
  appearance: AppearanceSettings;
  /** `null` = follow the theme, else exactly `#RRGGBB` (upper-case). */
  terminalBackground: string | null;
  /** Raw text of the custom colour field (kept while it is being typed, may be invalid). */
  terminalBackgroundText: string;
}

export const APPEARANCE_FIELD = { UI_FONT_SIZE: "appearance.uiFontSize", TERMINAL_BACKGROUND: "terminal.background" } as const;

export function appearanceFromSettings(s: Settings): AppearanceForm {
  return {
    appearance: s.appearance,
    terminalBackground: s.terminal.background,
    terminalBackgroundText: s.terminal.background ?? "",
  };
}

/** Normalises typed colour text: trims, adds a missing `#`, upper-cases. `null` when it is not exactly `#RRGGBB`. */
export function parseHexColor(text: string): string | null {
  const trimmed = text.trim();
  const withHash = trimmed.startsWith("#") ? trimmed : `#${trimmed}`;
  return isHexColor(withHash) ? withHash.toUpperCase() : null;
}

/** Field paths with an unusable value. The custom colour text is only checked while a custom colour is being typed. */
export function appearanceInvalidFields(f: AppearanceForm): string[] {
  const bad: string[] = [];
  const size = f.appearance.uiFontSize;
  if (!Number.isInteger(size) || clampUiFontSize(size) !== size) bad.push(APPEARANCE_FIELD.UI_FONT_SIZE);
  if (f.terminalBackgroundText.trim() !== "" && parseHexColor(f.terminalBackgroundText) === null) {
    // The typed text is not a colour (so not committed): saving would silently ignore it, so flag it.
    bad.push(APPEARANCE_FIELD.TERMINAL_BACKGROUND);
  }
  return bad;
}

export function applyAppearance(base: Settings, f: AppearanceForm): Settings {
  return {
    ...base,
    appearance: f.appearance,
    terminal: { ...base.terminal, background: f.terminalBackground },
  };
}
