import { describe, expect, it } from "vitest";
import enTranslation from "../i18n/locales/en/translation.json";
import { nameProblem } from "../features/files/fileActions";
import { CM_PHRASE_KEYS, cmPhrases } from "./cmPhrases";
import { failureKind } from "./fileFailure";
import { isBrowserAccelerator } from "./accelerators";
import { connectionTabIndex, matchShortcut, terminalClipboardKey } from "./keybindings";
import { countLines, diffLines } from "./lineDiff";

describe("diffLines (conflict compare-lite)", () => {
  it("counts and lists the changed lines with their line numbers", () => {
    const mine = "a\nb\nc\nd\n";
    const theirs = "a\nB\nc\nd\ne\n";
    const d = diffLines(mine, theirs);
    expect(d).toMatchObject({ added: 2, removed: 1, exact: true, truncated: false });
    expect(d.lines).toEqual([
      { op: "del", line: 2, text: "b" },
      { op: "add", line: 2, text: "B" },
      { op: "add", line: 5, text: "e" },
    ]);
  });

  it("identical texts have no differences; a final newline is not a line", () => {
    expect(diffLines("x\ny\n", "x\ny")).toMatchObject({ added: 0, removed: 0 });
    expect(countLines("x\ny\n")).toBe(2);
    expect(countLines("")).toBe(0);
  });

  it("limits the preview and says so", () => {
    const mine = Array.from({ length: 100 }, (_, i) => `mine ${i}`).join("\n");
    const d = diffLines(mine, "", 10);
    expect(d.removed).toBe(100);
    expect(d.lines).toHaveLength(10);
    expect(d.truncated).toBe(true);
  });

  it("falls back to approximate counts for very long differing texts", () => {
    const a = Array.from({ length: 2500 }, (_, i) => `a${i}`).join("\n");
    const b = Array.from({ length: 2500 }, (_, i) => `b${i}`).join("\n");
    const d = diffLines(a, b);
    expect(d).toMatchObject({ exact: false, added: 2500, removed: 2500, lines: [] });
  });
});

describe("editor shortcuts (§8.7)", () => {
  const key = (k: string, mods: Partial<{ ctrlKey: boolean; metaKey: boolean; shiftKey: boolean; altKey: boolean; code: string; getModifierState: (k: string) => boolean }> = {}) => ({
    key: k,
    ctrlKey: false,
    metaKey: false,
    shiftKey: false,
    altKey: false,
    ...mods,
  });

  it("Mod+S / Mod+Alt+S / Mod+W / Ctrl+Tab on Windows", () => {
    expect(matchShortcut(key("s", { ctrlKey: true }), "windows", false)).toBe("save");
    expect(matchShortcut(key("s", { ctrlKey: true, altKey: true }), "windows", false)).toBe("saveAll");
    expect(matchShortcut(key("w", { ctrlKey: true }), "windows", false)).toBe("closeTab");
    expect(matchShortcut(key("Tab", { ctrlKey: true }), "windows", false)).toBe("nextTab");
    expect(matchShortcut(key("Tab", { ctrlKey: true, shiftKey: true }), "windows", false)).toBe("prevTab");
  });

  it("Cmd on macOS; Option+S is recognised by the physical key", () => {
    expect(matchShortcut(key("s", { metaKey: true }), "macos", false)).toBe("save");
    expect(matchShortcut(key("ß", { metaKey: true, altKey: true, code: "KeyS" }), "macos", false)).toBe("saveAll");
  });

  it("AltGr+S (Ctrl+Alt reporting a different character) is typing, not Save all", () => {
    expect(matchShortcut(key("ś", { ctrlKey: true, altKey: true }), "windows", false)).toBeNull();
  });

  it("works with a non-Latin layout: the physical key decides, a Latin key wins", () => {
    expect(matchShortcut(key("і", { ctrlKey: true, code: "KeyS" }), "windows", false)).toBe("save");
    expect(matchShortcut(key("ц", { ctrlKey: true, code: "KeyW" }), "windows", false)).toBe("closeTab");
    expect(matchShortcut(key("і", { ctrlKey: true, altKey: true, code: "KeyS" }), "windows", false)).toBe("saveAll");
    expect(matchShortcut(key("л", { ctrlKey: true, code: "KeyK" }), "windows", false)).toBe("palette");
    expect(matchShortcut(key("Л", { ctrlKey: true, shiftKey: true, code: "KeyK" }), "windows", true)).toBe("paletteInTerminal");
    expect(matchShortcut(key("А", { ctrlKey: true, shiftKey: true, code: "KeyF" }), "windows", true)).toBe("terminalFind");
    expect(matchShortcut(key("ґ", { ctrlKey: true, code: "Backquote" }), "windows", true)).toBe("toggleTerminalPanel");
    expect(connectionTabIndex(key("1", { ctrlKey: true, code: "Digit1" }), "windows")).toBe(0);
    expect(connectionTabIndex(key("&", { ctrlKey: true, code: "Digit1" }), "windows")).toBe(0);
    expect(terminalClipboardKey(key("С", { ctrlKey: true, shiftKey: true, code: "KeyC" }), "windows")).toBe("copy");
    expect(isBrowserAccelerator(key("к", { ctrlKey: true, code: "KeyR" }), "windows")).toBe(true);
    // Latin letter in `key` (Dvorak/AZERTY) is used as is, whatever the physical key is.
    expect(matchShortcut(key("z", { ctrlKey: true, code: "KeyW" }), "windows", false)).toBeNull();
    expect(matchShortcut(key("w", { ctrlKey: true, code: "KeyZ" }), "windows", false)).toBe("closeTab");
    expect(matchShortcut(key("s", { ctrlKey: true, code: "KeyS" }), "windows", false)).toBe("save");
  });

  it("terminal semantics stay: Ctrl+K / Ctrl+S in a Cyrillic layout still reach the shell", () => {
    expect(matchShortcut(key("л", { ctrlKey: true, code: "KeyK" }), "windows", true)).toBeNull();
    expect(matchShortcut(key("і", { ctrlKey: true, code: "KeyS" }), "windows", true)).toBeNull();
  });

  it("AltGr+S with the AltGraph state is typing even when the physical key is S", () => {
    const altGr = (name: string) => name === "AltGraph";
    expect(matchShortcut(key("ś", { ctrlKey: true, altKey: true, code: "KeyS", getModifierState: altGr }), "windows", false)).toBeNull();
  });

  it("the terminal keeps Ctrl+S / Ctrl+W / Ctrl+Tab for the shell", () => {
    expect(matchShortcut(key("s", { ctrlKey: true }), "windows", true)).toBeNull();
    expect(matchShortcut(key("w", { ctrlKey: true }), "windows", true)).toBeNull();
    expect(matchShortcut(key("Tab", { ctrlKey: true }), "windows", true)).toBeNull();
  });
});

describe("misc editor helpers", () => {
  it("nameProblem: one path component only", () => {
    expect(nameProblem("ok.txt")).toBeNull();
    expect(nameProblem("  ")).toBe("empty");
    expect(nameProblem("a/b")).toBe("slash");
    expect(nameProblem("..")).toBe("dots");
    expect(nameProblem(`a${String.fromCharCode(0)}b`)).toBe("nul");
  });

  it("failureKind maps open failures to the page the editor shows", () => {
    expect(failureKind({ code: "fileIsBinary", params: { path: "/x" } })).toBe("binary");
    expect(failureKind({ code: "fileTooLarge", params: { path: "/x", size: 2, limit: 1 } })).toBe("tooLarge");
    expect(failureKind({ code: "decodeFailed", params: { path: "/x", encoding: "utf-8" } })).toBe("decode");
    expect(failureKind({ code: "unsupportedEncoding", params: { encoding: "x" } })).toBe("decode");
    expect(failureKind({ code: "permissionDenied", params: { path: "/x" } })).toBe("other");
    expect(failureKind(null)).toBe("other");
  });

  it("every CodeMirror phrase has an English translation", () => {
    const find = (path: string): unknown => path.split(".").reduce<unknown>((o, k) => (o as Record<string, unknown> | undefined)?.[k], enTranslation);
    for (const key of CM_PHRASE_KEYS) expect(typeof find(key)).toBe("string");
    expect(cmPhrases()["Go to line"]).toBe("Go to line");
  });
});
