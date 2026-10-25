// The untranslated-keys report and the locale pruning (scripts/i18n-missing.mjs).
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { expectedKeys, extraKeys, flatten, missingKeys, pruneLocale, reportLines, run } from "./i18n-missing.mjs";
import en from "../src/i18n/locales/en/translation.json";

const SCRIPT = join(dirname(fileURLToPath(import.meta.url)), "i18n-missing.mjs");

const EN = {
  a: { title: "Title", body: "Body" },
  files_one: "{{count}} file",
  files_other: "{{count}} files",
};
const enFlat = flatten(EN);

describe("expectedKeys", () => {
  it("expands plural keys to the categories of the language", () => {
    expect(expectedKeys(enFlat, "en").get("files")).toEqual(["files_one", "files_other"]);
    expect([...expectedKeys(enFlat, "uk").get("files")].sort()).toEqual(["files_few", "files_many", "files_one", "files_other"]);
    expect(expectedKeys(enFlat, "ja").get("files")).toEqual(["files_other"]);
    expect(expectedKeys(enFlat, "de").get("a.title")).toEqual(["a.title"]);
  });
});

describe("missingKeys", () => {
  it("is empty for a complete translation", () => {
    const de = { a: { title: "Titel", body: "Text" }, files_one: "{{count}} Datei", files_other: "{{count}} Dateien" };
    expect(missingKeys(enFlat, flatten(de), "de")).toEqual([]);
  });

  it("lists absent and empty keys", () => {
    const de = { a: { title: "Titel", body: " " } };
    expect(missingKeys(enFlat, flatten(de), "de")).toEqual(["a.body", "files"]);
  });

  it("counts a plural key as missing when any form of the language is missing", () => {
    const uk = { a: { title: "Т", body: "Т" }, files_one: "{{count}} файл", files_other: "{{count}} файлу" };
    expect(missingKeys(enFlat, flatten(uk), "uk")).toEqual(["files (few, many)"]);
    // Japanese needs only `_other`: an English-shaped `_one` does not make it complete.
    expect(missingKeys(enFlat, flatten({ a: EN.a, files_one: "x" }), "ja")).toEqual(["files"]);
  });

  it("English is complete against itself", () => {
    expect(missingKeys(flatten(en), flatten(en), "en")).toEqual([]);
  });
});

describe("extraKeys", () => {
  it("finds keys English lacks, plural forms the language does not use and plural mismatches", () => {
    const de = { a: { title: "T", gone: "x" }, files_few: "x", files_other: "x", "a.title_one": "x" };
    expect(extraKeys(enFlat, flatten(de), "de")).toEqual(["a.gone", "a.title_one", "files_few"]);
    expect(extraKeys(enFlat, flatten({ files_one: "x" }), "ja")).toEqual(["files_one"]);
  });
});

describe("pruneLocale", () => {
  it("removes extra keys and empty strings, keeps translations and drops objects left empty", () => {
    const de = { a: { title: "Titel", body: "" }, old: { x: "weg" }, files_one: "eine", files_few: "x" };
    const { json, removed } = pruneLocale(EN, de, "de");
    expect(json).toEqual({ a: { title: "Titel" }, files_one: "eine" });
    expect(removed.sort()).toEqual(["a.body", "files_few", "old.x"]);
  });

  it("never adds a key", () => {
    expect(pruneLocale(EN, {}, "uk")).toEqual({ json: {}, removed: [] });
  });
});

describe("report", () => {
  it("prints the count and the keys per language", () => {
    expect(reportLines("uk", [])).toEqual(["uk: complete"]);
    expect(reportLines("de", ["a.body", "files"])).toEqual(["de: 2 missing", "  a.body", "  files"]);
  });

  it("the CLI exits 0 on the repository's locales and prints one line per locale", () => {
    const out = execFileSync(process.execPath, [SCRIPT], { encoding: "utf8" });
    expect(out).toMatch(/^uk: /m);
    expect(out).toMatch(/^de: /m);
  });
});

describe("run", () => {
  function fixture(locales) {
    const dir = mkdtempSync(join(tmpdir(), "i18n-missing-"));
    for (const [lng, json] of Object.entries(locales)) {
      mkdirSync(join(dir, lng));
      writeFileSync(join(dir, lng, "translation.json"), JSON.stringify(json));
    }
    return dir;
  }
  const readLocale = (dir, lng) => JSON.parse(readFileSync(join(dir, lng, "translation.json"), "utf8"));

  it("exits 0 with missing keys, 1 with --strict, 0 with --strict when complete", () => {
    const dir = fixture({ en: EN, de: { a: { title: "Titel" } } });
    try {
      const lines = [];
      expect(run([], dir, (l) => lines.push(l))).toBe(0);
      expect(lines.slice(0, 3)).toEqual(["de: 2 missing", "  a.body", "  files"]);
      expect(run(["--strict"], dir, () => {})).toBe(1);
      const annotated = [];
      expect(run(["--github"], dir, (l) => annotated.push(l))).toBe(0);
      expect(annotated).toContain("::warning title=Untranslated keys::de: 2 keys fall back to English (npm run i18n:missing)");
      writeFileSync(join(dir, "de", "translation.json"), JSON.stringify({ ...EN, a: { title: "T", body: "B" } }));
      expect(run(["--strict"], dir, () => {})).toBe(0);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("--prune rewrites only locales with something to remove and never touches English", () => {
    const uk = { a: { title: "Заголовок", stale: "x" }, files_one: "" };
    const ja = { a: { title: "タイトル" } };
    const dir = fixture({ en: { ...EN, a: { ...EN.a, x: "" } }, uk, ja });
    try {
      const lines = [];
      expect(run(["--prune"], dir, (l) => lines.push(l))).toBe(0);
      expect(readLocale(dir, "uk")).toEqual({ a: { title: "Заголовок" } });
      expect(readFileSync(join(dir, "uk", "translation.json"), "utf8").endsWith("}\n")).toBe(true);
      expect(readFileSync(join(dir, "ja", "translation.json"), "utf8")).toBe(JSON.stringify(ja)); // untouched
      expect(readLocale(dir, "en").a.x).toBe("");
      expect(lines).toContain("uk: removed a.stale, files_one");
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
