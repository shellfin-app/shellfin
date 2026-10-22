import { describe, expect, it } from "vitest";
import { MENU_IDS } from "../src/lib/actionIds.ts";
import { mnemonicCollisions, mnemonicOf } from "../src/lib/menuMnemonic.ts";
import { checkLocale } from "./menu-mnemonics-check.mjs";
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

  it("lets other menus reuse a letter (only one bar is compared at a time)", () => {
    expect(checkLocale("xx", { a: "File", b: "Edit", c: "View" }, ids)).toEqual([]);
    expect(checkLocale("xx", { a: "Find", b: "Edit", c: "View" }, ids)).toEqual([]);
  });

  it("handles a missing or empty title like the UI: no mnemonic, no collision", () => {
    expect(mnemonicCollisions({ a: "", b: "" })).toEqual([]);
    const problems = checkLocale("xx", { a: "File", b: "Edit" }, ids);
    expect(problems).toEqual(["xx: menubar.c is missing"]);
  });

  it("holds for the shipped locales", () => {
    expect(checkLocale("en", en.menubar)).toEqual([]);
    expect(checkLocale("uk", uk.menubar)).toEqual([]);
    expect(MENU_IDS.length).toBeGreaterThan(1);
  });
});
