// Parity with the Rust sanitiser (§9.5, issue #33): the shared fixture lists the code points both sides treat as
// controls or invisible/deceptive; the Rust test `deceptive_set_matches_shared_fixture` (ssh/shellquote.rs) checks
// `is_deceptive_char` against the same file.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { displayServerText } from "./serverText";

type Range = [string, string];
// The base goes through a variable: Vite rewrites the literal `new URL("…", import.meta.url)` form into an asset URL.
const here = import.meta.url;
const FIXTURE = fileURLToPath(new URL("../../src-tauri/tests/fixtures/deceptive_chars.json", here));
const fixture = JSON.parse(readFileSync(FIXTURE, "utf8")) as {
  control: Range[];
  deceptive: Range[];
  presentationSelectors: string[];
};
const parse = (ranges: Range[]) => ranges.map(([a, b]) => [parseInt(a, 16), parseInt(b, 16)] as const);
const escaped = [...parse(fixture.control), ...parse(fixture.deceptive)];
const within = (code: number) => escaped.some(([a, b]) => code >= a && code <= b);
const cp = (n: number) => String.fromCodePoint(n);

describe("displayServerText matches the Rust is_deceptive_char set", () => {
  it("escapes exactly the fixture's code points when they stand alone", () => {
    const wrong: string[] = [];
    for (let code = 0; code <= 0x10ffff; code++) {
      if (code >= 0xd800 && code <= 0xdfff) continue; // lone surrogates are not characters
      const ch = cp(code);
      if ((displayServerText(ch, 4) !== ch) !== within(code)) wrong.push(code.toString(16).toUpperCase());
    }
    expect(wrong).toEqual([]);
  });

  // Ties the set to Unicode (as known to this JavaScript engine): every Default_Ignorable_Code_Point and every
  // Bidi_Control must be in the fixture. No exclusions are needed today: U+FE0E/FE0F are in the fixture too (the
  // frontend's context rule for them is tested below), and the Hangul fillers U+115F/1160/3164/FFA0 are listed.
  // A newer Unicode version that adds such a code point makes this fail until the three lists are extended.
  const EXCLUDED: readonly number[] = [];
  it("contains every Default_Ignorable_Code_Point and Bidi_Control code point", () => {
    const re = /^[\p{Default_Ignorable_Code_Point}\p{Bidi_Control}]$/u;
    const missing: string[] = [];
    let seen = 0;
    for (let code = 0; code <= 0x10ffff; code++) {
      if (code >= 0xd800 && code <= 0xdfff) continue;
      if (!re.test(cp(code))) continue;
      seen++;
      if (!within(code) && !EXCLUDED.includes(code)) missing.push(code.toString(16).toUpperCase());
    }
    expect(seen).toBeGreaterThan(4000); // the property is really supported (E0000-E0FFF alone is 4096)
    expect(missing).toEqual([]);
  });

  it("keeps the presentation selectors only after an emoji they change", () => {
    const [text, emoji] = fixture.presentationSelectors.map((h) => cp(parseInt(h, 16)));
    expect(displayServerText(`${cp(0x2764)}${emoji}`, 10)).toBe(`${cp(0x2764)}${emoji}`);
    expect(displayServerText(`${cp(0x1f600)}${text}`, 10)).toBe(`${cp(0x1f600)}${text}`);
    expect(displayServerText(`a${emoji}`, 10)).not.toBe(`a${emoji}`);
  });

  it("escapes the interlinear annotation marks U+FFF9-FFFB and the block edges (#33)", () => {
    const L = cp(0x27e8);
    const R = cp(0x27e9);
    const esc = (code: number) => `${L}U+${code.toString(16).toUpperCase()}${R}`;
    const hostile = `ok${cp(0xfff9)}base${cp(0xfffa)}evil${cp(0xfffb)}.example`;
    expect(displayServerText(hostile, 100)).toBe(`ok${esc(0xfff9)}base${esc(0xfffa)}evil${esc(0xfffb)}.example`);
    expect(displayServerText(`${cp(0xffef)}${cp(0xfff0)}${cp(0xfff8)}${cp(0xfffc)}`, 100)).toBe(
      `${cp(0xffef)}${esc(0xfff0)}${esc(0xfff8)}${cp(0xfffc)}`,
    );
  });
});
