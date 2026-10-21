import { beforeEach, describe, expect, it, vi } from "vitest";
import { MockBackend } from "../../ipc/mock/mockBackend";
import { useStore } from "../../state/store";
import { setLanguage } from "../../i18n";

const backend = new MockBackend();
vi.mock("../../ipc", () => ({ getBackend: () => backend }));

import { nativeMenuLabels, syncNativeMenuLabels } from "./nativeMenuLabels";
import { NATIVE_MENU } from "./shellEvents";

describe("native menu labels (translated labels)", () => {
  beforeEach(() => {
    backend.menuLabels = {};
  });

  it("covers every native item id and the top-level titles, within the backend limits", () => {
    const labels = nativeMenuLabels();
    for (const id of Object.keys(NATIVE_MENU)) expect(labels[id], id).toBeTruthy();
    for (const name of ["file", "edit", "view", "connection", "terminal", "window", "help"]) {
      expect(labels[`menu.${name}`], name).toBeTruthy();
    }
    for (const label of Object.values(labels)) {
      expect(Array.from(label).length).toBeLessThanOrEqual(80);
      expect(label.trim()).not.toBe("");
    }
    expect(labels["file.save"]).toBe("Save");
    expect(labels["file.importBookmarks"]).toBe("Import bookmarks…");
    expect(labels["file.exportBookmarks"]).toBe("Export bookmarks…");
  });

  it("maps the bookmark import/export items to their actions", () => {
    expect(NATIVE_MENU["file.importBookmarks"]).toBe("importBookmarks");
    expect(NATIVE_MENU["file.exportBookmarks"]).toBe("exportBookmarks");
  });

  it("translates the bookmark items in Ukrainian", async () => {
    await setLanguage("uk");
    try {
      const labels = nativeMenuLabels();
      expect(labels["file.importBookmarks"]).toBe("Імпортувати закладки…");
      expect(labels["file.exportBookmarks"]).toBe("Експортувати закладки…");
    } finally {
      await setLanguage("en");
    }
  });

  it("sends nothing except on macOS", async () => {
    useStore.setState({ platform: "windows" });
    await syncNativeMenuLabels();
    expect(backend.menuLabels).toEqual({});
    useStore.setState({ platform: "macos" });
    await syncNativeMenuLabels();
    expect(backend.menuLabels["menu.file"]).toBe("File");
    useStore.setState({ platform: "windows" });
  });

  it("sends the labels of the active language", async () => {
    useStore.setState({ platform: "macos" });
    await setLanguage("uk");
    try {
      await syncNativeMenuLabels();
      expect(backend.menuLabels["menu.file"]).not.toBe("File");
    } finally {
      await setLanguage("en");
      useStore.setState({ platform: "windows" });
    }
  });
});
