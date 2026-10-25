import { describe, expect, it } from "vitest";
import { MENU_IDS } from "../src/lib/actionIds.ts";
import {
  foldMnemonic,
  mnemonicChar,
  mnemonicCollisions,
  mnemonicOf,
  nativeMenuTitle,
  parseMenuTitle,
  plainMenuTitle,
} from "../src/lib/menuMnemonic.ts";
import { CJK_MENUBAR, LONG_MENUBAR } from "../src/test/fixtureLocales.ts";
import { checkLocale, checkLocaleParts } from "./menu-mnemonics-check.mjs";
import en from "../src/i18n/locales/en/translation.json";
import uk from "../src/i18n/locales/uk/translation.json";

const ids = ["a", "b", "c"];
const english = { a: "File", b: "Edit", c: "View" };
const check = (locale, menubar) => checkLocale(locale, menubar, ids, english);

describe("menu title markers (parseMenuTitle)", () => {
  it("uses the first character without a marker", () => {
    expect(parseMenuTitle("File")).toEqual({ before: "", mnemonic: "F", after: "ile", plain: "File", marked: false, problems: [] });
    expect(mnemonicOf("Файл")).toBe("ф");
    expect(mnemonicOf("")).toBeUndefined();
    expect(mnemonicChar("")).toBe("");
  });

  it("uses the &-marked character and removes the marker from the text", () => {
    expect(parseMenuTitle("Ay&uda")).toMatchObject({ before: "Ay", mnemonic: "u", after: "da", plain: "Ayuda", marked: true });
    expect(parseMenuTitle("ファイル(&F)")).toMatchObject({ before: "ファイル(", mnemonic: "F", after: ")", plain: "ファイル(F)" });
    expect(mnemonicOf("E&xibir")).toBe("x");
  });

  it("&& is a literal ampersand and never a marker", () => {
    expect(parseMenuTitle("Tools && &Help")).toMatchObject({ plain: "Tools & Help", mnemonic: "H", marked: true, problems: [] });
    expect(parseMenuTitle("A&&B")).toMatchObject({ plain: "A&B", mnemonic: "A", marked: false, problems: [] });
  });

  it("reports hostile markers: two markers, a trailing &, & before a space", () => {
    expect(parseMenuTitle("&File &Edit")).toMatchObject({ mnemonic: "F", plain: "File Edit", problems: ["multiple"] });
    expect(parseMenuTitle("File&")).toMatchObject({ plain: "File", problems: ["trailing"] });
    expect(parseMenuTitle("File& x")).toMatchObject({ plain: "File x", problems: ["beforeSpace"] });
    expect(parseMenuTitle("&")).toMatchObject({ plain: "", mnemonic: "", problems: ["trailing"] });
  });

  it("an astral marked character stays whole", () => {
    expect(parseMenuTitle("a&\u{1D4D0}b")).toMatchObject({ before: "a", mnemonic: "\u{1D4D0}", after: "b" });
  });

  it("strips the markers for plain labels and the (X) group for the macOS menu", () => {
    expect(plainMenuTitle("Ay&uda")).toBe("Ayuda");
    expect(plainMenuTitle("ファイル(&F)")).toBe("ファイル(F)");
    expect(nativeMenuTitle("ファイル(&F)")).toBe("ファイル");
    expect(nativeMenuTitle("파일 (&F)")).toBe("파일");
    expect(nativeMenuTitle("Ay&uda")).toBe("Ayuda");
    expect(nativeMenuTitle("File")).toBe("File");
    // Only the group around the marked letter goes; a title that is nothing but the group keeps it.
    expect(nativeMenuTitle("(&F)")).toBe("(F)");
    expect(nativeMenuTitle("Ansicht (Liste)")).toBe("Ansicht (Liste)");
  });

  it("folds accents and case for matching", () => {
    expect(foldMnemonic("É")).toBe("e");
    expect(mnemonicOf("&Édition")).toBe("e");
    expect(mnemonicOf("état")).toBe("e"); // a decomposed é: the combining mark is not part of the mnemonic
    expect(mnemonicOf("İstanbul")).toBe("i");
    expect(foldMnemonic("ı")).toBe("i"); // Turkish dotless i
    expect(foldMnemonic("I")).toBe("i");
  });

  it("does not fold letters of other scripts: Cyrillic й is not и, ї is not і", () => {
    expect(foldMnemonic("й")).toBe("й");
    expect(foldMnemonic("Ї")).toBe("ї");
    expect(foldMnemonic("й")).toBe("й"); // decomposed input is composed, not stripped
    expect(mnemonicCollisions({ a: "Йога", b: "Игра" })).toEqual([]);
    expect(mnemonicCollisions({ a: "Їжа", b: "Іграшки" })).toEqual([]);
  });

  it("keeps combining marks with their base letter (grapheme clusters)", () => {
    expect(parseMenuTitle("état")).toMatchObject({ mnemonic: "é", after: "tat" });
    expect(parseMenuTitle("x&éy")).toMatchObject({ before: "x", mnemonic: "é", after: "y" });
    expect(mnemonicOf("&éy")).toBe("e");
  });

  it("drops a full-width （&F） group for headings and the macOS menu", () => {
    expect(nativeMenuTitle("表示（&V）")).toBe("表示");
    expect(plainMenuTitle("表示（&V）")).toBe("表示（V）");
  });
});

describe("menu mnemonics check", () => {
  it("detects a collision and names the menus and the letter", () => {
    const problems = check("xx", { a: "Edit", b: "Help", c: "Extra" });
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("xx");
    expect(problems[0]).toContain('"e"');
    expect(problems[0]).toContain("menubar.a");
    expect(problems[0]).toContain("menubar.c");
    expect(problems[0]).not.toContain("menubar.b");
  });

  it("is case- and accent-insensitive", () => {
    expect(mnemonicCollisions({ a: "Файл", b: "файли" })).toEqual([{ letter: "ф", ids: ["a", "b"] }]);
    expect(mnemonicCollisions({ a: "Édition", b: "Extras" })).toEqual([{ letter: "e", ids: ["a", "b"] }]);
  });

  it("a marker resolves a first-letter collision (es, pt-BR, fr)", () => {
    expect(check("es", { a: "Archivo", b: "Ver", c: "Ayuda" })).toHaveLength(1);
    expect(check("es", { a: "Archivo", b: "Ver", c: "Ay&uda" })).toEqual([]);
  });

  it("accepts distinct letters", () => {
    expect(check("xx", { a: "File", b: "Edit", c: "View" })).toEqual([]);
  });

  it("reports a missing title", () => {
    expect(check("xx", { a: "File", b: "Edit" })).toEqual(["xx: menubar.c is missing"]);
  });

  it("reports an empty title as unreachable (and not as a collision)", () => {
    expect(mnemonicCollisions({ a: "", b: "" })).toEqual([]);
    const problems = check("xx", { a: "File", b: "", c: "" });
    expect(problems).toHaveLength(2);
    expect(problems[0]).toContain("menubar.b");
    expect(problems[0]).toContain("cannot be opened with Alt+letter");
    expect(problems[1]).toContain("menubar.c");
  });

  it("reports a mnemonic that is not a single-unit letter or digit", () => {
    // An astral first character is two UTF-16 units.
    const astral = check("xx", { a: "File", b: "\u{1D4D0}bc", c: "View" });
    expect(astral).toHaveLength(1);
    expect(astral[0]).toContain("menubar.b");
    // Punctuation is not typeable as a mnemonic either.
    expect(check("xx", { a: "File", b: "[Edit]", c: "View" })[0]).toContain("menubar.b");
    expect(check("xx", { a: "File", b: "Ed&-it", c: "View" })[0]).toContain("menubar.b");
    // Digits are fine.
    expect(check("xx", { a: "File", b: "&2 Edit", c: "View" })).toEqual([]);
  });

  it("reports bad markers", () => {
    expect(check("xx", { a: "&Fi&le", b: "Edit", c: "View" })[0]).toContain("more than one & marker");
    expect(check("xx", { a: "File&", b: "Edit", c: "View" })[0]).toContain("ends with &");
    expect(check("xx", { a: "File", b: "Edit& x", c: "View" })[0]).toContain("before a space");
  });

  it("CJK titles need an explicit ASCII marker equal to the English letter", () => {
    expect(check("ja", { a: "ファイル(&F)", b: "編集(&E)", c: "表示(&V)" })).toEqual([]);
    // The first character would be accepted by the old rule, but nobody can type it as Alt+letter.
    expect(check("xx", { a: "ファイル", b: "編集(&E)", c: "表示(&V)" })[0]).toContain('needs an explicit ASCII-letter mnemonic like "(&F)"');
    expect(check("xx", { a: "ファイル(&フ)", b: "編集(&E)", c: "表示(&V)" })[0]).toContain("needs an explicit ASCII-letter");
    expect(check("ko", { a: "파일(&D)", b: "편집(&E)", c: "보기(&V)" })[0]).toContain('must use the English mnemonic "F"');
    // A ja / ko / zh locale needs markers even on a Latin title ("SSH" stays Latin in some languages).
    expect(check("zh-CN", { a: "File", b: "编辑(&E)", c: "查看(&V)" })[0]).toContain("menubar.a");
    expect(check("zh-CN", { a: "&File", b: "编辑(&E)", c: "查看(&V)" })).toEqual([]);
  });

  it("splits title problems from collisions", () => {
    const parts = checkLocaleParts("xx", { a: "Edit", b: "", c: "Extra" }, ids, english);
    expect(parts.titles).toHaveLength(1);
    expect(parts.collisions).toHaveLength(1);
  });

  it("holds for the shipped locales and the test fixtures", () => {
    expect(checkLocale("en", en.menubar)).toEqual([]);
    expect(checkLocale("uk", uk.menubar)).toEqual([]);
    expect(checkLocale("ja", CJK_MENUBAR)).toEqual([]);
    expect(checkLocale("de", LONG_MENUBAR)).toEqual([]);
    expect(MENU_IDS.length).toBeGreaterThan(1);
  });
});
