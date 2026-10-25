// The compact title bar (< 960 px), the window-controls hover from the Snap Layouts overlay and the narrow status bar.
import { readFileSync } from "node:fs";
import { act, fireEvent, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import { setupApp, settleApp } from "../../test/harness";
import { useStore } from "../../state/store";
import { useMenuStore } from "../menu/menuStore";
import { CAPTION_ZONE_HEIGHT, CAPTION_ZONE_WIDTH } from "../menu/menuPlacement";
import { COMPACT_STATUSBAR_BELOW, COMPACT_TITLEBAR_BELOW, isCompactTitleBar } from "./layoutLimits";

const originalWidth = window.innerWidth;

function setWidth(width: number): void {
  Object.defineProperty(window, "innerWidth", { configurable: true, writable: true, value: width });
}

afterEach(async () => {
  await settleApp();
  setWidth(originalWidth);
  useMenuStore.getState().close();
  setBackend(null);
});

function menuButton(): HTMLElement {
  return within(screen.getByRole("menubar", { name: "Application menu" })).getByRole("menuitem", { name: "Menu" });
}

/** The declarations of the first rule that starts with `selector` followed by an opening brace. */
function ruleBody(css: string, selector: string): string {
  const start = css.indexOf(`\n${selector} {`);
  if (start < 0) throw new Error(`${selector} not found`);
  return css.slice(start, css.indexOf("}", start));
}

describe("compact title bar", () => {
  it("the threshold is 960 px", () => {
    expect(isCompactTitleBar(COMPACT_TITLEBAR_BELOW - 1)).toBe(true);
    expect(isCompactTitleBar(COMPACT_TITLEBAR_BELOW)).toBe(false);
  });

  it("at 960 px and wider the six menus are shown, no menu button", () => {
    setWidth(960);
    setupApp();
    const names = within(screen.getByRole("menubar")).getAllByRole("menuitem").map((b) => b.textContent);
    expect(names).toEqual(["File", "Edit", "View", "Connection", "Terminal", "Help"]);
    expect(screen.queryByRole("menuitem", { name: "Menu" })).toBeNull();
  });

  it("below 960 px one menu button replaces the titles; the window controls stay the last item", () => {
    setWidth(700);
    setupApp();
    const items = within(screen.getByRole("menubar")).getAllByRole("menuitem");
    expect(items).toHaveLength(1);
    expect(menuButton().getAttribute("aria-haspopup")).toBe("menu");
    const bar = document.querySelector(".title-bar");
    expect(bar?.lastElementChild?.className).toBe("window-controls");
    expect(bar?.querySelector(":scope > .title-bar__content")).not.toBeNull();
  });

  it("follows a window resize", () => {
    setWidth(1200);
    setupApp();
    expect(screen.queryByRole("menuitem", { name: "Menu" })).toBeNull();
    act(() => {
      setWidth(600);
      window.dispatchEvent(new Event("resize"));
    });
    expect(menuButton()).toBeTruthy();
    act(() => {
      setWidth(1200);
      window.dispatchEvent(new Event("resize"));
    });
    expect(screen.queryByRole("menuitem", { name: "Menu" })).toBeNull();
  });

  it("mouse: the button lists the six menus, a menu opens, Menu goes back", async () => {
    setWidth(700);
    const { user } = setupApp();
    await user.click(menuButton());
    const root = await screen.findByRole("menu", { name: "Menu" });
    expect(within(root).getAllByRole("menuitem").map((b) => b.textContent)).toEqual(["File", "Edit", "View", "Connection", "Terminal", "Help"]);
    await user.click(within(root).getByRole("menuitem", { name: "View" }));
    const view = await screen.findByRole("menu", { name: "View" });
    expect(within(view).getByRole("menuitemcheckbox", { name: /Toggle sidebar/ })).toBeTruthy();
    await user.click(within(view).getByRole("menuitem", { name: "Menu" }));
    expect(await screen.findByRole("menu", { name: "Menu" })).toBeTruthy();
    expect(document.activeElement?.textContent).toBe("View");
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("mouse: an item of a menu runs its action", async () => {
    setWidth(700);
    const { user } = setupApp();
    await user.click(menuButton());
    await user.click(within(await screen.findByRole("menu", { name: "Menu" })).getByRole("menuitem", { name: "File" }));
    await user.click(within(await screen.findByRole("menu", { name: "File" })).getByRole("menuitem", { name: /New connection/ }));
    expect(await screen.findByRole("dialog", { name: "New connection" })).toBeTruthy();
  });

  it("keyboard: Enter on the button opens the list, arrows and Enter reach a menu, Left/Right switch menus", async () => {
    setWidth(700);
    const { user } = setupApp();
    menuButton().focus();
    await user.keyboard("{Enter}");
    expect(await screen.findByRole("menu", { name: "Menu" })).toBeTruthy();
    expect(document.activeElement?.textContent).toBe("File");
    await user.keyboard("{ArrowDown}{Enter}");
    expect(await screen.findByRole("menu", { name: "Edit" })).toBeTruthy();
    // The first real item is focused, not the "Menu" entry.
    expect(document.activeElement?.hasAttribute("data-menu-back")).toBe(false);
    await user.keyboard("{ArrowRight}");
    expect(await screen.findByRole("menu", { name: "View" })).toBeTruthy();
    await user.keyboard("{Backspace}");
    expect(await screen.findByRole("menu", { name: "Menu" })).toBeTruthy();
    // Back lands on the menu it came from, not on "File".
    expect(document.activeElement?.textContent).toBe("View");
    await user.keyboard("{ArrowDown}{ArrowRight}");
    expect(await screen.findByRole("menu", { name: "Connection" })).toBeTruthy();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("keyboard: F10, Alt+letter and an Alt tap still reach the menus", async () => {
    setWidth(700);
    const { user } = setupApp();
    await user.keyboard("{F10}");
    expect(await screen.findByRole("menu", { name: "File" })).toBeTruthy();
    await user.keyboard("{Escape}");
    fireEvent.keyDown(document.body, { key: "v", altKey: true });
    expect(await screen.findByRole("menu", { name: "View" })).toBeTruthy();
    await user.keyboard("{Escape}");
    fireEvent.keyDown(document.body, { key: "Alt" });
    fireEvent.keyUp(document.body, { key: "Alt" });
    expect(document.activeElement).toBe(menuButton());
  });

  it("keyboard: a menu with only disabled items (Terminal on Home) keeps the focus; Escape, arrows, Backspace work", async () => {
    // Full menu bar.
    setWidth(1200);
    const { user } = setupApp();
    await user.keyboard("{F10}");
    expect(await screen.findByRole("menu", { name: "File" })).toBeTruthy();
    await user.keyboard("{ArrowRight}{ArrowRight}{ArrowRight}{ArrowRight}");
    const terminal = await screen.findByRole("menu", { name: "Terminal" });
    expect(document.activeElement).toBe(terminal);
    await user.keyboard("{ArrowLeft}");
    expect(await screen.findByRole("menu", { name: "Connection" })).toBeTruthy();
    await user.keyboard("{ArrowRight}");
    expect(document.activeElement).toBe(await screen.findByRole("menu", { name: "Terminal" }));
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).toBeNull();
    expect(document.activeElement).not.toBe(document.body);
  });

  it("compact: in a menu with only disabled items the Menu entry keeps the focus", async () => {
    setWidth(700);
    const { user } = setupApp();
    menuButton().focus();
    await user.keyboard("{Enter}");
    expect(await screen.findByRole("menu", { name: "Menu" })).toBeTruthy();
    await user.keyboard("{ArrowDown}{ArrowDown}{ArrowDown}{ArrowDown}{Enter}");
    const terminal = await screen.findByRole("menu", { name: "Terminal" });
    expect(document.activeElement).toBe(within(terminal).getByRole("menuitem", { name: "Menu" }));
    await user.keyboard("{Backspace}");
    expect(await screen.findByRole("menu", { name: "Menu" })).toBeTruthy();
    expect(document.activeElement?.textContent).toBe("Terminal");
    await user.keyboard("{ArrowRight}");
    expect(await screen.findByRole("menu", { name: "Terminal" })).toBeTruthy();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("macOS has no in-app menu, compact or not", () => {
    setWidth(700);
    setupApp();
    act(() => useStore.setState({ platform: "macos" }));
    expect(screen.queryByRole("menubar")).toBeNull();
  });

  it("the CSS keeps the tab strip at 140 px, the controls out of the shrinking part and the status bar group fixed", () => {
    const css = readFileSync("src/features/shell/shell.css", "utf8");
    expect(ruleBody(css, ".conn-tabs-wrap")).toContain("min-width: 140px;");
    const content = ruleBody(css, ".title-bar__content");
    expect(content).toContain("min-width: 0;");
    expect(content).toContain("overflow: hidden;");
    // The same strict "below" as isCompactTitleBar / COMPACT_STATUSBAR_BELOW (no off-by-one max-width form).
    expect(css).toContain(`@media (width < ${COMPACT_STATUSBAR_BELOW}px)`);
    expect(css).not.toMatch(/max-width:\s*\d+px\)/);
    expect(ruleBody(css, ".status-bar__group")).toContain("flex: none;");
    expect(ruleBody(css, ".status-bar__item--strong")).toContain("min-width: 0;");
  });
});

describe("maximize button hover from the Snap Layouts overlay", () => {
  it("window:max-hover toggles the hover class; the click still works", async () => {
    const { user, backend } = setupApp();
    const button = () => screen.getByRole("button", { name: "Maximize" });
    await settleApp();
    expect(button().className).not.toContain("window-controls__button--hover");
    act(() => backend.emit("window:max-hover", { hovered: true }));
    expect(button().className).toContain("window-controls__button--hover");
    act(() => backend.emit("window:max-hover", { hovered: false }));
    expect(button().className).not.toContain("window-controls__button--hover");
    await user.click(button());
    expect(backend.windowCalls).toContain("toggleMaximize");
  });
});

describe("dialogs keep clear of the Snap Layouts overlay (Windows)", () => {
  const dialogCss = readFileSync("src/components/dialog.css", "utf8");
  const paletteCss = readFileSync("src/features/palette/palette.css", "utf8");
  const tokens = readFileSync("src/styles/tokens.css", "utf8");
  const conf = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8")) as {
    app: { windows: { minWidth: number; minHeight: number }[] };
  };

  it("<html data-platform> follows the platform", () => {
    setupApp();
    expect(document.documentElement.dataset.platform).toBe("windows");
    act(() => useStore.setState({ platform: "linux" }));
    expect(document.documentElement.dataset.platform).toBe("linux");
  });

  it("the backdrop and the palette start below the caption zone on Windows only", () => {
    expect(readFileSync("src/styles/global.css", "utf8")).toContain(
      ':root[data-platform="windows"] { --caption-clearance: calc(var(--h-titlebar) + 6px); }',
    );
    const backdrop = ruleBody(dialogCss, ".dialog-backdrop");
    expect(backdrop).toContain("padding-top: max(20px, var(--caption-clearance, 0px));");
    expect(backdrop).toContain("align-items: center;");
    expect(paletteCss).toContain(".palette-backdrop { align-items: flex-start; padding-top: max(12vh, var(--caption-clearance, 0px)); }");
    // The dialog shrinks into the backdrop (its body scrolls) instead of growing past it.
    const dialog = ruleBody(dialogCss, ".dialog");
    expect(dialog).toContain("max-height: 100%;");
    expect(dialog).toContain("max-width: 100%;");
    expect(ruleBody(dialogCss, ".dialog__body")).toContain("overflow: auto;");
  });

  it("at the minimum window size (580 x 500) a dialog fits and its top edge is below the overlay", () => {
    const titleBar = Number(/--h-titlebar: (\d+)px;/.exec(tokens)?.[1]);
    expect(titleBar).toBe(CAPTION_ZONE_HEIGHT);
    const main = conf.app.windows[0];
    if (!main) throw new Error("no window in tauri.conf.json");
    const { minWidth, minHeight } = main;
    expect([minWidth, minHeight]).toEqual([580, 500]);
    const top = Math.max(20, titleBar + 6);
    const available = minHeight - top - 20;
    // The overlay spans y 0..44 (at most) at x minWidth - 92 .. minWidth - 46: the dialog header starts lower.
    expect(top).toBeGreaterThan(CAPTION_ZONE_HEIGHT);
    // Room for the header, some body and the footer.
    expect(available).toBeGreaterThanOrEqual(400);
    // A wide dialog (Settings: 540 px, right edge at minWidth - 20) does reach into the overlay's columns, so the
    // vertical clearance is what keeps its header and close button clickable.
    expect(minWidth - 20).toBeGreaterThan(minWidth - CAPTION_ZONE_WIDTH);
  });
});

describe("connection tab strip", () => {
  it("a resize of the strip scrolls the active tab back into view (instant, nearest)", () => {
    const observed: { target: Element; callback: () => void }[] = [];
    const Original = globalThis.ResizeObserver;
    class Capturing {
      constructor(private readonly callback: () => void) {}
      observe(target: Element) {
        observed.push({ target, callback: this.callback });
      }
      unobserve() {}
      disconnect() {}
    }
    globalThis.ResizeObserver = Capturing as unknown as typeof ResizeObserver;
    const proto = HTMLElement.prototype as { scrollIntoView?: (arg?: ScrollIntoViewOptions) => void };
    const originalScroll = proto.scrollIntoView;
    const scroll = vi.fn();
    proto.scrollIntoView = scroll;
    try {
      setupApp();
      const strip = document.querySelector(".conn-tabs");
      const entry = observed.find((o) => o.target === strip);
      expect(entry).toBeDefined();
      scroll.mockClear();
      let queued: FrameRequestCallback | undefined;
      const raf = vi.spyOn(window, "requestAnimationFrame").mockImplementation((cb) => {
        queued = cb;
        return 1;
      });
      // Not inside the observer callback (no layout change during delivery): on the next frame, once per burst.
      act(() => {
        entry?.callback();
        entry?.callback();
      });
      expect(raf).toHaveBeenCalledTimes(1);
      expect(scroll).not.toHaveBeenCalled();
      act(() => queued?.(0));
      raf.mockRestore();
      expect(scroll).toHaveBeenCalledWith({ block: "nearest", inline: "nearest" });
      expect((scroll.mock.contexts[0] as Element).classList.contains("conn-tab--active")).toBe(true);
    } finally {
      globalThis.ResizeObserver = Original;
      if (originalScroll) proto.scrollIntoView = originalScroll;
      else delete proto.scrollIntoView;
    }
  });
});

describe("minimum window (580 px): nothing overlaps, nothing scrolls sideways", () => {
  const read = (path: string) => readFileSync(path, "utf8");

  it("notices stop below the caption zone and scroll (Windows)", () => {
    const notices = ruleBody(read("src/styles/global.css"), ".notices");
    expect(notices).toContain("bottom: 40px;");
    expect(notices).toContain("max-height: calc(100vh - 40px - var(--caption-clearance, 0px) - 8px); overflow-y: auto;");
    expect(read("src/styles/global.css")).toContain(".notices > * { flex: none; }");
    // At 580 x 500 on Windows the top of a full stack is at 500 - 40 - (500 - 40 - 50 - 8) = 58 px, below the 44 px zone.
    const top = 500 - 40 - (500 - 40 - (CAPTION_ZONE_HEIGHT + 6) - 8);
    expect(top).toBeGreaterThan(CAPTION_ZONE_HEIGHT);
  });

  it("sidebar section titles truncate before their buttons; the files toolbar never shrinks", () => {
    const shell = read("src/features/shell/shell.css");
    const lead = ruleBody(shell, ".sidebar__title-lead");
    expect(lead).toContain("min-width: 0;");
    expect(lead).toContain("overflow: hidden;");
    expect(lead).toContain("flex: 0 1 auto;");
    expect(shell).toContain(".sidebar__title-lead > .sec { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }");
    const files = read("src/features/files/files.css");
    expect(ruleBody(files, ".files-toolbar")).toContain("flex: none;");
    expect(ruleBody(read("src/features/hosts/hosts.css"), ".panel-header__actions")).toContain("flex: none;");
  });

  it("the terminal panel bar keeps its buttons: title truncates, tabs scroll inside, no horizontal scrollbar", () => {
    const css = read("src/features/terminal/terminal.css");
    const bar = ruleBody(css, ".terminal-panel__bar");
    expect(bar).toContain("overflow: hidden;");
    expect(bar).not.toContain("overflow-x: auto");
    expect(css).toContain(".terminal-panel__bar > :not(.terminal-panel__title, .terminal-panel__tabs, .terminal-panel__spacer) { flex: none; }");
    expect(ruleBody(css, ".terminal-panel__title")).toContain("text-overflow: ellipsis;");
    const tabs = ruleBody(css, ".terminal-panel__tabs");
    expect(tabs).toContain("min-width: 0;");
    expect(tabs).toContain("overflow-x: auto;");
  });

  it("settings rows put the control below the label in a narrow dialog; the palette placeholder ends with an ellipsis", () => {
    const css = read("src/features/settings/settings.css");
    expect(ruleBody(css, ".settings-sections")).toContain("container-type: inline-size;");
    expect(css).toMatch(/@container \(width < 480px\) \{\s*\.settings-sections \.setting__row \{ flex-wrap: wrap;[^}]*\}\s*\.settings-sections \.setting__control \{ flex: 1 1 100%; \}/);
    expect(ruleBody(read("src/features/palette/palette.css"), ".palette__input")).toContain("text-overflow: ellipsis;");
  });
});

describe("narrow window: palette placeholder, notices, terminal bar", () => {
  it("the palette shows the short placeholder below 960 px (no ellipsis while the input has the focus)", async () => {
    setWidth(700);
    setupApp();
    await settleApp();
    act(() => useStore.getState().openPalette("all"));
    const input = () => screen.getByRole("dialog", { name: "Command palette" }).querySelector(".palette__input");
    await screen.findByRole("dialog", { name: "Command palette" });
    expect(input()?.getAttribute("placeholder")).toBe("Hosts, files, commands (> @ /)");
    act(() => useStore.getState().closePalette());
    setWidth(1200);
    act(() => {
      window.dispatchEvent(new Event("resize"));
      useStore.getState().openPalette("all");
    });
    await screen.findByRole("dialog", { name: "Command palette" });
    expect(input()?.getAttribute("placeholder")).toMatch(/^Hosts, files and commands\. Type >/);
  });

  it("a new notice scrolls the stack to the bottom (the newest)", () => {
    setupApp();
    const region = screen.getByRole("region", { name: "Notifications" });
    Object.defineProperty(region, "scrollHeight", { configurable: true, value: 640 });
    region.scrollTop = 0;
    act(() => useStore.getState().pushNotice({ code: "internal" }));
    expect(region.scrollTop).toBe(640);
    region.scrollTop = 0;
    act(() => useStore.getState().pushNotice({ code: "notConnected", params: { sessionId: "s" } }));
    expect(region.scrollTop).toBe(640);
  });

  it("the terminal panel title gives way before the tab strip", () => {
    const css = readFileSync("src/features/terminal/terminal.css", "utf8");
    expect(ruleBody(css, ".terminal-panel__title")).toContain("flex: 0 100 auto;");
    expect(ruleBody(css, ".terminal-panel__tabs")).toContain("flex: 0 1 auto;");
  });
});
