import type { AppError, BookmarksExportResult, BookmarksImportPreview, BookmarksImportResult, WindowState } from "../../ipc/types";
import { loadLayoutPrefs } from "../layoutPrefs";
import type { SliceCreator } from "./types";

/** Sidebar views: Bookmarks only, Files only, or both panes. Keys, Settings and Search open dialogs. */
export type ActivityView = "bookmarks" | "files" | "both";

/** Outcome shown by the bookmark import / export summary dialog. */
export type BookmarksIoResult =
  | { mode: "import"; data: BookmarksImportResult }
  | { mode: "export"; data: BookmarksExportResult };

/** Modal dialogs driven by the store (prompts are driven by the prompts slice, not by this). */
export type DialogState =
  /** Bookmark export options (format), then the native Save dialog. */
  | { kind: "bookmarksIo" }
  /** Bookmark import review after the file was picked: nothing is saved until the user confirms (`token` = pending import). */
  | { kind: "bookmarksImport"; preview: BookmarksImportPreview; token: string }
  | { kind: "bookmarksIoResult"; result: BookmarksIoResult }
  /**
   * `prefill`: a `[user@]host[:port]` spec for a new bookmark (quick connect -> "Save as bookmark", "Edit connection").
   * `sshConfig`: the spec is an ssh-config alias, so `connection.defaultUser` must not override the config's `User`.
   */
  | {
      kind: "bookmark";
      bookmarkId: string | null;
      prefill?: { spec: string; name?: string; sshConfig?: boolean };
      replaceSession?: string;
    }
  | { kind: "confirmDeleteBookmark"; bookmarkId: string }
  /** "Confirm before connecting" (bookmark setting); `name` is the bookmark's name. */
  | { kind: "confirmConnect"; name: string }
  /** "Move to group…": keyboard/menu alternative to dragging a bookmark into another group. */
  | { kind: "moveBookmark"; bookmarkId: string }
  /** "Delete group…": confirm (empty group) or choose where its bookmarks go. */
  | { kind: "deleteGroup"; groupId: string }
  | { kind: "settings" }
  | { kind: "keys" }
  /** The ssh config load warnings (Welcome > Config > "N warnings"); the list comes from the live snapshot. */
  | { kind: "configWarnings" }
  /** Multi-line paste into a terminal without bracketed-paste mode (§8.6); `preview` is already escaped. */
  | { kind: "confirmPaste"; lines: number; preview: string }
  /** The server file changed since it was opened: overwrite / reload / cancel (§6.4). */
  | { kind: "conflict"; bufferId: string }
  /** Reopen / save a buffer with another encoding (§6.3). */
  | { kind: "encoding"; bufferId: string }
  /** Closing a tab with unsaved changes. */
  | { kind: "closeBuffer"; bufferId: string }
  /** Closing a connection that has tabs with unsaved changes. */
  | { kind: "closeSession"; sessionId: string; reopen?: boolean }
  /** Closing terminal tabs (`termIds`) or the whole connection (no `termIds`) while shells are running. */
  | { kind: "confirmCloseTerminals"; sessionId: string; termIds?: string[] }
  | { kind: "deleteEntry"; sessionId: string; path: string; entry: "file" | "dir" }
  /** Move to another directory (HTML5 drag-to-move does not work on Windows, so this replaces it). */
  | { kind: "moveEntry"; sessionId: string; path: string }
  /** Re-root the file tree at any absolute path ("Go to folder…"). */
  | { kind: "goToFolder"; sessionId: string }
  /** Uploads that hit an existing remote file: Replace? -> `transfer_retry(overwrite: true)`. */
  | { kind: "replaceTransfers"; sessionId: string }
  /** Help > Keyboard shortcuts (cheat sheet) and Help > About. */
  | { kind: "shortcuts" }
  | { kind: "about" }
  /** Updater: the update offer / progress / failure (`UpdatesSlice.update`). */
  | { kind: "update" }
  | { kind: "quit" };

export type PaletteMode = "all" | "files";

/** Informational toasts (not errors). The texts live in `notices.info.<kind>`. */
export type NoticeInfo =
  | { kind: "foldersUnsupported" }
  | { kind: "dropNoTarget" }
  | { kind: "mixedEol"; name: string }
  | { kind: "copied" }
  /** "Save output…" result (terminal buffer saved to a local file). */
  | { kind: "outputSaved"; path: string }
  | { kind: "outputCancelled" }
  | { kind: "outputTooLarge" }
  /** A global shortcut was typed into the focused terminal and went to the shell (one-time hint). */
  | { kind: "shortcutSwallowed"; combo: string }
  /** Updater: a manual check found nothing newer. */
  | { kind: "upToDate" }
  /** Updater: an automatic check found an update while another dialog was open. */
  | { kind: "updateAvailable"; version: string };

/** Toast severity: drives the colour, the icon and the hidden level word (colour is never the only signal). */
export type NoticeLevel = "info" | "success" | "warning" | "error";

/** The level of each informational toast. */
export function infoLevel(info: NoticeInfo): NoticeLevel {
  switch (info.kind) {
    case "copied":
    case "outputSaved":
    case "upToDate":
      return "success";
    case "updateAvailable":
      return "info";
    case "foldersUnsupported":
    case "mixedEol":
      return "warning";
    case "outputTooLarge":
      return "error";
    case "dropNoTarget":
    case "outputCancelled":
    case "shortcutSwallowed":
      return "info";
  }
}

/** A user-visible message: an error (rendered through `errors.<code>`) or an info toast. */
export interface Notice {
  id: number;
  level: NoticeLevel;
  error?: AppError;
  info?: NoticeInfo;
}

const MAX_NOTICES = 5;
let nextNoticeId = 1;

export interface UiSlice {
  activity: ActivityView;
  sidebarOpen: boolean;
  /** Status bar visible (View > Toggle status bar). */
  statusBarOpen: boolean;
  /** Native window state, fed by the backend (`window:state`). */
  windowState: WindowState;
  /** Command palette: `null` = closed. */
  palette: PaletteMode | null;
  dialog: DialogState | null;
  /** Bumped when the quit dialog must grab focus again (repeated `app:quit-requested`, §8.9). */
  quitFocusTick: number;
  /** A non-active session (or a test-connection probe) whose pending prompt the user chose to answer now. */
  promptSession: string | null;
  /** The connection tab whose name is being edited. */
  renamingSession: string | null;
  notices: Notice[];
  /** The transfers popover in the status bar. */
  transfersOpen: boolean;
  /** Remote directory highlighted as the target of an OS file drag (`drop:hover`). */
  dropTarget: string | null;
  setTransfersOpen(open: boolean): void;
  setDropTarget(dir: string | null): void;
  setActivity(view: ActivityView): void;
  setSidebarOpen(open: boolean): void;
  setStatusBarOpen(open: boolean): void;
  setWindowState(state: WindowState): void;
  openPalette(mode?: PaletteMode): void;
  closePalette(): void;
  openDialog(dialog: DialogState): void;
  /** Closes the open dialog; with `kind`, only if THAT kind is still open (a late close must not hit a newer dialog). */
  closeDialog(kind?: DialogState["kind"]): void;
  bumpQuitFocus(): void;
  showPromptOf(sessionId: string | null): void;
  setRenamingSession(sessionId: string | null): void;
  /** `level` defaults to "error"; non-fatal warnings (config warnings, session warnings) pass "warning". */
  pushNotice(error: AppError, level?: NoticeLevel): void;
  pushInfo(info: NoticeInfo): void;
  dismissNotice(id: number): void;
}

export const createUiSlice: SliceCreator<UiSlice> = (set) => ({
  activity: loadLayoutPrefs().activity,
  sidebarOpen: loadLayoutPrefs().sidebarOpen,
  statusBarOpen: loadLayoutPrefs().statusBarOpen,
  windowState: { maximized: false, fullscreen: false, focused: true },
  palette: null,
  dialog: null,
  quitFocusTick: 0,
  promptSession: null,
  renamingSession: null,
  notices: [],
  transfersOpen: false,
  dropTarget: null,
  setTransfersOpen: (open) =>
    set((s) => {
      s.transfersOpen = open;
    }),
  setDropTarget: (dir) =>
    set((s) => {
      s.dropTarget = dir;
    }),
  setActivity: (view) =>
    set((s) => {
      s.activity = view;
    }),
  setSidebarOpen: (open) =>
    set((s) => {
      s.sidebarOpen = open;
    }),
  setStatusBarOpen: (open) =>
    set((s) => {
      s.statusBarOpen = open;
    }),
  setWindowState: (state) =>
    set((s) => {
      s.windowState = state;
    }),
  // While the quit confirmation is open nothing may replace it or open on top of it (§8.9): otherwise the
  // backend (waiting in `confirming`) could never be answered and the window could not be closed.
  openPalette: (mode = "all") =>
    set((s) => {
      if (s.dialog?.kind === "quit") return;
      s.palette = mode;
    }),
  closePalette: () =>
    set((s) => {
      s.palette = null;
    }),
  openDialog: (dialog) =>
    set((s) => {
      if (s.dialog?.kind === "quit" && dialog.kind !== "quit") return;
      s.dialog = dialog;
      if (dialog.kind === "quit") s.palette = null;
    }),
  closeDialog: (kind) =>
    set((s) => {
      if (kind !== undefined && s.dialog?.kind !== kind) return;
      s.dialog = null;
    }),
  bumpQuitFocus: () =>
    set((s) => {
      s.quitFocusTick += 1;
    }),
  showPromptOf: (sessionId) =>
    set((s) => {
      s.promptSession = sessionId;
    }),
  setRenamingSession: (sessionId) =>
    set((s) => {
      s.renamingSession = sessionId;
    }),
  pushNotice: (error, level = "error") =>
    set((s) => {
      // Identical errors are shown once (a retry loop must not stack toasts).
      const key = JSON.stringify(error);
      if (s.notices.some((n) => n.error !== undefined && JSON.stringify(n.error) === key)) return;
      s.notices.push({ id: nextNoticeId++, level, error });
      if (s.notices.length > MAX_NOTICES) s.notices.splice(0, s.notices.length - MAX_NOTICES);
    }),
  pushInfo: (info) =>
    set((s) => {
      const key = JSON.stringify(info);
      if (s.notices.some((n) => n.info !== undefined && JSON.stringify(n.info) === key)) return;
      s.notices.push({ id: nextNoticeId++, level: infoLevel(info), info });
      if (s.notices.length > MAX_NOTICES) s.notices.splice(0, s.notices.length - MAX_NOTICES);
    }),
  dismissNotice: (id) =>
    set((s) => {
      s.notices = s.notices.filter((n) => n.id !== id);
    }),
});
