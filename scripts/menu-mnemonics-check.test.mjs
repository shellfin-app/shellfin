import { describe, expect, it } from "vitest";
import { MENU_IDS } from "../src/lib/actionIds.ts";
import { mnemonicCollisions, mnemonicOf } from "../src/lib/menuMnemonic.ts";
import { checkLocale, checkLocaleParts } from "./menu-mnemonics-check.mjs";
import en from "../src/i18n/locales/en/translation.json";
import uk from "../src/i18n/locales/uk/translation.json";

const ids = ["a", "b", "c"];

describe("menu mnemonics", () => {
  it("uses the lower-cased first character", () => {
    expect(mnemonicOf("File")).toBe("f");
    expect(mnemonicOf("Файл")).toBe("ф");
    expect(mnemonicOf("")).toBeUndefined();
  });

  it("detects a collision and names the menus and the letter", () => {
    const problems = checkLocale("xx", { a: "Edit", b: "Help", c: "Extra" }, ids);
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("xx");
    expect(problems[0]).toContain('"e"');
    expect(problems[0]).toContain("menubar.a");
    expect(problems[0]).toContain("menubar.c");
    expect(problems[0]).not.toContain("menubar.b");
  });

  it("is case-insensitive", () => {
    expect(mnemonicCollisions({ a: "Файл", b: "файли" })).toEqual([{ letter: "ф", ids: ["a", "b"] }]);
  });

  it("accepts distinct letters", () => {
    expect(checkLocale("xx", { a: "File", b: "Edit", c: "View" }, ids)).toEqual([]);
  });

  it("reports a missing title", () => {
    expect(checkLocale("xx", { a: "File", b: "Edit" }, ids)).toEqual(["xx: menubar.c is missing"]);
  });

  it("reports an empty title as unreachable (and not as a collision)", () => {
    expect(mnemonicCollisions({ a: "", b: "" })).toEqual([]);
    const problems = checkLocale("xx", { a: "File", b: "", c: "" }, ids);
    expect(problems).toHaveLength(2);
    expect(problems[0]).toContain("menubar.b");
    expect(problems[0]).toContain("cannot be opened with Alt+letter");
    expect(problems[1]).toContain("menubar.c");
  });

  it("reports a title whose mnemonic is not a single UTF-16 unit", () => {
    // "İ".toLowerCase() is "i" + combining dot (2 units): e.key never has that length.
    expect(mnemonicOf("İstanbul")).toHaveLength(2);
    const dotted = checkLocale("xx", { a: "File", b: "İstanbul", c: "View" }, ids);
    expect(dotted).toHaveLength(1);
    expect(dotted[0]).toContain("menubar.b");
    expect(dotted[0]).toContain("Alt+letter");
    // An astral first character is two UTF-16 units.
    const astral = checkLocale("xx", { a: "File", b: "\u{1D4D0}bc", c: "View" }, ids);
    expect(astral).toHaveLength(1);
    expect(astral[0]).toContain("menubar.b");
  });

  it("splits title problems from collisions", () => {
    const parts = checkLocaleParts("xx", { a: "Edit", b: "", c: "Extra" }, ids);
    expect(parts.titles).toHaveLength(1);
    expect(parts.collisions).toHaveLength(1);
  });

  it("holds for the shipped locales", () => {
    expect(checkLocale("en", en.menubar)).toEqual([]);
    expect(checkLocale("uk", uk.menubar)).toEqual([]);
    expect(MENU_IDS.length).toBeGreaterThan(1);
  });
});
