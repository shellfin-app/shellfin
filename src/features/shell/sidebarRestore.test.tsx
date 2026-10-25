// The sidebar shows the user's width clamped to the current window after every resize, also after a one-step widen
// (maximize, Snap), where react-resizable-panels keeps a percentage of the old window instead of the pixel width.
import { act, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import { flushLayoutPrefs, loadLayoutPrefs } from "../../state/layoutPrefs";
import { setupApp, settleApp } from "../../test/harness";
import { clampSidebarWidth } from "./layoutLimits";
import { sidebarCorrection } from "./sidebarWidth";

/** A stand-in for the sidebar panel: a size, the resize calls and the panel's `onResize`. */
const fake = vi.hoisted(() => ({
  size: 0,
  resizes: [] as number[],
  onResize: undefined as ((size: { inPixels: number }) => void) | undefined,
  /** What the library does on a group resize: keep the old share, report the new pixel size (not the user's). */
  drift(px: number) {
    this.size = px;
    this.onResize?.({ inPixels: px });
  },
}));

vi.mock("react-resizable-panels", async () => {
  const React = await import("react");
  type PanelProps = {
    id?: string;
    children?: ReactNode;
    className?: string;
    defaultSize?: number;
    panelRef?: { current: unknown } | null;
    onResize?: (size: { inPixels: number }) => void;
  };
  function Panel({ id, children, className, defaultSize, panelRef, onResize }: PanelProps) {
    if (id === "sidebar") {
      fake.onResize = onResize;
      if (fake.size === 0 && defaultSize !== undefined) fake.size = defaultSize;
      if (panelRef)
        panelRef.current = {
          getSize: () => ({ inPixels: fake.size, asPercentage: 0 }),
          resize: (px: number) => {
            fake.resizes.push(px);
            fake.size = px;
            onResize?.({ inPixels: px });
          },
        };
    }
    return React.createElement("div", { className, "data-panel-stub": id ?? "panel" }, children);
  }
  const box = ({ children, className }: { children?: ReactNode; className?: string }) => React.createElement("div", { className }, children);
  return { Group: box, Panel, Separator: box };
});

const originalWidth = window.innerWidth;

function resizeWindow(width: number): void {
  Object.defineProperty(window, "innerWidth", { configurable: true, writable: true, value: width });
  act(() => {
    window.dispatchEvent(new Event("resize"));
  });
}

function storeUserWidth(px: number): void {
  window.localStorage.setItem("shellfin.layout.v1", JSON.stringify({ sidebarWidth: px, sidebarOpen: true }));
}

beforeEach(() => {
  fake.size = 0;
  fake.resizes = [];
  fake.onResize = undefined;
});

afterEach(async () => {
  await settleApp();
  flushLayoutPrefs();
  window.localStorage.removeItem("shellfin.layout.v1");
  resizeWindow(originalWidth);
  setBackend(null);
});

describe("sidebar width across window resizes", () => {
  it("the correction is the clamped user width, or nothing when the panel has it", () => {
    expect(sidebarCorrection(208, 300, 1280)).toBe(300);
    expect(sidebarCorrection(484, 300, 1350)).toBe(300);
    expect(sidebarCorrection(300, 300, 1280)).toBeNull();
    expect(sidebarCorrection(300.4, 300, 1280)).toBeNull();
    expect(sidebarCorrection(300, 300, 580)).toBe(clampSidebarWidth(300, 580));
    expect(sidebarCorrection(Number.NaN, 300, 1280)).toBe(300);
  });

  it("narrow, then widen in one step: the user's width comes back and is never overwritten", async () => {
    storeUserWidth(300);
    resizeWindow(1280);
    setupApp();
    await settleApp();
    expect(fake.size).toBe(300);

    resizeWindow(580);
    fake.drift(300 * (580 / 1280)); // the library keeps the share first
    await waitFor(() => expect(fake.size).toBe(clampSidebarWidth(300, 580)));
    expect(loadLayoutPrefs().sidebarWidth).toBe(300);

    // One step back to 1280 (maximize / Snap): the library keeps 208 px or a share of the old window.
    resizeWindow(1280);
    fake.drift(208 * (1280 / 580));
    await waitFor(() => expect(fake.size).toBe(300));
    expect(loadLayoutPrefs().sidebarWidth).toBe(300);
  });

  it("reload while narrow, then widen in one step: the stored width, not the share of the narrow window", async () => {
    storeUserWidth(300);
    resizeWindow(580);
    setupApp();
    await settleApp();
    expect(fake.size).toBe(clampSidebarWidth(300, 580));

    resizeWindow(1350);
    fake.drift(484);
    await waitFor(() => expect(fake.size).toBe(300));
    expect(fake.resizes.at(-1)).toBe(300);
    expect(loadLayoutPrefs().sidebarWidth).toBe(300);
  });

  it("without any drift report, a window resize alone still corrects the size", async () => {
    storeUserWidth(300);
    resizeWindow(580);
    setupApp();
    await settleApp();
    fake.size = 208; // the library kept the pixel width and reported nothing
    resizeWindow(1280);
    await waitFor(() => expect(fake.size).toBe(300));
  });
});
