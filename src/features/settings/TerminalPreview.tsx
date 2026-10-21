// Settings > Terminal: a static look-alike of the terminal (docs/ARCHITECTURE.md §8.6). Pure React/CSS, no xterm
// instance and no server data: the colours come from `buildTerminalTheme` (the same palette the real terminal uses for
// the shown theme and `terminal.background`), the font, size, line height, letter spacing and cursor from the form.
// Ligatures are always off, as in the real terminal.
import type { CSSProperties } from "react";
import { useTranslation } from "react-i18next";
import type { CursorStyle } from "../../ipc/types";
import { terminalFontStack } from "../../lib/fontFamily";
import type { AppearanceForm } from "../../lib/settingsFormAppearance";
import { resolveTheme } from "../../lib/theme";
import { buildTerminalTheme } from "../terminal/terminalTheme";
import { systemPrefersDark } from "./appearance";
import "./terminalPreview.css";

export interface TerminalPreviewProps {
  look: AppearanceForm;
  /** Font text already validated by the caller (an invalid one previews as the bundled font). */
  fontFamily: string | null;
  /** Raw field text; out-of-range values fall back to the defaults below. */
  fontSize: string | undefined;
  lineHeight: number | undefined;
  letterSpacing: string | undefined;
  cursorStyle: CursorStyle;
  cursorBlink: boolean;
}

const DEFAULT_SIZE = 13;
const DEFAULT_LINE_HEIGHT = 1.2;

// Static demo text (technical, not UI copy).
const PROMPT_USER = "deploy@web-01";
const PROMPT_PATH = "~";
const COMMAND = "ls -la";
const PROMPT_COLON = ":";
const PROMPT_SIGN = "$ ";
const CURSOR_CELL = " ";
const LISTING: ReadonlyArray<{ mode: string; name: string; color: "blue" | "green" | "cyan" | "red" | "default"; link?: string }> = [
  { mode: "drwxr-xr-x  4 deploy deploy 4096 Oct 21 09:12 ", name: "app", color: "blue" },
  { mode: "-rwxr-xr-x  1 deploy deploy  812 Oct 21 09:10 ", name: "deploy.sh", color: "green" },
  { mode: "lrwxrwxrwx  1 deploy deploy   11 Oct 21 09:10 ", name: "current", color: "cyan", link: " -> releases/42" },
  { mode: "-rw-r--r--  1 deploy deploy  220 Oct 21 09:08 ", name: "error.log", color: "red" },
  { mode: "-rw-r--r--  1 deploy deploy   64 Oct 21 09:08 ", name: "notes.txt", color: "default" },
];

function toNumber(text: string | undefined, min: number, max: number, fallback: number): number {
  const n = Number(text);
  return text !== undefined && text.trim() !== "" && Number.isFinite(n) && n >= min && n <= max ? n : fallback;
}

export function TerminalPreview({ look, fontFamily, fontSize, lineHeight, letterSpacing, cursorStyle, cursorBlink }: TerminalPreviewProps) {
  const { t } = useTranslation();
  const shown = resolveTheme(look.appearance.theme, systemPrefersDark());
  const theme = buildTerminalTheme(look.terminalBackground, shown, look.appearance.accent);
  const colors = {
    blue: theme.blue,
    green: theme.green,
    cyan: theme.cyan,
    red: theme.red,
    magenta: theme.magenta,
    default: undefined,
  } as const;
  const style: CSSProperties = {
    background: theme.background,
    color: theme.foreground,
    fontFamily: terminalFontStack(fontFamily),
    fontSize: `${toNumber(fontSize, 8, 40, DEFAULT_SIZE)}px`,
    lineHeight: lineHeight !== undefined && Number.isFinite(lineHeight) ? lineHeight : DEFAULT_LINE_HEIGHT,
    letterSpacing: `${toNumber(letterSpacing, -5, 20, 0)}px`,
    fontVariantLigatures: "none",
    ["--term-cursor" as string]: theme.cursor,
    ["--term-cursor-accent" as string]: theme.cursorAccent,
  };
  const prompt = (
    <>
      <span style={{ color: theme.green }}>{PROMPT_USER}</span>
      <span>{PROMPT_COLON}</span>
      <span style={{ color: theme.blue }}>{PROMPT_PATH}</span>
      <span>{PROMPT_SIGN}</span>
    </>
  );
  return (
    <div className="term-preview mono" role="img" aria-label={t("settings.previewTerminal")} style={style}>
      <div className="term-preview__line">
        {prompt}
        <span>{COMMAND}</span>
      </div>
      {LISTING.map((row) => (
        <div key={row.name} className="term-preview__line">
          <span>{row.mode}</span>
          <span style={{ color: colors[row.color] }}>{row.name}</span>
          {row.link ? <span>{row.link}</span> : null}
        </div>
      ))}
      <div className="term-preview__line">
        {prompt}
        <span className={`term-preview__cursor term-preview__cursor--${cursorStyle}${cursorBlink ? " term-preview__cursor--blink" : ""}`}>
          {CURSOR_CELL}
        </span>
      </div>
    </div>
  );
}
