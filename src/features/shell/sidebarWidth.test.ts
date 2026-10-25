import { describe, expect, it } from "vitest";
import { clampSidebarWidth } from "./layoutLimits";
import { DRAG_RELEASE_GRACE_MS, KEY_RESIZE_WINDOW_MS, createSidebarWidthTracker } from "./sidebarWidth";

describe("sidebar width tracker", () => {
  it("a layout clamp (narrow window) is not stored and does not change the user's width", () => {
    let now = 1000;
    const tracker = createSidebarWidthTracker(360, () => now);
    // The window narrows to 580 px: the shell clamps the sidebar (main area keeps 320 px) and the panel reports it.
    const clamped = clampSidebarWidth(tracker.userWidth(), 580);
    expect(clamped).toBe(208);
    expect(tracker.onResize(clamped)).toBeNull();
    expect(tracker.userWidth()).toBe(360);
    // Widening again gives the user's width back (also not stored again).
    now += 10_000;
    expect(clampSidebarWidth(tracker.userWidth(), 1600)).toBe(360);
    expect(tracker.onResize(360)).toBeNull();
  });

  it("a drag on the separator is stored, until shortly after the pointer is released", () => {
    let now = 0;
    const tracker = createSidebarWidthTracker(280, () => now);
    tracker.pointerDown();
    expect(tracker.onResize(301.6)).toBe(302);
    expect(tracker.onResize(320)).toBe(320);
    tracker.pointerUp();
    now += DRAG_RELEASE_GRACE_MS + 1;
    expect(tracker.onResize(200)).toBeNull();
    expect(tracker.userWidth()).toBe(320);
  });

  it("a size reported after pointerup (release in the same frame as the last move) is saved", () => {
    let now = 1000;
    const tracker = createSidebarWidthTracker(300, () => now);
    tracker.pointerDown();
    expect(tracker.onResize(340)).toBe(340);
    tracker.pointerUp();
    now += 16; // the library's final report, one frame later
    expect(tracker.onResize(350)).toBe(350);
    expect(tracker.userWidth()).toBe(350);
    // A later layout change is not the user's.
    now += DRAG_RELEASE_GRACE_MS + 1;
    expect(tracker.onResize(208)).toBeNull();
    expect(tracker.userWidth()).toBe(350);
  });

  it("a pointer release elsewhere (no drag) opens no grace window", () => {
    let now = 1000;
    const tracker = createSidebarWidthTracker(300, () => now);
    tracker.pointerUp();
    now += 5;
    expect(tracker.onResize(208)).toBeNull();
  });

  it("a keyboard resize right after a key on the separator is stored; a later layout change is not", () => {
    let now = 0;
    const tracker = createSidebarWidthTracker(280, () => now);
    tracker.key();
    now += 20;
    expect(tracker.onResize(290)).toBe(290);
    now += KEY_RESIZE_WINDOW_MS + 1;
    expect(tracker.onResize(200)).toBeNull();
    expect(tracker.userWidth()).toBe(290);
  });

  it("ignores sizes that are not a width", () => {
    const tracker = createSidebarWidthTracker(280, () => 0);
    tracker.pointerDown();
    for (const bad of [0, -5, Number.NaN, Number.POSITIVE_INFINITY]) expect(tracker.onResize(bad)).toBeNull();
    expect(tracker.userWidth()).toBe(280);
  });
});
