// Layout: Bookmarks header structure, sidebar limits, maximized terminal panel.
import { readFileSync } from "node:fs";
import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { setBackend } from "../../ipc";
import { ACTION_IDS, MENUS, shortcutLabel } from "../../lib/actionIds";
import { sshConfigTarget } from "../../lib/targets";
import { loadLayoutPrefs, resetLayoutPrefs, saveLayoutPrefs } from "../../state/layoutPrefs";
import { useStore } from "../../state/store";
import { setupApp } from "../../test/harness";
import { actionEnabled, runAction } from "../actions/registry";
import { openSession } from "../session/sessionActions";
import { disposeAllTerminals } from "../terminal/terminalRegistry";
import { ACTIVITY_BAR_PX, MAIN_MIN, SIDEBAR_MIN, clampSidebarWidth, sidebarMaxPx } from "./layoutLimits";
import { nativeMenuLabels } from "./nativeMenuLabels";
import { NATIVE_MENU } from "./shellEvents";

afterEach(() => {
  disposeAllTerminals();
  resetLayoutPrefs();
  setBackend(null);
});

async function connected() {
  const app = setupApp();
  const sessionId = await openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
  await waitFor(() => expect(useStore.getState().workspaces[sessionId]?.terminalTabs).toHaveLength(1), { timeout: 3000 });
  return { ...app, sessionId };
}

describe("Bookmarks panel header", () => {
  it("keeps '+' and 'more' in ONE right-aligned group (a third flex child would drift '+' to the middle)", () => {
    setupApp();
    const header = document.querySelector(".bookmarks-panel .panel-header");
    expect(header).not.toBeNull();
    // space-between: the title on the left, exactly one more child on the right.
    expect(header?.children).toHaveLength(2);
    const actions = header?.querySelector(".panel-header__actions");
    expect(actions?.parentElement).toBe(header);
    expect(within(actions as HTMLElement).getByRole("button", { name: "Add bookmark" })).toBeTruthy();
    expect(within(actions as HTMLElement).getByRole("button", { name: "More bookmark actions" })).toBeTruthy();
  });

  it("the stylesheet pushes the group to the end and does not let it shrink", () => {
    const css = readFileSync("src/features/hosts/hosts.css", "utf8");
    expect(css).toMatch(/\.panel-header__actions\s*\{[^}]*margin-left:\s*auto/);
    expect(css).toMatch(/\.panel-header__actions\s*\{[^}]*flex:\s*none/);
  });
});

describe("sidebar limits", () => {
  it("allows up to 80% of the window and keeps the main area usable", () => {
    expect(sidebarMaxPx(2000)).toBe(1600);
    // Mid-size window: 80% would leave less than MAIN_MIN, so the main area wins.
    expect(sidebarMaxPx(1000)).toBe(1000 - ACTIVITY_BAR_PX - MAIN_MIN);
    // Narrow window: the main area minimum wins over the 80% share.
    expect(sidebarMaxPx(600)).toBe(600 - ACTIVITY_BAR_PX - MAIN_MIN);
    // Tiny window: never below the sidebar minimum.
    expect(sidebarMaxPx(300)).toBe(SIDEBAR_MIN);
  });

  it("clamps a stored or dragged width to the current window", () => {
    expect(clampSidebarWidth(100, 1600)).toBe(SIDEBAR_MIN);
    expect(clampSidebarWidth(900, 1600)).toBe(900);
    expect(clampSidebarWidth(5000, 2000)).toBe(1600);
    // The window shrank: the width saved in a bigger window follows it.
    expect(clampSidebarWidth(1600, 800)).toBe(sidebarMaxPx(800));
    expect(clampSidebarWidth(Number.NaN, 1600)).toBe(SIDEBAR_MIN);
  });

  it("persists a wide sidebar (the stored value is no longer capped at 800 px)", () => {
    saveLayoutPrefs({ sidebarWidth: 1500 });
    expect(loadLayoutPrefs().sidebarWidth).toBe(1500);
  });
});

describe("maximized terminal panel", () => {
  it("is an action in the View menu, with no shortcut", () => {
    expect(ACTION_IDS).toContain("toggleMaximizePanel");
    expect(MENUS.view).toContain("toggleMaximizePanel");
    expect(shortcutLabel("toggleMaximizePanel", "windows")).toBeUndefined();
  });

  it("is mapped from the native macOS menu with a translated label", () => {
    expect(NATIVE_MENU["view.maximizePanel"]).toBe("toggleMaximizePanel");
    expect(nativeMenuLabels()["view.maximizePanel"]).toBe("Maximize terminal panel");
  });

  it("is disabled without a session", () => {
    setupApp();
    expect(actionEnabled("toggleMaximizePanel")).toBe(false);
  });

  it("toggles from the panel button and the action; editor tabs and unsaved buffers stay", async () => {
    const { user, sessionId } = await connected();
    const panel = await screen.findByRole("region", { name: "Terminal panel" });
    const state = () => useStore.getState().workspaces[sessionId];
    useStore.setState((s) => {
      const ws = s.workspaces[sessionId];
      if (ws) ws.editorTabs = ["buf-1"];
    });
    expect(state()?.panelMaximized).toBe(false);

    await user.click(within(panel).getByRole("button", { name: "Maximize panel" }));
    expect(state()?.panelMaximized).toBe(true);
    expect(state()?.editorTabs).toEqual(["buf-1"]); // hidden, never closed

    const restore = within(panel).getByRole("button", { name: "Restore panel" });
    expect(restore.getAttribute("aria-pressed")).toBe("true");
    await user.click(restore);
    expect(state()?.panelMaximized).toBe(false);

    act(() => runAction("toggleMaximizePanel"));
    expect(state()?.panelMaximized).toBe(true);
    act(() => runAction("toggleMaximizePanel"));
    expect(state()?.panelMaximized).toBe(false);
  });

  it("maximizing a hidden panel shows it; hiding or opening a file restores the editor", async () => {
    const { sessionId } = await connected();
    const state = () => useStore.getState().workspaces[sessionId];
    act(() => useStore.getState().setPanelOpen(sessionId, false));
    act(() => runAction("toggleMaximizePanel"));
    expect(state()).toMatchObject({ panelOpen: true, panelMaximized: true });

    act(() => useStore.getState().setPanelOpen(sessionId, false));
    expect(state()?.panelMaximized).toBe(false);

    act(() => useStore.getState().setPanelMaximized(sessionId, true));
    act(() => useStore.getState().openEditorTab(sessionId, "buf-2"));
    expect(state()?.panelMaximized).toBe(false);
  });

  it("is session-only: not part of the stored layout", () => {
    expect(Object.keys(loadLayoutPrefs())).not.toContain("panelMaximized");
  });
});
