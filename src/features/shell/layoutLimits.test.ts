// The window's minimum size (src-tauri/tauri.conf.json) against the layout limits and the CSS sizes they assume.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { ACTIVITY_BAR_PX, MAIN_MIN, SEPARATOR_PX, SHELL_MIN_WIDTH, SIDEBAR_MIN, sidebarMaxPx } from "./layoutLimits";

const conf = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8")) as {
  app: { windows: { label: string; minWidth: number; minHeight: number }[] };
};
const main = conf.app.windows.find((w) => w.label === "main");
const tokens = readFileSync("src/styles/tokens.css", "utf8");

function tokenPx(name: string): number {
  const match = new RegExp(`${name}: (\\d+)px;`).exec(tokens);
  if (!match) throw new Error(`${name} not found in tokens.css`);
  return Number(match[1]);
}

describe("window minimum size", () => {
  it("the activity bar width matches --w-activity", () => {
    expect(ACTIVITY_BAR_PX).toBe(tokenPx("--w-activity"));
  });

  it("minWidth keeps the open sidebar and the main area at their minimum", () => {
    expect(SHELL_MIN_WIDTH).toBe(ACTIVITY_BAR_PX + SIDEBAR_MIN + SEPARATOR_PX + MAIN_MIN);
    expect(main?.minWidth).toBeGreaterThanOrEqual(SHELL_MIN_WIDTH);
    // At the minimum width the sidebar can still have its minimum width.
    expect(sidebarMaxPx(main?.minWidth ?? 0)).toBeGreaterThanOrEqual(SIDEBAR_MIN);
  });

  it("minHeight keeps the title bar, the status bar, the editor and the terminal panel at their minimum", () => {
    const panelMin = 120; // TERMINAL_MIN in Shell.tsx (editor and terminal panels)
    const needed = tokenPx("--h-titlebar") + tokenPx("--h-status") + 2 * panelMin + SEPARATOR_PX;
    expect(main?.minHeight).toBeGreaterThanOrEqual(needed);
  });
});
