import { describe, expect, it } from "vitest";
import { contrastRatio } from "../../lib/theme";
import { xtermTheme } from "../../styles/xtermTheme";
import { makeBootstrap } from "../../ipc/mock/fixtures";
import { useStore } from "../../state/store";
import { endAppearancePreview, previewAppearance } from "../settings/appearance";
import { LIGHT_PALETTE, buildTerminalTheme, currentTerminalTheme, paletteFor, searchDecorationsFor } from "./terminalTheme";

const ANSI = [
  "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
  "brightBlack", "brightRed", "brightGreen", "brightYellow", "brightBlue", "brightMagenta", "brightCyan", "brightWhite",
] as const;

describe("buildTerminalTheme", () => {
  it("null follows the theme: dark keeps the existing terminal colours", () => {
    const theme = buildTerminalTheme(null, "dark");
    expect(theme.background).toBe(xtermTheme.background);
    expect(theme.foreground).toBe(xtermTheme.foreground);
    expect(theme.red).toBe(xtermTheme.red);
    expect(theme.cursor).toBe("#4FD1C5"); // teal accent, as before
  });

  it("null follows the theme: light uses the light palette", () => {
    const theme = buildTerminalTheme(null, "light");
    expect(theme).toMatchObject({ background: "#FFFFFF", foreground: "#1F2329", cursor: "#1F2329", selectionBackground: "#CFDDF3" });
    expect(theme.green).toBe("#15803D");
    expect(theme.blue).toBe("#1F5FBF");
    expect(theme.yellow).toBe("#8A5A00");
    expect(theme.red).toBe("#C62828");
  });

  it("the dark cursor follows the accent", () => {
    expect(buildTerminalTheme(null, "dark", "pink").cursor).toBe("#F29BC4");
  });

  it("a custom background wins over the theme, text colours follow its luminance", () => {
    const onBlack = buildTerminalTheme("#000000", "light");
    expect(onBlack.background).toBe("#000000");
    expect(onBlack.cursorAccent).toBe("#000000");
    expect(onBlack.foreground).toBe(xtermTheme.foreground); // dark palette: light text, even in the light app theme

    const onPaper = buildTerminalTheme("#FBF9F4", "dark");
    expect(onPaper.background).toBe("#FBF9F4");
    expect(onPaper.foreground).toBe(LIGHT_PALETTE.foreground); // light palette: dark text, even in the dark app theme
    expect(onPaper.red).toBe(LIGHT_PALETTE.red);
  });

  it("an invalid background is ignored (follows the theme)", () => {
    expect(buildTerminalTheme("not-a-color", "light").background).toBe("#FFFFFF");
  });

  it("paletteFor switches at relative luminance 0.16", () => {
    expect(paletteFor("#FFFFFF", "dark")).toBe("light");
    expect(paletteFor("#14171D", "light")).toBe("dark");
    expect(paletteFor("#5A5A5A", "light")).toBe("dark");
    expect(paletteFor("#808080", "dark")).toBe("light");
    expect(paletteFor("#B0B0B0", "dark")).toBe("light");
    expect(paletteFor(null, "light")).toBe("light");
    expect(paletteFor(null, "dark")).toBe("dark");
  });
});

describe("light terminal palette contrast", () => {
  for (const bg of ["#FFFFFF", "#FBF9F4"]) {
    it(`every colour reaches 4.5:1 on ${bg}`, () => {
      for (const key of ANSI) expect(contrastRatio(LIGHT_PALETTE[key], bg), key).toBeGreaterThanOrEqual(4.5);
      expect(contrastRatio(LIGHT_PALETTE.foreground, bg)).toBeGreaterThanOrEqual(4.5);
      expect(contrastRatio(LIGHT_PALETTE.cursor, bg)).toBeGreaterThanOrEqual(3);
    });
  }

  it("the text stays readable on the selection colour", () => {
    expect(contrastRatio(LIGHT_PALETTE.foreground, LIGHT_PALETTE.selectionBackground)).toBeGreaterThanOrEqual(4.5);
  });

  it("search highlights keep the text readable", () => {
    const d = searchDecorationsFor("light");
    expect(contrastRatio(LIGHT_PALETTE.foreground, d.matchBackground)).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(LIGHT_PALETTE.foreground, d.activeMatchBackground)).toBeGreaterThanOrEqual(4.5);
    for (const value of Object.values(d)) expect(value).toMatch(/^#[0-9A-F]{6}$/i);
  });
});

describe("currentTerminalTheme", () => {
  it("follows the settings dialog's live preview, then the saved settings again", () => {
    useStore.getState().setSettings(structuredClone(makeBootstrap(Date.now()).settings));
    previewAppearance({ theme: "dark", accent: "pink", uiFontSize: 13, reduceMotion: "system" }, "#FFFFFF");
    expect(currentTerminalTheme().background).toBe("#FFFFFF");
    expect(currentTerminalTheme().foreground).toBe(LIGHT_PALETTE.foreground);
    endAppearancePreview();
    expect(currentTerminalTheme().background).toBe(xtermTheme.background);
  });
});
