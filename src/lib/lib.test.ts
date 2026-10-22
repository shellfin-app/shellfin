import { describe, expect, it } from "vitest";
import { displayServerText } from "./serverText";
import { basename, dirname, joinPath, segments } from "./remotePath";
import { matchShortcut, terminalClipboardKey, type KeyEventLike } from "./keybindings";
import { formatEndpoint } from "./endpoint";
import { languageForPath } from "./languages";

const key = (k: string, mods: Partial<KeyEventLike> = {}): KeyEventLike => ({
  key: k, ctrlKey: false, metaKey: false, shiftKey: false, altKey: false, ...mods,
});

describe("displayServerText (§9.5)", () => {
  it("escapes C0/C1 and bidi controls", () => {
    expect(displayServerText("a\x1b[31mb", 100)).toBe("a\\x1b[31mb");
    expect(displayServerText("x‮y", 100)).toBe("x⟨U+202E⟩y");
    expect(displayServerText("\u0085", 100)).toBe("\\x85");
  });
  it("makes zero-width and invisible format characters visible, like the Rust sanitiser", () => {
    expect(displayServerText("deploy​.sh", 100)).toBe("deploy⟨U+200B⟩.sh");
    expect(displayServerText("deploy​.sh", 100)).not.toBe(displayServerText("deploy.sh", 100));
    expect(displayServerText("a‌b‍c", 100)).toBe("a⟨U+200C⟩b⟨U+200D⟩c");
    expect(displayServerText("a‎b‏c", 100)).toBe("a⟨U+200E⟩b⟨U+200F⟩c");
    expect(displayServerText("a⁠b", 100)).toBe("a⟨U+2060⟩b");
    expect(displayServerText("﻿a", 100)).toBe("⟨U+FEFF⟩a");
    expect(displayServerText("a­b", 100)).toBe("a⟨U+00AD⟩b");
    expect(displayServerText("a b c", 100)).toBe("a⟨U+2028⟩b⟨U+2029⟩c");
    expect(displayServerText("aㅤbﾠc", 100)).toBe("a⟨U+3164⟩b⟨U+FFA0⟩c");
    expect(displayServerText("a‪b‫c‬d‭e‮f", 100)).toBe(
      "a⟨U+202A⟩b⟨U+202B⟩c⟨U+202C⟩d⟨U+202D⟩e⟨U+202E⟩f",
    );
    expect(displayServerText("a⁦b⁧c⁨d⁩e", 100)).toBe("a⟨U+2066⟩b⟨U+2067⟩c⟨U+2068⟩d⟨U+2069⟩e");
    expect(displayServerText("a\u{E0041}b", 100)).toBe("a⟨U+E0041⟩b"); // Unicode tag characters (astral)
    expect(displayServerText("x\u{1F600}y", 100)).toBe("x\u{1F600}y"); // ordinary astral characters are untouched
    expect(displayServerText("café 日本 ❤️", 100)).toBe("café 日本 ❤️"); // text and emoji stay intact
  });
  it("escapes variation selectors 1-14 but keeps the text/emoji presentation selectors", () => {
    expect(displayServerText("a︀b", 100)).toBe("a⟨U+FE00⟩b");
    expect(displayServerText("a︍b", 100)).toBe("a⟨U+FE0D⟩b");
    expect(displayServerText("a︎b", 100)).toBe("a︎b");
    expect(displayServerText("a️b", 100)).toBe("a️b");
    expect(displayServerText("❤️", 100)).toBe("❤️");
    expect(displayServerText("a︀.txt", 100)).not.toBe(displayServerText("a.txt", 100));
    expect(displayServerText("a\u{E0100}b\u{E01EF}c", 100)).toBe("a⟨U+E0100⟩b⟨U+E01EF⟩c"); // VS17-256 via the tag range
    for (let cp = 0xfe00; cp <= 0xfe0f; cp++) {
      const ch = String.fromCodePoint(cp);
      const shown = displayServerText(ch, 100);
      expect(shown === ch).toBe(cp >= 0xfe0e);
    }
  });
  it("counts escape markers as characters when truncating", () => {
    // the limit applies to the escaped text (each marker is 8 code points), as for every other escape
    expect(displayServerText("︀︁︂", 3)).toBe("⟨U…");
    expect(displayServerText("︀︁", 9)).toBe("⟨U+FE00⟩…");
    expect(displayServerText("︀︁", 16)).toBe("⟨U+FE00⟩⟨U+FE01⟩");
    expect(displayServerText("ab︀cd", 100)).toBe("ab⟨U+FE00⟩cd");
  });
  it("truncates with an ellipsis", () => {
    expect(displayServerText("abcdefghij", 5)).toBe("abcd…");
    expect(displayServerText("abc", 5)).toBe("abc");
  });
  it("can keep newlines for banners", () => {
    expect(displayServerText("a\nb\tc", 100, true)).toBe("a\nb\tc");
    expect(displayServerText("a\nb", 100)).toBe("a\\x0ab");
  });
});

describe("remotePath", () => {
  it("handles POSIX paths", () => {
    expect(basename("/srv/app/x.txt")).toBe("x.txt");
    expect(basename("/")).toBe("/");
    expect(dirname("/srv/app/x.txt")).toBe("/srv/app");
    expect(dirname("/x")).toBe("/");
    expect(joinPath("/srv", "app")).toBe("/srv/app");
    expect(joinPath("/", "srv")).toBe("/srv");
    expect(segments("/srv/app")).toEqual(["srv", "app"]);
  });
});

describe("keybindings on the Ukrainian layout", () => {
  it("Ctrl+, (Settings) matches by the physical Comma key, which types a Cyrillic letter there", () => {
    expect(matchShortcut(key("б", { ctrlKey: true, code: "Comma" }), "windows", false)).toBe("settings");
    expect(matchShortcut(key("б", { metaKey: true, code: "Comma" }), "macos", false)).toBe("settings");
    expect(matchShortcut(key(",", { ctrlKey: true, code: "Comma" }), "windows", false)).toBe("settings");
  });
  it("zoom keys keep working with a non-ASCII key value", () => {
    expect(matchShortcut(key("ю", { ctrlKey: true, code: "Minus" }), "windows", false)).toBe("zoomOut");
    expect(matchShortcut(key("ю", { ctrlKey: true, code: "Equal" }), "windows", false)).toBe("zoomIn");
  });
  it("an ASCII key value wins over the physical code, and AltGr never maps to the physical key", () => {
    expect(matchShortcut(key(".", { ctrlKey: true, code: "Comma" }), "windows", false)).toBeNull();
    const altGr = { ...key("б", { ctrlKey: true, altKey: true, code: "Comma" }), getModifierState: () => true };
    expect(matchShortcut(altGr, "windows", false)).toBeNull();
  });
});

describe("keybindings (§8.7)", () => {
  it("Ctrl+K opens the palette outside the terminal", () => {
    expect(matchShortcut(key("k", { ctrlKey: true }), "windows", false)).toBe("palette");
  });
  it("plain Ctrl+K inside the terminal passes through to the shell; Ctrl+Shift+K opens the palette", () => {
    expect(matchShortcut(key("k", { ctrlKey: true }), "windows", true)).toBeNull();
    expect(matchShortcut(key("K", { ctrlKey: true, shiftKey: true }), "linux", true)).toBe("paletteInTerminal");
  });
  it("Cmd+K works inside the terminal on macOS", () => {
    expect(matchShortcut(key("k", { metaKey: true }), "macos", true)).toBe("palette");
  });
  it("other app shortcuts are ignored inside the terminal", () => {
    expect(matchShortcut(key("b", { ctrlKey: true }), "windows", true)).toBeNull();
    expect(matchShortcut(key("b", { ctrlKey: true }), "windows", false)).toBe("toggleSidebar");
  });
  it("terminal panel shortcuts work inside the terminal too (§8.7 'always')", () => {
    expect(matchShortcut(key("`", { ctrlKey: true }), "windows", true)).toBe("toggleTerminalPanel");
    expect(matchShortcut(key("~", { ctrlKey: true, shiftKey: true }), "linux", true)).toBe("newTerminal");
    expect(matchShortcut(key("`", { ctrlKey: true, shiftKey: true }), "linux", false)).toBe("newTerminal");
  });
  it("terminal find: Ctrl+Shift+F on Windows/Linux (Ctrl+F is readline), Cmd+F on macOS; only inside the terminal", () => {
    expect(matchShortcut(key("f", { ctrlKey: true }), "windows", true)).toBeNull();
    expect(matchShortcut(key("F", { ctrlKey: true, shiftKey: true }), "windows", true)).toBe("terminalFind");
    expect(matchShortcut(key("f", { metaKey: true }), "macos", true)).toBe("terminalFind");
    expect(matchShortcut(key("F", { ctrlKey: true, shiftKey: true }), "windows", false)).toBeNull();
  });
  it("terminal clipboard keys", () => {
    expect(terminalClipboardKey(key("C", { ctrlKey: true, shiftKey: true }), "windows")).toBe("copy");
    expect(terminalClipboardKey(key("V", { ctrlKey: true, shiftKey: true }), "linux")).toBe("paste");
    expect(terminalClipboardKey(key("Insert", { shiftKey: true }), "linux")).toBe("paste");
    expect(terminalClipboardKey(key("c", { ctrlKey: true }), "windows")).toBeNull(); // SIGINT stays with the remote
    expect(terminalClipboardKey(key("v", { ctrlKey: true }), "windows")).toBeNull();
    expect(terminalClipboardKey(key("v", { metaKey: true }), "macos")).toBe("paste");
    expect(terminalClipboardKey(key("c", { metaKey: true }), "macos")).toBe("copy");
  });
});

describe("misc", () => {
  it("formatEndpoint brackets IPv6", () => {
    expect(formatEndpoint("deploy", "10.0.4.12", 22)).toBe("deploy@10.0.4.12:22");
    expect(formatEndpoint("u", "::1", 2222)).toBe("u@[::1]:2222");
  });
  it("picks a language from the path", () => {
    expect(languageForPath("/srv/app/docker-compose.yml").label).toBe("YAML");
    expect(languageForPath("/srv/app/.env.production").id).toBe("properties");
    expect(languageForPath("/srv/app/README.md").id).toBe("plain");
  });
});
