// The action registry: ONE definition per app action, used by the menu bar, the macOS native menu events, the
// keyboard shortcuts, the command palette and context menus. Ids and menu layout: `lib/actionIds.ts`.
import { getBackend } from "../../ipc";
import type { ActionId } from "../../lib/actionIds";
import { bookmarkDialog, goToFolderDialog, keysDialog, settingsDialog } from "../../lib/dialogs";
import { HELP_LINKS } from "../../lib/helpLinks";
import { guard } from "../../lib/reportError";
import { sshCommand } from "../../lib/sshCommand";
import { t } from "../../i18n/translate";
import type { FilesSortKey } from "../../state/layoutPrefs";
import type { RootState } from "../../state/slices/types";
import { useStore } from "../../state/store";
import { openErrorDetails } from "../errorReport/errorReport";
import { closeActiveBuffer, saveActiveBuffer, saveAllBuffers } from "../editor/editorActions";
import { isDirLike, lookupEntry } from "../files/fileActions";
import {
  canDisconnect,
  canReconnect,
  canSaveAsBookmark,
  disconnectSession,
  duplicateSession,
  reconnectSession,
  saveSessionAsBookmark,
} from "../session/connectionActions";
import { editConnection } from "../session/editConnection";
import { openBookmarksExport, startBookmarksImport } from "../hosts/bookmarksIo";
import { requestCloseSession } from "../session/sessionActions";
import { openTerminalTab, requestCloseTerminalTabs } from "../terminal/terminalActions";
import { loadedTerminalRegistry } from "../terminal/terminalLoader";
import { saveTerminalText } from "../terminal/saveOutput";
import { uploadFiles } from "../transfers/transferActions";
import { checkForUpdates } from "../updates/updateActions";
import { runEditCommand, type EditCommand } from "./editTarget";
import { cycleRegion, focusRegion, regionOf } from "./focusRegions";
import { zoomFonts } from "./zoom";

export interface ActionDef {
  run(): void | Promise<unknown>;
  /** Defaults to enabled. */
  enabled?(state: RootState): boolean;
  /** Present for toggles: the current state. */
  checked?(state: RootState): boolean;
  /** A short state text the menu shows next to the label (the sort direction of the checked sort). */
  detail?(state: RootState): string | undefined;
}

function activeId(s: RootState): string | null {
  return s.activeSessionId;
}

function workspace(s: RootState) {
  const id = activeId(s);
  return id ? s.workspaces[id] : undefined;
}

function connectedSession(s: RootState): string | null {
  const id = activeId(s);
  return id && s.sessionsById[id]?.state.phase === "connected" ? id : null;
}

function activeTerm(s: RootState): string | null {
  return workspace(s)?.activeTerminal ?? null;
}

const edit = (command: EditCommand): ActionDef => ({
  run: () => {
    runEditCommand(command);
  },
});

function openExternal(url: string): void {
  guard(getBackend().appOpenExternal(url, t("help.openTitle"), t("help.openBody", { url })));
}

/** Show or hide the bottom terminal panel of the active session; showing it focuses the terminal. */
export function toggleTerminalPanel(): void {
  const s = useStore.getState();
  const id = activeId(s);
  const ws = workspace(s);
  if (!id || !ws) return;
  if (ws.panelOpen) s.setPanelOpen(id, false);
  else {
    s.setPanelOpen(id, true);
    focusRegion("terminal");
  }
}

/**
 * Maximize the terminal panel over the editor area, or restore it. The editor is only hidden (its tabs and unsaved
 * buffers stay in the store); a hidden panel is shown first. Session-only state.
 */
export function toggleMaximizePanel(): void {
  const s = useStore.getState();
  const id = activeId(s);
  const ws = workspace(s);
  if (!id || !ws) return;
  if (ws.panelMaximized) {
    s.setPanelMaximized(id, false);
    return;
  }
  s.setPanelMaximized(id, true);
  focusRegion("terminal");
}

/**
 * Ctrl+` (VS Code behaviour): opens and focuses the panel; with the panel open but the focus elsewhere it moves
 * the focus into the terminal; with the focus already in the terminal it hides the panel again.
 */
export function toggleTerminalPanelFocusAware(): void {
  const s = useStore.getState();
  const ws = workspace(s);
  if (!ws) return;
  if (ws.panelOpen && regionOf(document.activeElement) !== "terminal") focusRegion("terminal");
  else toggleTerminalPanel();
}

function toggleBookmarksPane(): void {
  const s = useStore.getState();
  const hasSession = s.activeSessionId !== null;
  if (!s.sidebarOpen) {
    s.setSidebarOpen(true);
    return;
  }
  if (!hasSession) return; // only the bookmarks exist without a session
  // Both panes -> Files only; Files only or Bookmarks only -> both.
  s.setActivity(s.activity === "both" ? "files" : "both");
}

/**
 * File tree sort (header buttons and View menu, §8.11): a new key starts in its natural direction (name ascending,
 * size and date descending: largest and newest first); choosing the active key again flips the direction.
 */
export function chooseFilesSort(key: FilesSortKey): void {
  const s = useStore.getState();
  const current = s.filesSort;
  if (current.key === key) s.setFilesSort({ key, dir: current.dir === "asc" ? "desc" : "asc" });
  else s.setFilesSort({ key, dir: key === "name" ? "asc" : "desc" });
}

const sortFiles = (key: FilesSortKey): ActionDef => ({
  run: () => chooseFilesSort(key),
  checked: (s) => s.filesSort.key === key,
  detail: (s) => (s.filesSort.key !== key ? undefined : s.filesSort.dir === "asc" ? t("files.sort.ascending") : t("files.sort.descending")),
});

export const ACTIONS: Record<ActionId, ActionDef> = {
  // ---- File ----
  newConnection: { run: () => useStore.getState().openDialog(bookmarkDialog(null)) },
  // Import opens the native file dialog right away and shows a review dialog after the pick; export asks for the
  // format first.
  importBookmarks: { run: () => startBookmarksImport(), enabled: (s) => s.bookmarks?.readOnly !== true },
  exportBookmarks: { run: () => openBookmarksExport() },
  save: {
    run: () => {
      const id = activeId(useStore.getState());
      if (id) saveActiveBuffer(id);
    },
    enabled: (s) => workspace(s)?.activeBuffer != null,
  },
  saveAll: {
    run: () => {
      const id = activeId(useStore.getState());
      if (id) return saveAllBuffers({ sessionId: id, interactive: true });
      return undefined;
    },
    enabled: (s) => (workspace(s)?.editorTabs ?? []).some((b) => s.buffers[b]?.dirty === true),
  },
  closeTab: {
    run: () => {
      const id = activeId(useStore.getState());
      if (id) closeActiveBuffer(id);
    },
    enabled: (s) => workspace(s)?.activeBuffer != null,
  },
  closeConnection: {
    run: () => {
      const id = activeId(useStore.getState());
      if (id) requestCloseSession(id);
    },
    enabled: (s) => activeId(s) !== null,
  },
  settings: { run: () => useStore.getState().openDialog(settingsDialog()) },
  exit: { run: () => getBackend().windowClose() },

  // ---- Edit ----
  undo: edit("undo"),
  redo: edit("redo"),
  cut: edit("cut"),
  copy: edit("copy"),
  paste: edit("paste"),
  find: edit("find"),
  replace: edit("replace"),
  selectAll: edit("selectAll"),

  // ---- View ----
  toggleSidebar: {
    run: () => {
      const s = useStore.getState();
      s.setSidebarOpen(!s.sidebarOpen);
    },
    checked: (s) => s.sidebarOpen,
  },
  toggleTerminalPanel: {
    run: toggleTerminalPanel,
    enabled: (s) => workspace(s) !== undefined,
    checked: (s) => workspace(s)?.panelOpen === true,
  },
  toggleMaximizePanel: {
    run: toggleMaximizePanel,
    enabled: (s) => workspace(s) !== undefined,
    checked: (s) => workspace(s)?.panelMaximized === true,
  },
  toggleBookmarks: {
    run: toggleBookmarksPane,
    checked: (s) => s.sidebarOpen && (s.activity !== "files" || activeId(s) === null),
  },
  toggleStatusBar: {
    run: () => {
      const s = useStore.getState();
      s.setStatusBarOpen(!s.statusBarOpen);
    },
    checked: (s) => s.statusBarOpen,
  },
  sortFilesByName: sortFiles("name"),
  sortFilesBySize: sortFiles("size"),
  sortFilesByModified: sortFiles("mtime"),
  commandPalette: { run: () => useStore.getState().openPalette("all") },
  zoomIn: { run: () => zoomFonts(1) },
  zoomOut: { run: () => zoomFonts(-1) },
  zoomReset: { run: () => zoomFonts(0) },
  fullScreen: {
    run: () => getBackend().windowToggleFullscreen(),
    checked: (s) => s.windowState.fullscreen,
  },

  // ---- Connection ----
  reconnect: {
    run: () => {
      const id = activeId(useStore.getState());
      if (id) reconnectSession(id);
    },
    enabled: (s) => canReconnect(s, activeId(s)),
  },
  disconnect: {
    run: () => {
      const id = activeId(useStore.getState());
      if (id) disconnectSession(id);
    },
    enabled: (s) => canDisconnect(s, activeId(s)),
  },
  duplicateConnection: {
    run: () => {
      const id = activeId(useStore.getState());
      if (id) duplicateSession(id);
    },
    enabled: (s) => activeId(s) !== null,
  },
  saveAsBookmark: {
    run: () => {
      const id = activeId(useStore.getState());
      if (id) saveSessionAsBookmark(id);
    },
    enabled: (s) => canSaveAsBookmark(s, activeId(s)),
  },
  editConnection: {
    run: () => {
      const id = activeId(useStore.getState());
      if (id) editConnection(id);
    },
    enabled: (s) => activeId(s) !== null,
  },
  copySshCommand: {
    run: () => {
      const s = useStore.getState();
      const id = activeId(s);
      const target = id ? s.sessionsById[id]?.target : undefined;
      const command = target ? sshCommand(target, s.bookmarks?.bookmarks ?? [], s.sshConfig?.hosts ?? []) : null;
      if (command) navigator.clipboard.writeText(command).catch(() => undefined);
    },
    enabled: (s) => {
      const target = s.sessionsById[activeId(s) ?? ""]?.target;
      return target !== undefined && sshCommand(target, s.bookmarks?.bookmarks ?? [], s.sshConfig?.hosts ?? []) !== null;
    },
  },
  manageKeys: { run: () => useStore.getState().openDialog(keysDialog()) },
  goToFolder: {
    run: () => {
      const id = connectedSession(useStore.getState());
      if (id) useStore.getState().openDialog(goToFolderDialog(id));
    },
    enabled: (s) => connectedSession(s) !== null && workspace(s)?.tree.root != null,
  },
  uploadFiles: {
    run: () => {
      const s = useStore.getState();
      const id = connectedSession(s);
      const tree = id ? s.workspaces[id]?.tree : undefined;
      if (!id || !tree?.root) return undefined;
      // Into the selected folder (or next to the selected file's folder), else the tree root.
      const selected = tree.selection ? lookupEntry(tree, tree.selection) : undefined;
      const dir = selected && tree.selection && isDirLike(selected) ? tree.selection : tree.root;
      return uploadFiles(id, dir);
    },
    enabled: (s) => {
      const id = connectedSession(s);
      return id !== null && s.workspaces[id]?.tree.root != null && s.sessionsById[id]?.info?.sftpAvailable === true;
    },
  },
  showTransfers: {
    run: () => useStore.getState().setTransfersOpen(true),
    enabled: (s) => s.transfers.some((r) => r.sessionId === activeId(s)),
  },

  // ---- Terminal ----
  newTerminal: {
    run: () => {
      const id = connectedSession(useStore.getState());
      return id ? openTerminalTab(id) : undefined;
    },
    enabled: (s) => connectedSession(s) !== null,
  },
  closeTerminal: {
    run: () => {
      const s = useStore.getState();
      const id = activeId(s);
      const term = activeTerm(s);
      if (id && term) requestCloseTerminalTabs(id, [term]);
    },
    enabled: (s) => activeTerm(s) !== null,
  },
  renameTerminal: {
    run: () => {
      const s = useStore.getState();
      const id = activeId(s);
      const term = activeTerm(s);
      if (!id || !term) return;
      s.setPanelOpen(id, true);
      s.setRenamingTerminal(term);
    },
    enabled: (s) => activeTerm(s) !== null,
  },
  restartTerminal: {
    run: () => {
      const term = activeTerm(useStore.getState());
      const registry = loadedTerminalRegistry();
      return term && registry ? registry.restartTerminalById(term) : undefined;
    },
    enabled: (s) => {
      const term = activeTerm(s);
      return term !== null && s.terminals[term]?.status === "exited";
    },
  },
  clearTerminal: {
    run: () => {
      const term = activeTerm(useStore.getState());
      if (term) loadedTerminalRegistry()?.clearTerminal(term);
    },
    enabled: (s) => activeTerm(s) !== null,
  },
  findInTerminal: {
    run: () => {
      const s = useStore.getState();
      const id = activeId(s);
      const term = activeTerm(s);
      if (!id || !term) return;
      s.setPanelOpen(id, true);
      s.setTerminalSearch(term);
    },
    enabled: (s) => activeTerm(s) !== null,
  },
  saveTerminalOutput: {
    run: () => {
      const term = activeTerm(useStore.getState());
      const text = term ? loadedTerminalRegistry()?.terminalText(term) : undefined;
      return text === undefined ? undefined : saveTerminalText(text);
    },
    enabled: (s) => activeTerm(s) !== null,
  },

  // ---- Help ----
  keyboardShortcuts: { run: () => useStore.getState().openDialog({ kind: "shortcuts" }) },
  documentation: { run: () => openExternal(HELP_LINKS.documentation) },
  // The error details dialog without an error: environment only (and the log on request), then "Report on GitHub…".
  reportProblem: { run: () => openErrorDetails(null) },
  securityPolicy: { run: () => openExternal(HELP_LINKS.securityPolicy) },
  openLogs: { run: () => getBackend().appOpenLogsDir() },
  checkForUpdates: { run: () => checkForUpdates() },
  about: { run: () => useStore.getState().openDialog({ kind: "about" }) },

  // ---- Focus ----
  focusFiles: { run: () => void focusRegion("sidebar") },
  focusEditor: { run: () => void focusRegion("editor"), enabled: (s) => workspace(s)?.activeBuffer != null },
  focusTerminal: { run: () => void focusRegion("terminal"), enabled: (s) => workspace(s)?.activeTerminal != null },
  focusNextRegion: { run: () => cycleRegion(1) },
  focusPrevRegion: { run: () => cycleRegion(-1) },
};

export function actionEnabled(id: ActionId, state: RootState = useStore.getState()): boolean {
  return ACTIONS[id].enabled?.(state) ?? true;
}

/** Runs an action by id (disabled actions are ignored). Failures are reported as notices. */
export function runAction(id: ActionId): void {
  if (!actionEnabled(id)) return;
  const result = ACTIONS[id].run();
  if (result) guard(result);
}

export function actionLabel(id: ActionId): string {
  return t(`action.${id}`);
}
