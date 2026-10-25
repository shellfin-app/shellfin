// #17 infrastructure with test-only fixture locales (never shipped): `&` mnemonic markers in the menu bar, CJK
// titles, accent-insensitive Alt+letter, the macOS native labels and the measured compact title bar.
import { act, fireEvent, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { i18next, setLanguage } from "../../i18n";
import uk from "../../i18n/locales/uk/translation.json";
import { setBackend } from "../../ipc";
import { MENU_IDS } from "../../lib/actionIds";
import { escapeNativeLabel } from "../../lib/menuMnemonic";
import { useStore } from "../../state/store";
import { CJK_MENUBAR, LONG_MENUBAR } from "../../test/fixtureLocales";
import { setupApp, settleApp } from "../../test/harness";
import { useMenuStore } from "../menu/menuStore";
import { COMPACT_TITLEBAR_BELOW, TITLEBAR_DRAG_MIN, TITLEBAR_TABS_MIN, isCompactTitleBar } from "./layoutLimits";
import { nativeMenuLabels } from "./nativeMenuLabels";

const originalWidth = window.innerWidth;

function setWidth(width: number): void {
  Object.defineProperty(window, "innerWidth", { configurable: true, writable: true, value: width });
}

/** Runs the test with the Ukrainian bundle's menu titles replaced by a fixture (the fixture is not a real locale). */
async function withMenubar(menubar: Record<string, string>): Promise<void> {
  i18next.addResourceBundle("uk", "translation", { menubar }, true, true);
  await act(() => setLanguage("uk"));
}

afterEach(async () => {
  await settleApp();
  i18next.addResourceBundle("uk", "translation", { menubar: uk.menubar }, true, true);
  await act(() => setLanguage("en"));
  setWidth(originalWidth);
  useMenuStore.getState().close();
  setBackend(null);
  vi.restoreAllMocks();
});

const bar = () => screen.getByRole("menubar");
const titles = () => within(bar()).getAllByRole("menuitem");

describe("CJK fixture: ファイル(&F) …", () => {
  it("shows the title without the marker and underlines the marked Latin letter", async () => {
    setWidth(1400);
    setupApp();
    await withMenubar(CJK_MENUBAR);
    expect(titles().map((b) => b.textContent)).toEqual(["ファイル(F)", "編集(E)", "表示(V)", "接続(C)", "ターミナル(T)", "ヘルプ(H)"]);
    const marks = titles().map((b) => b.querySelector(".menubar__mnemonic")?.textContent);
    expect(marks).toEqual(["F", "E", "V", "C", "T", "H"]);
    expect(bar().textContent).not.toContain("&");
  });

  it("Alt+F opens File; the dropdown's accessible name has no marker", async () => {
    setWidth(1400);
    setupApp();
    await withMenubar(CJK_MENUBAR);
    fireEvent.keyDown(document.body, { key: "f", code: "KeyF", altKey: true });
    expect(await screen.findByRole("menu", { name: "ファイル(F)" })).toBeTruthy();
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
    // A Japanese IME left in kana mode reports the kana, the physical key still opens the menu.
    fireEvent.keyDown(document.body, { key: "ひ", code: "KeyV", altKey: true });
    expect(await screen.findByRole("menu", { name: "表示(V)" })).toBeTruthy();
  });

  it("the macOS native menu shows the titles without marker and without the (F) group", async () => {
    await withMenubar({ ...CJK_MENUBAR, view: "表示（&V）" });
    const shown = Object.fromEntries(Object.entries(nativeMenuLabels()).map(([id, label]) => [id, mudaDisplay(label)]));
    expect(shown["menu.file"]).toBe("ファイル");
    expect(shown["menu.view"]).toBe("表示"); // full-width parentheses too
    expect(shown["menu.window"]).toBe("ウィンドウ");
    for (const [id, text] of Object.entries(shown)) {
      if (!id.startsWith("menu.")) continue;
      expect(text, id).not.toMatch(/[(（][A-Z][)）]/);
    }
  });
});

/** What muda shows on macOS for a label: `&&` is one `&`, a single `&` (a mnemonic marker) disappears. */
function mudaDisplay(label: string): string {
  return label.replace(/&(&?)/g, "$1");
}

describe("macOS native labels keep literal ampersands", () => {
  it("a literal & in a title or an action label survives muda's mnemonic stripping", async () => {
    await withMenubar({ ...LONG_MENUBAR, view: "Tools && &Help", help: "Q&&A" });
    i18next.addResourceBundle("uk", "translation", { action: { save: "Save & close" } }, true, true);
    const labels = nativeMenuLabels();
    expect(labels["menu.view"]).toBe("Tools && Help");
    expect(mudaDisplay(labels["menu.view"] ?? "")).toBe("Tools & Help");
    expect(mudaDisplay(labels["menu.help"] ?? "")).toBe("Q&A");
    expect(mudaDisplay(labels["file.save"] ?? "")).toBe("Save & close");
    expect(mudaDisplay(labels["menu.edit"] ?? "")).toBe("Édition des éléments");
    i18next.addResourceBundle("uk", "translation", { action: { save: uk.action.save } }, true, true);
  });

  it("escaping stays within 80 code points and never leaves half an && pair", () => {
    expect(escapeNativeLabel("a&b")).toBe("a&&b");
    expect(escapeNativeLabel(`${"x".repeat(79)}&`, 80)).toBe("x".repeat(79));
    expect(escapeNativeLabel(`${"x".repeat(78)}&`, 80)).toBe(`${"x".repeat(78)}&&`);
    expect(Array.from(escapeNativeLabel("\u{1F600}".repeat(90), 80))).toHaveLength(80);
  });

});

describe("CJK fixture: compact menu", () => {
  it("the compact list uses the same labels", async () => {
    setWidth(700);
    const { user } = setupApp();
    await withMenubar(CJK_MENUBAR);
    await user.click(within(bar()).getByRole("menuitem", { name: "Меню" }));
    const root = await screen.findByRole("menu", { name: "Меню" });
    expect(within(root).getAllByRole("menuitem").map((b) => b.textContent)).toEqual([
      "ファイル(F)", "編集(E)", "表示(V)", "接続(C)", "ターミナル(T)", "ヘルプ(H)",
    ]);
  });
});

describe("long accented fixture: markers resolve collisions, accents fold", () => {
  it("underlines the marked letters; Alt+U opens Help, Alt+E and Alt+É open Edit", async () => {
    setWidth(1400);
    setupApp();
    await withMenubar(LONG_MENUBAR);
    const help = titles()[MENU_IDS.indexOf("help")];
    expect(help?.textContent).toBe("Ayuda und Unterstützung");
    expect(help?.querySelector(".menubar__mnemonic")?.textContent).toBe("u");
    fireEvent.keyDown(document.body, { key: "u", code: "KeyU", altKey: true });
    expect(await screen.findByRole("menu", { name: "Ayuda und Unterstützung" })).toBeTruthy();
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
    fireEvent.keyDown(document.body, { key: "e", code: "KeyE", altKey: true });
    expect(await screen.findByRole("menu", { name: "Édition des éléments" })).toBeTruthy();
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
    // AZERTY: the É key without a dead key.
    fireEvent.keyDown(document.body, { key: "É", code: "Digit2", altKey: true });
    expect(await screen.findByRole("menu", { name: "Édition des éléments" })).toBeTruthy();
  });
});

describe("compact title bar by measurement", () => {
  const fit = { contentWidth: 1000, menuWidth: 400, othersWidth: 100 };

  it("below 960 px always compact; above it compact exactly when the titles do not fit", () => {
    expect(isCompactTitleBar(COMPACT_TITLEBAR_BELOW - 1, fit)).toBe(true);
    expect(isCompactTitleBar(COMPACT_TITLEBAR_BELOW, fit)).toBe(false);
    const exact = fit.menuWidth + fit.othersWidth + TITLEBAR_TABS_MIN + TITLEBAR_DRAG_MIN;
    expect(isCompactTitleBar(1400, { ...fit, contentWidth: exact })).toBe(false);
    expect(isCompactTitleBar(1400, { ...fit, contentWidth: exact - 1 })).toBe(true);
  });

  it("without a usable measurement only the width bound decides (jsdom, first paint)", () => {
    expect(isCompactTitleBar(1400, null)).toBe(false);
    expect(isCompactTitleBar(1400, { contentWidth: 0, menuWidth: 900, othersWidth: 0 })).toBe(false);
    expect(isCompactTitleBar(1400, { contentWidth: 500, menuWidth: 0, othersWidth: 0 })).toBe(false);
    expect(isCompactTitleBar(1400, { contentWidth: 500, menuWidth: 900, othersWidth: Number.NaN })).toBe(false);
    expect(isCompactTitleBar(700, null)).toBe(true);
  });

  /** Fake layout: each menu title `titleWidth` px, the row `rowWidth` px, logo 20 px, search button 30 px, gap 8 px. */
  function fakeLayout(sizes: { titleWidth: number; rowWidth: number }) {
    vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
      const w = this.matches(".menubar__item:not(.menubar__item--compact)")
        ? sizes.titleWidth
        : this.matches(".title-bar__logo")
          ? 20
          : this.matches(".search-box")
            ? 30
            : 0;
      return { width: w, height: 0, x: 0, y: 0, top: 0, left: 0, right: w, bottom: 0, toJSON: () => ({}) } as DOMRect;
    });
    const clientWidth = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "clientWidth");
    vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockImplementation(function (this: HTMLElement) {
      return this.matches(".title-bar__content") ? sizes.rowWidth : (clientWidth?.get?.call(this) as number);
    });
    const computed = window.getComputedStyle.bind(window);
    vi.spyOn(window, "getComputedStyle").mockImplementation((el: Element, pseudo?: string | null) => {
      const style = computed(el, pseudo);
      if (!el.matches(".title-bar__content")) return style;
      return new Proxy(style, {
        get: (target, prop) =>
          prop === "paddingLeft" ? "0px" : prop === "paddingRight" ? "4px" : prop === "columnGap" ? "8px" : (Reflect.get(target, prop) as unknown),
      });
    });
  }

  /** Captures the ResizeObserver callbacks, so a test can deliver a resize. */
  function captureObservers(): (() => void)[] {
    const callbacks: (() => void)[] = [];
    class Capturing {
      constructor(callback: () => void) {
        callbacks.push(callback);
      }
      observe() {}
      unobserve() {}
      disconnect() {}
    }
    vi.stubGlobal("ResizeObserver", Capturing);
    return callbacks;
  }

  it("a wide window whose titles do not fit gets the compact button; widening brings the titles back", () => {
    const sizes = { titleWidth: 150, rowWidth: 900 };
    fakeLayout(sizes);
    const observers = captureObservers();
    setWidth(1100);
    setupApp();
    // 6 x 150 + logo 20 + search 30 + 4 gaps... > 900 - 4: no room for the 140 px tab strip.
    expect(screen.getByRole("menuitem", { name: "Menu" })).toBeTruthy();
    act(() => {
      sizes.rowWidth = 1400;
      setWidth(1500);
      window.dispatchEvent(new Event("resize"));
      for (const cb of observers) cb();
    });
    expect(screen.queryByRole("menuitem", { name: "Menu" })).toBeNull();
    expect(titles()).toHaveLength(6);
    vi.unstubAllGlobals();
  });

  it("titles that fit stay at 1100 px; keyboard parity holds in the measured compact mode", async () => {
    const sizes = { titleWidth: 60, rowWidth: 900 };
    fakeLayout(sizes);
    const observers = captureObservers();
    setWidth(1100);
    const { user } = setupApp();
    expect(titles()).toHaveLength(6);
    act(() => {
      sizes.titleWidth = 150; // a longer language or a bigger UI scale: the bar resizes
      for (const cb of observers) cb();
    });
    const button = screen.getByRole("menuitem", { name: "Menu" });
    await user.keyboard("{F10}");
    expect(await screen.findByRole("menu", { name: "File" })).toBeTruthy();
    await user.keyboard("{Escape}");
    fireEvent.keyDown(document.body, { key: "h", code: "KeyH", altKey: true });
    expect(await screen.findByRole("menu", { name: "Help" })).toBeTruthy();
    await user.keyboard("{Escape}");
    fireEvent.keyDown(document.body, { key: "Alt" });
    fireEvent.keyUp(document.body, { key: "Alt" });
    expect(document.activeElement).toBe(button);
    vi.unstubAllGlobals();
  });

  it("macOS never shows the in-app bar, measured or not", () => {
    fakeLayout({ titleWidth: 150, rowWidth: 900 });
    setWidth(1100);
    setupApp();
    act(() => useStore.setState({ platform: "macos" }));
    expect(screen.queryByRole("menubar")).toBeNull();
  });
});
