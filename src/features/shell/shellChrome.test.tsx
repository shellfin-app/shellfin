import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import { matchShortcut } from "../../lib/keybindings";
import { setupApp, setupConnected, settleApp } from "../../test/harness";
import { useStore } from "../../state/store";
import { runAction } from "../actions/registry";
import { A } from "../../lib/actionIds";
import { installShellEvents, runNativeMenuItem } from "./shellEvents";
import { useMenuStore } from "../menu/menuStore";

afterEach(async () => {
  await settleApp();
  useMenuStore.getState().close();
  setBackend(null);
});

function menubar(): HTMLElement {
  return screen.getByRole("menubar", { name: "Application menu" });
}

describe("Title bar and menu bar", () => {
  it("one row: logo, six menus, tabs, search and window controls; the app name is not repeated", () => {
    setupApp();
    const names = within(menubar())
      .getAllByRole("menuitem")
      .map((b) => b.textContent);
    expect(names).toEqual(["File", "Edit", "View", "Connection", "Terminal", "Help"]);
    expect(screen.getByRole("button", { name: "Minimize" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Maximize" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Close" })).toBeTruthy();
    expect(document.querySelector(".title-bar")?.textContent).not.toContain("Shellfin");
  });

  it("window controls call the backend; the maximize button turns into Restore", async () => {
    const { user, backend } = setupApp();
    await installShellEvents();
    await user.click(screen.getByRole("button", { name: "Minimize" }));
    await user.click(screen.getByRole("button", { name: "Maximize" }));
    act(() => backend.emit("window:state", { maximized: true, fullscreen: false, focused: true }));
    expect(await screen.findByRole("button", { name: "Restore" })).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Close" }));
    expect(backend.windowCalls).toEqual(["minimize", "toggleMaximize", "close"]);
  });

  it("dragging the empty bar starts a window drag; a double click maximizes", () => {
    const { backend } = setupApp();
    const drag = document.querySelector(".title-bar__drag") as HTMLElement;
    fireEvent.mouseDown(drag, { button: 0, detail: 1 });
    fireEvent.mouseDown(drag, { button: 0, detail: 2 });
    fireEvent.mouseDown(screen.getByRole("button", { name: "Minimize" }), { button: 0, detail: 1 });
    expect(backend.windowCalls).toEqual(["startDragging", "toggleMaximize"]);
  });

  it("macOS: no window buttons and no in-app menu bar, the content is offset for the traffic lights", () => {
    setupApp();
    act(() => useStore.setState({ platform: "macos" }));
    expect(screen.queryByRole("menubar")).toBeNull();
    expect(screen.queryByRole("button", { name: "Minimize" })).toBeNull();
    expect(document.querySelector(".title-bar")?.className).toContain("title-bar--traffic-lights");
    act(() => useStore.getState().setWindowState({ maximized: false, fullscreen: true, focused: true }));
    expect(document.querySelector(".title-bar")?.className).not.toContain("title-bar--traffic-lights");
  });

  it("File opens a menu with shortcuts; an item runs its action; Escape closes", async () => {
    const { user } = setupApp();
    await user.click(within(menubar()).getByRole("menuitem", { name: "File" }));
    const menu = await screen.findByRole("menu", { name: "File" });
    expect(within(menu).getByRole("menuitem", { name: /New connection/ }).textContent).toContain("Ctrl+N");
    expect(within(menu).getByRole("menuitem", { name: /^Save(?! all)/ }).getAttribute("aria-disabled")).toBe("true");
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).toBeNull();

    await user.click(within(menubar()).getByRole("menuitem", { name: "File" }));
    await user.click(within(await screen.findByRole("menu", { name: "File" })).getByRole("menuitem", { name: /New connection/ }));
    expect(await screen.findByRole("dialog", { name: "New connection" })).toBeTruthy();
  });

  it("keyboard: F10 focuses the bar and opens the first menu, arrows switch menus, Alt+letter opens one", async () => {
    const { user } = setupApp();
    await user.keyboard("{F10}");
    expect(await screen.findByRole("menu", { name: "File" })).toBeTruthy();
    expect(document.activeElement?.getAttribute("role")).toBe("menuitem");
    await user.keyboard("{ArrowRight}");
    expect(await screen.findByRole("menu", { name: "Edit" })).toBeTruthy();
    await user.keyboard("{ArrowLeft}{ArrowLeft}");
    expect(await screen.findByRole("menu", { name: "Help" })).toBeTruthy();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).toBeNull();

    fireEvent.keyDown(document.body, { key: "v", altKey: true });
    expect(await screen.findByRole("menu", { name: "View" })).toBeTruthy();
  });

  it("View > Toggle status bar and Toggle sidebar are checkable and work", async () => {
    const { user } = setupApp();
    await user.click(within(menubar()).getByRole("menuitem", { name: "View" }));
    let item = await screen.findByRole("menuitemcheckbox", { name: /Toggle status bar/ });
    expect(item.getAttribute("aria-checked")).toBe("true");
    await user.click(item);
    await waitFor(() => expect(document.querySelector(".status-bar")).toBeNull());
    await user.click(within(menubar()).getByRole("menuitem", { name: "View" }));
    item = await screen.findByRole("menuitemcheckbox", { name: /Toggle status bar/ });
    expect(item.getAttribute("aria-checked")).toBe("false");
    await user.click(item);
    expect(document.querySelector(".status-bar")).not.toBeNull();

    runAction(A.toggleSidebar);
    expect(useStore.getState().sidebarOpen).toBe(false);
  });

  it("Help > Keyboard shortcuts opens the cheat sheet; About shows the version", async () => {
    const { user } = setupApp();
    runAction(A.keyboardShortcuts);
    const sheet = await screen.findByRole("dialog", { name: "Keyboard shortcuts" });
    expect(within(sheet).getByText("Toggle sidebar")).toBeTruthy();
    expect(within(sheet).getByText("Ctrl+B")).toBeTruthy();
    await user.click(within(sheet).getByRole("button", { name: "Close" }));
    runAction(A.about);
    const about = await screen.findByRole("dialog", { name: "About Shellfin" });
    expect(within(about).getByText("Version 0.1.0")).toBeTruthy();
  });

  it("Help > Documentation asks the backend to open the website (native confirmation)", async () => {
    const { backend } = setupApp();
    const open = vi.spyOn(backend, "appOpenExternal").mockResolvedValue(true);
    runAction(A.documentation);
    await waitFor(() => expect(open).toHaveBeenCalledWith("https://shellfin.app", expect.any(String), expect.any(String)));
  });

  it("macOS native menu events run the same actions; unknown ids are ignored", async () => {
    setupApp();
    runNativeMenuItem("view.toggleSidebar");
    expect(useStore.getState().sidebarOpen).toBe(false);
    runNativeMenuItem("no.such.item");
    runNativeMenuItem("help.shortcuts");
    expect(await screen.findByRole("dialog", { name: "Keyboard shortcuts" })).toBeTruthy();
    runNativeMenuItem("app.settings"); // a modal is open: the native menu does nothing
    expect(useStore.getState().dialog?.kind).toBe("shortcuts");
  });
});

describe("Sidebar views and layout", () => {
  it("the activity bar switches Bookmarks only / Files only / both", async () => {
    const { user } = await setupConnected();
    const bookmarks = screen.getByRole("button", { name: "Bookmarks" });
    const files = screen.getByRole("button", { name: "Files" });
    const both = screen.getByRole("button", { name: "Bookmarks and files" });
    expect(useStore.getState().activity).toBe("both");
    expect(both.getAttribute("aria-pressed")).toBe("true");
    expect(screen.getByRole("group", { name: "Hosts" })).toBeTruthy();
    expect(screen.getByRole("tree")).toBeTruthy();

    await user.click(files);
    expect(screen.queryByRole("group", { name: "Hosts" })).toBeNull();
    expect(screen.getByRole("tree")).toBeTruthy();
    await user.click(bookmarks);
    expect(screen.getByRole("group", { name: "Hosts" })).toBeTruthy();
    expect(screen.queryByRole("tree")).toBeNull();
    await user.click(both);
    expect(screen.getByRole("tree")).toBeTruthy();
  });

  it("a view button reopens a hidden sidebar; the choice is remembered", async () => {
    const { user } = await setupConnected();
    act(() => runAction(A.toggleSidebar));
    expect(document.querySelector(".sidebar")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Files" }));
    expect(document.querySelector(".sidebar")).not.toBeNull();
    expect(JSON.parse(window.localStorage.getItem("shellfin.layout.v1") ?? "{}")).toMatchObject({ activity: "files", sidebarOpen: true });
  });

  it("each pane of the split collapses to its header", async () => {
    await setupConnected();
    expect(screen.getByRole("button", { name: "Collapse Bookmarks" }).getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByRole("button", { name: "Collapse Files" })).toBeTruthy();
  });
});

describe("P1-1: tabs overflow", () => {
  it("the all-connections list shows every session and switches to the chosen one", async () => {
    const { user } = await setupConnected();
    await user.click(screen.getByRole("button", { name: "All connections" }));
    const menu = await screen.findByRole("menu");
    const names = within(menu)
      .getAllByRole("menuitemcheckbox")
      .map((i) => i.textContent);
    expect(names).toEqual(["Home", "prod-web-01"]);
    await user.click(within(menu).getByRole("menuitemcheckbox", { name: "Home" }));
    expect(useStore.getState().activeSessionId).toBeNull();
  });

  it("the search field is a button that opens the palette on click only", async () => {
    const { user } = setupApp();
    const search = screen.getByRole("button", { name: "Search hosts, files and commands" });
    search.focus();
    expect(useStore.getState().palette).toBeNull();
    await user.click(search);
    expect(useStore.getState().palette).toBe("all");
  });
});

describe("P1-5: hidden panels can always be brought back", () => {
  it("the status bar toggles the sidebar and the terminal panel; a collapsed strip shows the terminal count", async () => {
    const { user, sessionId } = await setupConnected();
    const terminal = screen.getByRole("button", { name: /Toggle terminal panel/ });
    expect(terminal.getAttribute("aria-pressed")).toBe("true");
    await user.click(terminal);
    expect(useStore.getState().workspaces[sessionId]?.panelOpen).toBe(false);
    const strip = await screen.findByRole("button", { name: /Show the terminal panel/ });
    expect(strip.textContent).toContain("Terminals (1)");
    await user.click(strip);
    expect(useStore.getState().workspaces[sessionId]?.panelOpen).toBe(true);

    await user.click(screen.getByRole("button", { name: /Toggle sidebar/ }));
    expect(useStore.getState().sidebarOpen).toBe(false);
    await user.click(screen.getByRole("button", { name: /Toggle sidebar/ }));
    expect(useStore.getState().sidebarOpen).toBe(true);
  });
});

describe("P1-4: bookmark rows", () => {
  it("single click selects, double click or Enter connects; every row has a status dot", async () => {
    const { user } = setupApp();
    const row = screen.getByRole("button", { name: "Connect to pi-home" });
    expect(within(row).getByRole("img", { name: "Not connected" })).toBeTruthy();
    await user.click(row);
    expect(row.getAttribute("aria-pressed")).toBe("true");
    expect(useStore.getState().sessionOrder).toHaveLength(0);
    row.focus();
    await user.keyboard("{Enter}");
    await waitFor(() => expect(useStore.getState().sessionOrder).toHaveLength(1));
  });

  it("arrow keys move through the list (one tab stop); Left on a row goes to its group", async () => {
    const { user } = setupApp();
    const rows = () => [...document.querySelectorAll<HTMLElement>("[data-host-nav]")];
    expect(rows().filter((r) => r.tabIndex === 0)).toHaveLength(1);
    rows()[0]?.focus();
    await user.keyboard("{ArrowDown}");
    expect(document.activeElement).toBe(rows()[1]);
    await user.keyboard("{End}");
    expect(document.activeElement).toBe(rows().at(-1));
    await user.keyboard("{Home}");
    expect(document.activeElement).toBe(rows()[0]);
    await user.keyboard("{ArrowLeft}"); // collapses the first group
    expect(rows()[0]?.getAttribute("aria-expanded")).toBe("false");
  });

  it("the context menu has Connect, Open in new tab, Edit, Duplicate, Copy ssh command, Copy host, Delete", async () => {
    const { user } = setupApp();
    fireEvent.contextMenu(screen.getByRole("button", { name: "Connect to pi-home" }));
    const menu = await screen.findByRole("menu");
    const names = within(menu)
      .getAllByRole("menuitem")
      .map((i) => i.textContent);
    expect(names).toEqual([
      "Connect",
      "Open in new tab",
      "Edit…F2",
      "Duplicate",
      "Move upAlt ↑",
      "Move downAlt ↓",
      "Move to group…",
      "Copy ssh command",
      "Copy host",
      "DeleteDel",
    ]);
    await user.click(within(menu).getByRole("menuitem", { name: "Duplicate" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Connect to pi-home (copy)" })).toBeTruthy());
  });

  it("Open in new tab opens a second session to the same target", async () => {
    const { user } = setupApp();
    fireEvent.contextMenu(screen.getByRole("button", { name: "Connect to pi-home" }));
    await user.click(await screen.findByRole("menuitem", { name: "Connect" }));
    await waitFor(() => expect(useStore.getState().sessionOrder).toHaveLength(1));
    fireEvent.contextMenu(screen.getByRole("button", { name: "Go to session pi-home" }));
    await user.click(await screen.findByRole("menuitem", { name: "Open another connection" }));
    await waitFor(() => expect(useStore.getState().sessionOrder).toHaveLength(2));
  });
});

describe("Session tab menu", () => {
  it("offers Reconnect, Disconnect, Duplicate, Save as bookmark, Copy user@host, Rename, Close", async () => {
    const { user, sessionId } = await setupConnected();
    fireEvent.contextMenu(document.querySelector(`[data-session-tab="${sessionId}"]`) as HTMLElement);
    const menu = await screen.findByRole("menu");
    const names = within(menu)
      .getAllByRole("menuitem")
      .map((i) => i.textContent);
    expect(names).toEqual(["Reconnect", "Disconnect", "Duplicate connection", "Save as bookmark…", "Copy user@host", "Rename", "Close", "Close others"]);
    expect(within(menu).getByRole("menuitem", { name: "Reconnect" }).getAttribute("aria-disabled")).toBe("true");
    await user.click(within(menu).getByRole("menuitem", { name: "Duplicate connection" }));
    await waitFor(() => expect(useStore.getState().sessionOrder).toHaveLength(2));
  });
});

describe("Go into folder", () => {
  it("re-roots the tree at the folder (and is absent on files and on the root)", async () => {
    const { user, sessionId } = await setupConnected();
    const root = useStore.getState().workspaces[sessionId]?.tree.root;
    fireEvent.contextMenu(await screen.findByRole("treeitem", { name: /nginx/ }));
    const menu = await screen.findByRole("menu");
    await user.click(within(menu).getByRole("menuitem", { name: "Go into folder" }));
    await waitFor(() => expect(useStore.getState().workspaces[sessionId]?.tree.root).toBe(`${root}/nginx`));

    fireEvent.contextMenu(document.querySelector(`[data-tree-path="${root}/nginx"]`) as HTMLElement);
    expect(within(await screen.findByRole("menu")).queryByRole("menuitem", { name: "Go into folder" })).toBeNull();
  });
});

describe("View > font size", () => {
  it("increase / decrease / reset change the terminal and editor font and save the settings", async () => {
    setupApp();
    const before = useStore.getState().settings?.terminal.fontSize ?? 13;
    runAction(A.zoomIn);
    await waitFor(() => expect(useStore.getState().settings?.terminal.fontSize).toBe(before + 1));
    expect(useStore.getState().settings?.editor.fontSize).toBe(before + 1);
    runAction(A.zoomOut);
    runAction(A.zoomOut);
    await waitFor(() => expect(useStore.getState().settings?.terminal.fontSize).toBe(before - 1));
    runAction(A.zoomReset);
    await waitFor(() => expect(useStore.getState().settings?.terminal.fontSize).toBe(13));
  });
});

describe("keyboard shortcuts", () => {
  const key = (init: Partial<KeyboardEvent> & { key: string }) => ({
    ctrlKey: false,
    metaKey: false,
    shiftKey: false,
    altKey: false,
    ...init,
  });

  const APP_KEYS: [string, Partial<KeyboardEvent> & { key: string }, string][] = [
    ["zoomIn", { key: "=", ctrlKey: true }, "zoomIn"],
    ["zoomOut", { key: "-", ctrlKey: true }, "zoomOut"],
    ["Ctrl+_", { key: "_", ctrlKey: true, shiftKey: true }, "zoomOut"],
    ["zoomReset", { key: "0", ctrlKey: true }, "zoomReset"],
    ["settings", { key: ",", ctrlKey: true }, "settings"],
    ["focusFiles", { key: "E", ctrlKey: true, shiftKey: true }, "focusFiles"],
    ["focusEditor", { key: "Y", ctrlKey: true, shiftKey: true }, "focusEditor"],
    ["focusTerminal", { key: "J", ctrlKey: true, shiftKey: true }, "focusTerminal"],
    ["F6", { key: "F6" }, "focusNextRegion"],
    ["Shift+F6", { key: "F6", shiftKey: true }, "focusPrevRegion"],
    ["F11", { key: "F11" }, "fullScreen"],
  ];

  it("zoom, settings, focus movement and full screen are app shortcuts outside the terminal", () => {
    for (const [, init, id] of APP_KEYS) expect(matchShortcut(key(init), "windows", false)).toBe(id);
    expect(matchShortcut(key({ key: "=", metaKey: true }), "macos", false)).toBe("zoomIn");
  });

  it("Inside a focused terminal those keys reach the shell (no shortcut match) on Windows/Linux", () => {
    for (const [name, init] of APP_KEYS) {
      expect(matchShortcut(key(init), "windows", true), name).toBeNull();
      expect(matchShortcut(key(init), "linux", true), name).toBeNull();
    }
  });

  it("Cmd chords reach no shell on macOS, so they stay app shortcuts in the terminal; F6/F11 do not", () => {
    expect(matchShortcut(key({ key: "=", metaKey: true }), "macos", true)).toBe("zoomIn");
    expect(matchShortcut(key({ key: ",", metaKey: true }), "macos", true)).toBe("settings");
    expect(matchShortcut(key({ key: "F6" }), "macos", true)).toBeNull();
    expect(matchShortcut(key({ key: "F11" }), "macos", true)).toBeNull();
    expect(matchShortcut(key({ key: "b", metaKey: true }), "macos", true)).toBeNull();
  });

  it("Ctrl+Shift+E in the app moves the focus into the file tree; F6 cycles the regions", async () => {
    const { user } = await setupConnected();
    // The terminal has the focus after connecting and owns Ctrl+Shift+E: move out of it first.
    if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
    await user.keyboard("{Control>}{Shift>}e{/Shift}{/Control}");
    expect(document.activeElement?.className).toContain("file-tree");
    await user.keyboard("{F6}");
    expect(document.activeElement?.closest(".sidebar")).toBeNull();
  });
});
