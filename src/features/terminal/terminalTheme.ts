// xterm.js theme from the app theme and `terminal.background` (docs/ARCHITECTURE.md §8.6). Canvas renderers cannot read
// CSS variables, so the palettes are values here; the light one is the "Light theme tokens" terminal palette.
import type { ITheme } from "@xterm/xterm";
import type { AccentColor } from "../../ipc/types";
import { ACCENT_VALUES, currentResolvedTheme, isHexColor, toneOfBackground, type ResolvedTheme } from "../../lib/theme";
import { xtermSearchDecorations, xtermTheme } from "../../styles/xtermTheme";
import { useStore } from "../../state/store";
import { shownAppearance } from "../settings/appearance";

/** Light terminal palette: every colour reaches 4.5:1 on white and on warm paper (#FBF9F4); test: terminalTheme.test.ts. */
export const LIGHT_PALETTE = {
  background: "#FFFFFF",
  foreground: "#1F2329",
  cursor: "#1F2329",
  selectionBackground: "#CFDDF3",
  black: "#3B4048",
  red: "#C62828",
  green: "#15803D",
  yellow: "#8A5A00",
  blue: "#1F5FBF",
  magenta: "#8A3FB8",
  cyan: "#0E7490",
  white: "#6B7280",
  brightBlack: "#656C78",
  brightRed: "#B91C1C",
  brightGreen: "#166534",
  brightYellow: "#7A4F00",
  brightBlue: "#1D4ED8",
  brightMagenta: "#7E22CE",
  brightCyan: "#0F766E",
  brightWhite: "#4A515C",
} as const satisfies ITheme;

/** The palette (text colours) that suits a background: by relative luminance, > 0.16 = light palette. */
export function paletteFor(background: string | null, appTheme: ResolvedTheme): ResolvedTheme {
  return background !== null && isHexColor(background) ? toneOfBackground(background) : appTheme;
}

/**
 * `background === null` follows the app theme (dark: the existing terminal colours; light: the light palette).
 * A `#RRGGBB` value is the background, with foreground, cursor, selection and the 16 ANSI colours taken from the dark
 * or light palette by the background's luminance.
 */
export function buildTerminalTheme(background: string | null, appTheme: ResolvedTheme, accent: AccentColor = "teal"): ITheme {
  const custom = background !== null && isHexColor(background) ? background : null;
  const palette = paletteFor(custom, appTheme);
  if (palette === "light") {
    const bg = custom ?? LIGHT_PALETTE.background;
    return { ...LIGHT_PALETTE, background: bg, cursorAccent: bg };
  }
  const bg = custom ?? xtermTheme.background ?? "#0F1115";
  return { ...xtermTheme, background: bg, cursor: ACCENT_VALUES[accent].dark, cursorAccent: bg };
}

/** Search-match colours for the addon (it requires #RRGGBB). */
export function searchDecorationsFor(palette: ResolvedTheme): typeof xtermSearchDecorations {
  if (palette === "dark") return xtermSearchDecorations;
  return {
    matchBackground: "#DCE8FA",
    matchBorder: "#1F5FBF",
    matchOverviewRuler: "#1F5FBF",
    activeMatchBackground: "#FFE9A8",
    activeMatchBorder: "#A15C00",
    activeMatchColorOverviewRuler: "#A15C00",
  };
}

/** Background and accent on screen: an open settings preview wins over the saved settings. */
function shownInputs(): { background: string | null; accent: AccentColor } {
  const shown = shownAppearance();
  if (shown) return { background: shown.terminalBackground, accent: shown.appearance.accent };
  const settings = useStore.getState().settings;
  return { background: settings?.terminal.background ?? null, accent: settings?.appearance.accent ?? "teal" };
}

/** The theme for what is shown on screen (saved settings or the settings dialog's live preview). */
export function currentTerminalTheme(): ITheme {
  const { background, accent } = shownInputs();
  return buildTerminalTheme(background, currentResolvedTheme(), accent);
}

export function currentSearchDecorations(): typeof xtermSearchDecorations {
  return searchDecorationsFor(paletteFor(shownInputs().background, currentResolvedTheme()));
}
