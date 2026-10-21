// Backend -> shell events of the custom title bar: the macOS native menu (`menu:action`, ARCHITECTURE §4.2) and the
// native window state (`window:state`; the state at load comes with `AppBootstrap.window`).
import { getBackend } from "../../ipc";
import type { ActionId } from "../../lib/actionIds";
import { useStore } from "../../state/store";
import { actionEnabled, runAction } from "../actions/registry";
import { cycleBuffer } from "../editor/editorActions";
import { isModalOpen } from "../session/promptVisibility";

/** The ids the backend's macOS menu sends, mapped to the action registry. Unknown ids are ignored. */
export const NATIVE_MENU: Record<string, ActionId | (() => void)> = {
  "app.settings": "settings",
  "connection.new": "newConnection",
  "file.upload": "uploadFiles",
  "file.importBookmarks": "importBookmarks",
  "file.exportBookmarks": "exportBookmarks",
  "file.save": "save",
  "file.saveAll": "saveAll",
  "file.closeTab": "closeTab",
  "connection.close": "closeConnection",
  "edit.find": "find", // editor find or terminal find, by focus
  "view.commandPalette": "commandPalette",
  "view.toggleSidebar": "toggleSidebar",
  "view.toggleTerminal": "toggleTerminalPanel",
  "view.maximizePanel": "toggleMaximizePanel",
  "view.focusFiles": "focusFiles",
  "view.zoomIn": "zoomIn",
  "view.zoomOut": "zoomOut",
  "view.zoomReset": "zoomReset",
  "connection.reconnect": "reconnect",
  "connection.disconnect": "disconnect",
  "connection.edit": "editConnection",
  "connection.saveAsBookmark": "saveAsBookmark",
  "connection.copyTarget": "copySshCommand",
  "connection.keys": "manageKeys",
  "terminal.new": "newTerminal",
  "terminal.restart": "restartTerminal",
  "terminal.clear": "clearTerminal",
  "terminal.saveOutput": "saveTerminalOutput",
  "window.nextTab": () => cycleActiveBuffer(1),
  "window.prevTab": () => cycleActiveBuffer(-1),
  "help.shortcuts": "keyboardShortcuts",
  "help.openLogs": "openLogs",
  "help.website": "documentation",
  "help.checkForUpdates": "checkForUpdates", // macOS: in the app menu (convention), Help elsewhere
};

function cycleActiveBuffer(delta: 1 | -1): void {
  const id = useStore.getState().activeSessionId;
  if (id && useStore.getState().workspaces[id]?.activeBuffer) cycleBuffer(id, delta);
}

/** Runs the native menu item `id`. Items without a context (no session, no editor ...) do nothing. */
export function runNativeMenuItem(id: string): void {
  // The native menu cannot know about our dialogs: a modal owns the keyboard and the commands.
  if (isModalOpen(useStore.getState())) return;
  const entry = NATIVE_MENU[id];
  if (typeof entry === "function") entry();
  else if (entry !== undefined && actionEnabled(entry)) runAction(entry);
}

export async function installShellEvents(): Promise<void> {
  const backend = getBackend();
  await Promise.all([
    backend.listen("menu:action", ({ id }) => runNativeMenuItem(id)),
    backend.listen("window:state", (state) => useStore.getState().setWindowState(state)),
  ]);
}
