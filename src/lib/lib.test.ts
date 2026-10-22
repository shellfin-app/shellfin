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
  // Invisible and combining characters are built from code points so this source stays plain ASCII there.
  const cp = (n: number) => String.fromCodePoint(n);
  const VS = (n: number) => cp(0xfe00 + n); // VS(0) = U+FE00 ... VS(15) = U+FE0F
  const VS15 = cp(0xfe0e);
  const VS16 = cp(0xfe0f);
  const KEYCAP = cp(0x20e3);
  const ZWSP = cp(0x200b);
  const HEART = cp(0x2764); // text-default emoji
  const COPY = cp(0xa9); // text-default emoji
  const GRIN = cp(0x1f600); // emoji-default
  const L = cp(0x27e8); // escape marker brackets
  const R = cp(0x27e9);
  const esc = (code: number) => `${L}U+${code.toString(16).toUpperCase()}${R}`;
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
  it("escapes variation selectors 1-14 always", () => {
    expect(displayServerText(`a${VS(0)}b`, 100)).toBe(`a${esc(0xfe00)}b`);
    expect(displayServerText(`a${VS(13)}b`, 100)).toBe(`a${esc(0xfe0d)}b`);
    expect(displayServerText(`${HEART}${VS(0)}`, 100)).toBe(`${HEART}${esc(0xfe00)}`);
    expect(displayServerText(`a${VS(0)}.txt`, 100)).not.toBe(displayServerText("a.txt", 100));
    // VS17-256 live in the tag range
    expect(displayServerText(`a${cp(0xe0100)}b${cp(0xe01ef)}c`, 100)).toBe(`a${esc(0xe0100)}b${esc(0xe01ef)}c`);
    for (let code = 0xfe00; code <= 0xfe0d; code++) {
      const ch = cp(code);
      expect(displayServerText(ch, 100)).not.toBe(ch);
      expect(displayServerText(`${HEART}${ch}`, 100)).toBe(`${HEART}${esc(code)}`);
    }
  });
  it("keeps U+FE0E/FE0F only where they change the presentation of the previous character", () => {
    expect(displayServerText(`${HEART}${VS16}`, 100)).toBe(`${HEART}${VS16}`); // text-default emoji -> emoji style
    expect(displayServerText(`${COPY}${VS16}`, 100)).toBe(`${COPY}${VS16}`);
    expect(displayServerText(`1${VS16}${KEYCAP}`, 100)).toBe(`1${VS16}${KEYCAP}`); // keycap sequences
    expect(displayServerText(`#${VS16}${KEYCAP}`, 100)).toBe(`#${VS16}${KEYCAP}`);
    expect(displayServerText(`${GRIN}${VS15}`, 100)).toBe(`${GRIN}${VS15}`); // emoji-default -> text style
  });
  it("escapes presentation selectors that change nothing, so look-alike names differ", () => {
    expect(displayServerText(`a${VS16}`, 100)).toBe(`a${esc(0xfe0f)}`);
    expect(displayServerText(`a${VS15}`, 100)).toBe(`a${esc(0xfe0e)}`);
    expect(displayServerText(`1${VS16}`, 100)).toBe(`1${esc(0xfe0f)}`); // no keycap
    expect(displayServerText(`1${VS15}${KEYCAP}`, 100)).toBe(`1${esc(0xfe0e)}${KEYCAP}`);
    expect(displayServerText(`${GRIN}${VS16}`, 100)).toBe(`${GRIN}${esc(0xfe0f)}`); // already emoji style
    expect(displayServerText(`${HEART}${VS15}`, 100)).toBe(`${HEART}${esc(0xfe0e)}`); // already text style
    expect(displayServerText(`${HEART}${VS16}${VS16}`, 100)).toBe(`${HEART}${VS16}${esc(0xfe0f)}`); // doubled
    expect(displayServerText(VS16, 100)).toBe(esc(0xfe0f)); // at the start
    expect(displayServerText(`${ZWSP}${VS16}`, 100)).toBe(`${esc(0x200b)}${esc(0xfe0f)}`); // after an escaped character
    const pairs: [string, string][] = [
      ["report.pdf", `report${VS16}.pdf`],
      ["a.txt", `a${VS15}.txt`],
      ["file1", `file1${VS16}`],
      [GRIN, `${GRIN}${VS16}`],
      [HEART, `${HEART}${VS15}`],
    ];
    for (const [plain, spoof] of pairs) {
      expect(displayServerText(spoof, 100)).not.toBe(displayServerText(plain, 100));
    }
  });
  it("counts escape markers as characters when truncating", () => {
    // the limit applies to the escaped text (each marker is 8 code points), as for every other escape
    expect(displayServerText(`${VS(0)}${VS(1)}${VS(2)}`, 3)).toBe(`${L}U…`);
    expect(displayServerText(`${VS(0)}${VS(1)}`, 9)).toBe(`${esc(0xfe00)}…`);
    expect(displayServerText(`${VS(0)}${VS(1)}`, 16)).toBe(`${esc(0xfe00)}${esc(0xfe01)}`);
    expect(displayServerText(`ab${VS(0)}cd`, 100)).toBe(`ab${esc(0xfe00)}cd`);
  });
  it("is idempotent when truncation cuts a keycap sequence", () => {
    const input = `x1${VS16}${KEYCAP}y`;
    const once = displayServerText(input, 4);
    // the FE0F would be orphaned by the cut, so it is dropped with the keycap
    expect(once).toBe("x1…");
    expect(displayServerText(once, 4)).toBe(once);
    // a cut after the keycap keeps the whole sequence
    expect(displayServerText(`1${VS16}${KEYCAP}yz`, 4)).toBe(`1${VS16}${KEYCAP}…`);
    // a cut right after a meaningful FE0F (no keycap follows) keeps it
    expect(displayServerText(`${HEART}${VS16}yz`, 3)).toBe(`${HEART}${VS16}…`);
    // a keycap mark after a base that is not a keycap base does not drop the FE0F
    expect(displayServerText(`${HEART}${VS16}${KEYCAP}yz`, 3)).toBe(`${HEART}${VS16}…`);
  });
  it("gives the same result when applied twice with the same limit", () => {
    const inputs = [
      `1${VS16}${KEYCAP}`,
      `x1${VS16}${KEYCAP}y`,
      `#${VS16}${KEYCAP}#${VS16}${KEYCAP}`,
      `${HEART}${VS16}${HEART}${VS16}${HEART}${VS16}`,
      `${HEART}${VS16}${VS16}${VS15}`,
      `a${VS(0)}b${VS(5)}c`,
      `${ZWSP}${ZWSP}${VS16}${KEYCAP}`,
      `${GRIN}${VS15}${GRIN}${VS16}`,
      "a-long-file-name-with-many-characters.txt",
    ];
    for (const input of inputs) {
      for (const limit of [1, 2, 3, 4, 5, 6, 8, 10, 100]) {
        const once = displayServerText(input, limit);
        expect(displayServerText(once, limit)).toBe(once);
      }
    }
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
