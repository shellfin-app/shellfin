import type { ITheme } from "@xterm/xterm";

/** xterm.js theme matching "A · Graphite" (values mirror styles/tokens.css; canvas renderers cannot read CSS vars). */
export const xtermTheme: ITheme = {
  background: "#0F1115",
  foreground: "#C9CED8",
  cursor: "#4FD1C5",
  cursorAccent: "#0F1115",
  selectionBackground: "#2A3A45",
  black: "#1B1E25",
  red: "#F87171",
  green: "#4ADE80",
  yellow: "#E5C07B",
  blue: "#8AB4F8",
  magenta: "#C792EA",
  cyan: "#4FD1C5",
  white: "#D7DCE4",
  brightBlack: "#6B7385",
  brightRed: "#FCA5A5",
  brightGreen: "#86EFAC",
  brightYellow: "#F0D8A0",
  brightBlue: "#B3CDFB",
  brightMagenta: "#DDB8F5",
  brightCyan: "#99E6DE",
  brightWhite: "#F2F4F8",
};

/** Search-match highlighting (the search addon requires #RRGGBB values). */
export const xtermSearchDecorations = {
  matchBackground: "#34404D",
  matchBorder: "#8AB4F8",
  matchOverviewRuler: "#8AB4F8",
  activeMatchBackground: "#6B5A2A",
  activeMatchBorder: "#E5C07B",
  activeMatchColorOverviewRuler: "#E5C07B",
};

/** CSS font shorthand for `document.fonts.load()`. */
export function terminalFontSpec(sizePx: number): string {
  return `${sizePx}px "JetBrains Mono"`;
}
