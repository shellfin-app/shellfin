import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import { makeSettings } from "../../ipc/mock/fixtures";
import { MockBackend } from "../../ipc/mock/mockBackend";
import type { Settings } from "../../ipc/types";
import { resetStore, useStore } from "../../state/store";
import { zoomFonts } from "./zoom";

describe("font zoom", () => {
  let backend: MockBackend;
  beforeEach(() => {
    resetStore();
    backend = new MockBackend();
    setBackend(backend);
    useStore.getState().setSettings(makeSettings());
  });
  afterEach(() => setBackend(null));

  it("rapid presses are serialised and the font never jumps back while a newer press is pending", async () => {
    const start = makeSettings().terminal.fontSize;
    const sent: number[] = [];
    vi.spyOn(backend, "settingsUpdate").mockImplementation(async (settings: Settings) => {
      sent.push(settings.terminal.fontSize);
      await new Promise((r) => setTimeout(r, 10));
      return settings;
    });
    const seen: number[] = [];
    const unsubscribe = useStore.subscribe((s) => {
      const size = s.settings?.terminal.fontSize;
      if (size !== undefined && seen.at(-1) !== size) seen.push(size);
    });
    const presses = [1, 1, 1, 1, 1].map((d) => zoomFonts(d, "both"));
    await Promise.all(presses);
    unsubscribe();
    expect(useStore.getState().settings?.terminal.fontSize).toBe(start + 5);
    expect(seen).toEqual([start + 1, start + 2, start + 3, start + 4, start + 5]); // strictly increasing: no flicker
    expect(sent.at(-1)).toBe(start + 5);
    expect(sent.length).toBeLessThan(5); // coalesced, one update in flight at a time
  });

  it("a failed update rolls back only to the last confirmed size and keeps later presses", async () => {
    const start = makeSettings().terminal.fontSize;
    let call = 0;
    vi.spyOn(backend, "settingsUpdate").mockImplementation(async (settings: Settings) => {
      call += 1;
      await new Promise((r) => setTimeout(r, 5));
      if (call === 1) throw { code: "storageReadOnly" }; // an expected failure: a notice (`internal` would open the error dialog)
      return settings;
    });
    await Promise.all([zoomFonts(1, "both"), zoomFonts(1, "both")]);
    expect(useStore.getState().settings?.terminal.fontSize).toBe(start + 2);
    expect(useStore.getState().notices.length).toBeGreaterThan(0);

    call = 0;
    await zoomFonts(1, "both"); // the next press fails alone: back to the confirmed size
    expect(useStore.getState().settings?.terminal.fontSize).toBe(start + 2);
  });
});
