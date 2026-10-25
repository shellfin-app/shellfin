import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { CAPTION_ZONE_HEIGHT, CAPTION_ZONE_WIDTH, MENU_EDGE, placeMenu } from "./menuPlacement";

const base = { width: 200, height: 150, viewportWidth: 1000, viewportHeight: 700 };

describe("placeMenu", () => {
  it("keeps the requested position when it fits", () => {
    expect(placeMenu({ ...base, x: 300, y: 300, windows: true })).toEqual({ left: 300, top: 300 });
  });

  it("clamps into the viewport", () => {
    expect(placeMenu({ ...base, x: -50, y: -50, windows: false })).toEqual({ left: MENU_EDGE, top: MENU_EDGE });
    expect(placeMenu({ ...base, x: 990, y: 690, windows: false })).toEqual({ left: 1000 - 200 - MENU_EDGE, top: 700 - 150 - MENU_EDGE });
  });

  it("Windows: a menu reaching into the top-right caption zone moves below the title bar", () => {
    const placed = placeMenu({ ...base, x: 990, y: 10, windows: true });
    expect(placed.left).toBe(1000 - 200 - MENU_EDGE);
    expect(placed.top).toBe(CAPTION_ZONE_HEIGHT + MENU_EDGE);
  });

  it("Windows: menus beside the zone or below the title bar stay where they are", () => {
    expect(placeMenu({ ...base, x: 100, y: 10, windows: true })).toEqual({ left: 100, top: 10 });
    expect(placeMenu({ ...base, x: 990, y: 60, windows: true })).toEqual({ left: 794, top: 60 });
  });

  it("other platforms keep the clamped position at the top right", () => {
    expect(placeMenu({ ...base, x: 990, y: 10, windows: false })).toEqual({ left: 794, top: 10 });
  });

  it("stays inside the viewport in a very short window", () => {
    const placed = placeMenu({ ...base, viewportHeight: 120, x: 990, y: 0, windows: true });
    expect(placed.top).toBeGreaterThanOrEqual(MENU_EDGE);
    expect(placed.left + base.width).toBeLessThanOrEqual(1000 - CAPTION_ZONE_WIDTH);
  });

  it("Windows: a tall menu at the top right moves left of the caption zone, not back into it", () => {
    const tall = { ...base, height: 680, viewportHeight: 700 };
    const placed = placeMenu({ ...tall, x: 990, y: 10, windows: true });
    expect(placed.top).toBe(10);
    expect(placed.left).toBe(1000 - CAPTION_ZONE_WIDTH - 200 - MENU_EDGE);
    const overlaps = placed.left + 200 > 1000 - CAPTION_ZONE_WIDTH && placed.top < CAPTION_ZONE_HEIGHT;
    expect(overlaps).toBe(false);
    // Other platforms keep it at the right edge.
    expect(placeMenu({ ...tall, x: 990, y: 10, windows: false }).left).toBe(1000 - 200 - MENU_EDGE);
  });

  it("a menu taller than the window scrolls (CSS), the menu bar dropdown keeps its own limit", () => {
    const menuCss = readFileSync("src/features/menu/menu.css", "utf8");
    const rule = menuCss.slice(menuCss.indexOf(".context-menu {"), menuCss.indexOf("}", menuCss.indexOf(".context-menu {")));
    expect(rule).toContain(`max-height: calc(100vh - ${2 * MENU_EDGE}px);`);
    expect(rule).toContain("overflow: auto;");
    expect(readFileSync("src/features/shell/shell.css", "utf8")).toContain(".context-menu.menubar__dropdown { min-width: 240px; max-height: calc(100vh - 56px);");
  });

  it("Windows: never overlaps the zone from any spot of a small window", () => {
    for (const height of [100, 300, 440, 488]) {
      for (const x of [0, 200, 400, 579]) {
        for (const y of [0, 20, 43, 300]) {
          const p = placeMenu({ x, y, width: 240, height, viewportWidth: 580, viewportHeight: 500, windows: true });
          const overlaps = p.left + 240 > 580 - CAPTION_ZONE_WIDTH && p.top < CAPTION_ZONE_HEIGHT;
          expect(overlaps, `${height} ${x} ${y}`).toBe(false);
          expect(p.left).toBeGreaterThanOrEqual(MENU_EDGE);
          expect(p.top).toBeGreaterThanOrEqual(MENU_EDGE);
        }
      }
    }
  });
});
