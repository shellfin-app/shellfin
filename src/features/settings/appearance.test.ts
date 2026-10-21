import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { makeBootstrap } from "../../ipc/mock/fixtures";
import type { Settings } from "../../ipc/types";
import { resetStore, useStore } from "../../state/store";
import {
  applyCachedAppearance,
  endAppearancePreview,
  installAppearance,
  previewAppearance,
  shownAppearance,
  subscribeAppearance,
} from "./appearance";

/** matchMedia whose answers the test can change; listeners are notified like the browser does. */
const queries = { dark: true, reduced: false };
const listeners = new Map<string, Set<() => void>>();
function fakeMatchMedia(query: string): MediaQueryList {
  const key = query.includes("color-scheme") ? "dark" : "reduced";
  const set = listeners.get(key) ?? new Set();
  listeners.set(key, set);
  return {
    get matches() {
      return queries[key];
    },
    media: query,
    addEventListener: (_type: string, cb: () => void) => set.add(cb),
    removeEventListener: (_type: string, cb: () => void) => set.delete(cb),
  } as unknown as MediaQueryList;
}
function setSystem(key: "dark" | "reduced", value: boolean) {
  queries[key] = value;
  for (const cb of listeners.get(key) ?? []) cb();
}

function settingsWith(patch: (s: Settings) => void): Settings {
  const s = structuredClone(makeBootstrap(Date.now()).settings);
  patch(s);
  return s;
}

const html = document.documentElement;

describe("appearance on <html>", () => {
  beforeAll(() => {
    vi.stubGlobal("matchMedia", fakeMatchMedia);
    window.matchMedia = fakeMatchMedia;
    resetStore();
    installAppearance();
  });
  afterEach(() => {
    localStorage.clear();
    setSystem("dark", true);
    setSystem("reduced", false);
  });

  it("applies the saved theme, accent, size and motion when the settings arrive", () => {
    useStore.getState().setSettings(
      settingsWith((s) => {
        s.appearance = { theme: "light", accent: "orange", uiFontSize: 15, reduceMotion: "on" };
      }),
    );
    expect(html.dataset.theme).toBe("light");
    expect(html.dataset.accent).toBe("orange");
    expect(html.dataset.motion).toBe("reduce");
    expect(Number(html.style.getPropertyValue("--ui-scale"))).toBeCloseTo(15 / 13);
  });

  it("system follows the OS live, in both directions", () => {
    useStore.getState().setSettings(
      settingsWith((s) => {
        s.appearance = { theme: "system", accent: "teal", uiFontSize: 13, reduceMotion: "system" };
      }),
    );
    expect(html.dataset.theme).toBe("dark");
    setSystem("dark", false);
    expect(html.dataset.theme).toBe("light");
    setSystem("dark", true);
    expect(html.dataset.theme).toBe("dark");
    setSystem("reduced", true);
    expect(html.dataset.motion).toBe("reduce");
  });

  it("an explicit theme ignores OS changes", () => {
    useStore.getState().setSettings(
      settingsWith((s) => {
        s.appearance = { theme: "dark", accent: "teal", uiFontSize: 13, reduceMotion: "off" };
      }),
    );
    setSystem("dark", false);
    expect(html.dataset.theme).toBe("dark");
    setSystem("reduced", true);
    expect(html.dataset.motion).toBe("full");
  });

  it("terminal.background sets --term-bg and the tone, and notifies subscribers", () => {
    const seen: string[] = [];
    const off = subscribeAppearance((applied) => seen.push(applied.theme));
    useStore.getState().setSettings(
      settingsWith((s) => {
        s.terminal.background = "#FBF9F4";
      }),
    );
    expect(html.style.getPropertyValue("--term-bg")).toBe("#FBF9F4");
    expect(html.dataset.termTone).toBe("light");
    expect(seen.length).toBeGreaterThan(0);
    off();
  });

  it("a preview applies unsaved values and the saved settings come back", () => {
    useStore.getState().setSettings(settingsWith(() => undefined));
    previewAppearance({ theme: "light", accent: "green", uiFontSize: 16, reduceMotion: "off" }, "#FFFFFF");
    expect(html.dataset.theme).toBe("light");
    expect(html.dataset.accent).toBe("green");
    expect(shownAppearance()?.terminalBackground).toBe("#FFFFFF"); // the terminal theme is built from this
    // A settings change while the dialog is open (e.g. from another window event) keeps the preview on screen.
    useStore.getState().setSettings(settingsWith(() => undefined));
    expect(html.dataset.theme).toBe("light");
    expect(html.dataset.accent).toBe("green");
    endAppearancePreview(); // the dialog closes: the saved values come back
    expect(html.dataset.theme).toBe("dark");
    expect(html.dataset.accent).toBe("teal");
    expect(shownAppearance()?.terminalBackground).toBeNull();
  });

  it("the cached appearance is applied before the settings are known", () => {
    localStorage.setItem(
      "shellfin.appearance",
      JSON.stringify({ appearance: { theme: "light", accent: "blue", uiFontSize: 14, reduceMotion: "system" }, terminalBackground: null }),
    );
    applyCachedAppearance();
    expect(html.dataset.theme).toBe("light");
    expect(html.dataset.accent).toBe("blue");
  });
});
