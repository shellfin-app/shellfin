import { afterEach, describe, expect, it } from "vitest";
import { FILES_SORT_DEFAULT, loadLayoutPrefs, parseFilesSort, resetLayoutPrefs, saveLayoutPrefs } from "./layoutPrefs";

const KEY = "shellfin.layout.v1";

afterEach(() => resetLayoutPrefs());

describe("layout prefs: file tree sort", () => {
  it("defaults to name ascending", () => {
    expect(loadLayoutPrefs().filesSort).toEqual(FILES_SORT_DEFAULT);
    expect(FILES_SORT_DEFAULT).toEqual({ key: "name", dir: "asc" });
  });

  it("round-trips a valid sort", () => {
    saveLayoutPrefs({ filesSort: { key: "mtime", dir: "desc" } });
    expect(JSON.parse(window.localStorage.getItem(KEY) ?? "{}").filesSort).toEqual({ key: "mtime", dir: "desc" });
    expect(loadLayoutPrefs().filesSort).toEqual({ key: "mtime", dir: "desc" });
  });

  it("garbage values fall back to the default without touching the other prefs", () => {
    for (const bad of [null, "size", 3, [], { key: "owner", dir: "asc" }, { key: "size", dir: "up" }, { key: "size" }, { dir: "desc" }, { key: { k: 1 }, dir: "asc" }]) {
      expect(parseFilesSort(bad)).toEqual(FILES_SORT_DEFAULT);
      window.localStorage.setItem(KEY, JSON.stringify({ sidebarWidth: 333, filesSort: bad }));
      const prefs = loadLayoutPrefs();
      expect(prefs.filesSort).toEqual(FILES_SORT_DEFAULT);
      expect(prefs.sidebarWidth).toBe(333);
    }
    window.localStorage.setItem(KEY, "{not json");
    expect(loadLayoutPrefs().filesSort).toEqual(FILES_SORT_DEFAULT);
  });

  it("ignores extra fields of a stored sort", () => {
    expect(parseFilesSort({ key: "size", dir: "asc", extra: "<script>" })).toEqual({ key: "size", dir: "asc" });
  });
});
