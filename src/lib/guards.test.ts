import { afterEach, describe, expect, it } from "vitest";
import { isBrowserAccelerator } from "./accelerators";
import { installDragDropGuard } from "./dragdrop";
import type { KeyEventLike } from "./keybindings";
import { displayLocalPath } from "./localPath";

const key = (k: string, mods: Partial<KeyEventLike> = {}): KeyEventLike => ({
  key: k, ctrlKey: false, metaKey: false, shiftKey: false, altKey: false, ...mods,
});

describe("isBrowserAccelerator", () => {
  it("catches reload, print, view-source, save, find, zoom and history keys", () => {
    for (const k of ["F5", "F3", "F7"]) expect(isBrowserAccelerator(key(k), "windows"), k).toBe(true);
    for (const k of ["r", "p", "u", "s", "f", "g", "o", "h", "j", "d", "=", "+", "-", "0"]) {
      expect(isBrowserAccelerator(key(k, { ctrlKey: true }), "linux"), `Ctrl+${k}`).toBe(true);
    }
    expect(isBrowserAccelerator(key("R", { ctrlKey: true, shiftKey: true }), "windows")).toBe(true);
    expect(isBrowserAccelerator(key("r", { metaKey: true }), "macos")).toBe(true);
    expect(isBrowserAccelerator(key("ArrowLeft", { altKey: true }), "windows")).toBe(true);
    expect(isBrowserAccelerator(key("ArrowRight", { altKey: true }), "linux")).toBe(true);
  });

  it("leaves everything the app, the editor and the shell need alone", () => {
    for (const k of ["a", "c", "v", "x", "z", "k", "n", "b", "w", "t", "l"]) {
      expect(isBrowserAccelerator(key(k, { ctrlKey: true }), "windows"), `Ctrl+${k}`).toBe(false);
    }
    expect(isBrowserAccelerator(key("r"), "windows")).toBe(false); // plain typing
    expect(isBrowserAccelerator(key("V", { ctrlKey: true, shiftKey: true }), "windows")).toBe(false); // paste
    expect(isBrowserAccelerator(key("C", { ctrlKey: true, shiftKey: true }), "windows")).toBe(false);
    expect(isBrowserAccelerator(key("ArrowLeft", { altKey: true }), "macos")).toBe(false); // word movement
    expect(isBrowserAccelerator(key("r", { ctrlKey: true, altKey: true }), "windows")).toBe(false); // AltGr
    expect(isBrowserAccelerator(key("r", { ctrlKey: true }), "macos")).toBe(false); // Ctrl+R is the shell's on a Mac
  });
});

describe("drag and drop guard", () => {
  let uninstall: (() => void) | null = null;
  afterEach(() => {
    uninstall?.();
    document.body.replaceChildren();
  });

  /** Builds `<tag attrs>` elements under `parent` (no innerHTML in this code base). */
  function el(tag: string, parent: Element, attrs: Record<string, string> = {}): HTMLElement {
    const node = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs)) node.setAttribute(k, v);
    parent.appendChild(node);
    return node;
  }

  function drop(target: Element, type = "drop"): Event {
    const event = new Event(type, { bubbles: true, cancelable: true });
    Object.defineProperty(event, "dataTransfer", { value: { dropEffect: "copy" } });
    target.dispatchEvent(event);
    return event;
  }

  it("a drop on ordinary UI is cancelled and shows the 'not allowed' cursor", () => {
    uninstall = installDragDropGuard();
    const span = el("span", el("div", document.body));
    expect(drop(span).defaultPrevented).toBe(true);
    const over = drop(span, "dragover");
    expect((over as unknown as { dataTransfer: { dropEffect: string } }).dataTransfer.dropEffect).toBe("none");
    expect(over.defaultPrevented).toBe(false); // not preventDefault: that would ALLOW the drop
  });

  it("drop zones and text fields keep their drops; the terminal does not", () => {
    uninstall = installDragDropGuard();
    const zone = el("p", el("div", document.body, { "data-drop-zone": "" }));
    const input = el("input", document.body);
    const xtermTextarea = el("textarea", el("div", document.body, { class: "xterm" }));
    expect(drop(zone).defaultPrevented).toBe(false);
    expect(drop(input).defaultPrevented).toBe(false);
    expect(drop(xtermTextarea).defaultPrevented).toBe(true);
  });

  it("links and images cannot be dragged out", () => {
    uninstall = installDragDropGuard();
    const link = el("a", document.body, { href: "https://example.com" });
    expect(drop(link, "dragstart").defaultPrevented).toBe(true);
  });
});

describe("displayLocalPath", () => {
  it("uses one separator style per platform", () => {
    expect(displayLocalPath("~\\.ssh/id_ed25519_enc", "windows")).toBe("~\\.ssh\\id_ed25519_enc");
    expect(displayLocalPath("~/.ssh/id_ed25519", "linux")).toBe("~/.ssh/id_ed25519");
  });
});
