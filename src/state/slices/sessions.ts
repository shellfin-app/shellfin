import type { BookmarkColor, HopRef, RecentTarget, SessionEvent, SessionInfo, SessionState } from "../../ipc/types";
import { assertNever } from "../../lib/assertNever";
import type { SliceCreator } from "./types";
import type { DialogState } from "./ui";

export interface SessionStats {
  txBytesPerSec: number;
  rxBytesPerSec: number;
}

export interface BannerEntry {
  hop: HopRef;
  /** Server text: render only through displayServerText (§9.5). */
  text: string;
}

/** NEVER holds a `Target` with a password: a draft password lives only in the dialog's local state (§8.3). */
export interface SessionEntry {
  target: RecentTarget;
  label: string;
  /** User-chosen tab name (display only; `label` stays the host's identity). */
  tabName?: string;
  state: SessionState;
  /** Last `connected` info; kept while reconnecting. */
  info?: SessionInfo;
  banner?: BannerEntry;
  /** Test-connection session: not shown as a tab, never activated, no workspace. */
  probe: boolean;
  /** Bookmark color: from the bookmark at open, then `info.color` and `bookmarkUpdated`. */
  color: BookmarkColor | null;
  /** The saved bookmark differs from what this connection was made with (ARCHITECTURE §7.6): offer "Reconnect to apply". */
  needsReconnect: boolean;
  /** When the session was opened (elapsed time on the connecting card). */
  startedAtMs: number;
  /** When the connection was lost (first `reconnecting` after `connected`), for the overlay. */
  lostAtMs: number | null;
  /** When the current `reconnecting` stage started (drives the countdown bar). */
  stageSinceMs: number | null;
}

function sameStats(a: SessionStats | undefined, b: SessionStats): boolean {
  return a !== undefined && a.txBytesPerSec === b.txBytesPerSec && a.rxBytesPerSec === b.rxBytesPerSec;
}

/** Does the open dialog act on the session `sessionId` (directly, or through a buffer that is already gone)? */
function dialogBelongsTo(dialog: DialogState, sessionId: string, buffers: Record<string, unknown>): boolean {
  switch (dialog.kind) {
    case "confirmCloseTerminals":
    case "deleteEntry":
    case "moveEntry":
    case "goToFolder":
    case "replaceTransfers":
    case "closeSession":
      return dialog.sessionId === sessionId;
    case "conflict":
    case "encoding":
    case "closeBuffer":
      return buffers[dialog.bufferId] === undefined;
    default:
      return false;
  }
}

export interface SessionsSlice {
  sessionsById: Record<string, SessionEntry>;
  /**
   * Live transfer rates (a `stats` event per session per second). Kept apart from `sessionsById` so the tab strip,
   * the bookmarks panel and every other `sessionsById` subscriber do not re-render once a second.
   */
  sessionStats: Record<string, SessionStats>;
  sessionOrder: string[];
  /** `null` = the Home (welcome) tab is active. */
  activeSessionId: string | null;
  addSession(id: string, target: RecentTarget, label: string, probe?: boolean): void;
  dismissBanner(id: string): void;
  /** Renames the connection tab (`null` / empty = back to the host label). */
  renameSession(id: string, name: string | null): void;
  removeSession(id: string): void;
  /** Moves a session tab to `index` in the strip (a reopened connection keeps its place). */
  placeSession(id: string, index: number): void;
  setActiveSession(id: string | null): void;
  applySessionEvent(id: string, event: SessionEvent): void;
}

export const createSessionsSlice: SliceCreator<SessionsSlice> = (set) => ({
  sessionsById: {},
  sessionStats: {},
  sessionOrder: [],
  activeSessionId: null,

  addSession: (id, target, label, probe = false) =>
    set((s) => {
      s.sessionsById[id] = {
        target,
        label,
        state: { phase: "resolving" },
        probe,
        color: target.kind === "bookmark" ? (s.bookmarks?.bookmarks.find((b) => b.id === target.id)?.color ?? null) : null,
        needsReconnect: false,
        startedAtMs: Date.now(),
        lostAtMs: null,
        stageSinceMs: null,
      };
      if (!probe) {
        s.sessionOrder.push(id);
        s.activeSessionId = id;
      }
    }),

  removeSession: (id) =>
    set((s) => {
      delete s.sessionsById[id];
      delete s.sessionStats[id];
      const index = s.sessionOrder.indexOf(id);
      s.sessionOrder = s.sessionOrder.filter((x) => x !== id);
      const ws = s.workspaces[id];
      if (ws) {
        for (const bufferId of ws.editorTabs) {
          delete s.buffers[bufferId];
          delete s.cursors[bufferId];
        }
        for (const termId of ws.terminalTabs) {
          delete s.terminals[termId];
          if (s.terminalSearch === termId) s.terminalSearch = null;
        }
      }
      delete s.workspaces[id];
      delete s.prompts[id];
      s.transfers = s.transfers.filter((t) => t.sessionId !== id);
      // Anything in the UI that still points at the removed session goes with it.
      if (s.promptSession === id) s.promptSession = null;
      if (s.renamingSession === id) s.renamingSession = null;
      if (s.renamingTerminal !== null && s.terminals[s.renamingTerminal] === undefined) s.renamingTerminal = null;
      if (s.activeSessionId === id) s.dropTarget = null;
      const dialog = s.dialog;
      if (dialog !== null && dialogBelongsTo(dialog, id, s.buffers)) s.dialog = null;
      if (s.activeSessionId === id) {
        s.activeSessionId = s.sessionOrder[Math.min(index, s.sessionOrder.length - 1)] ?? null;
      }
    }),

  placeSession: (id, index) =>
    set((s) => {
      const from = s.sessionOrder.indexOf(id);
      if (from < 0) return;
      s.sessionOrder.splice(from, 1);
      s.sessionOrder.splice(Math.min(index, s.sessionOrder.length), 0, id);
    }),

  renameSession: (id, name) =>
    set((s) => {
      const e = s.sessionsById[id];
      if (!e) return;
      if (name === null || name.trim() === "") delete e.tabName;
      else e.tabName = name.trim().slice(0, 64);
    }),

  dismissBanner: (id) =>
    set((s) => {
      const e = s.sessionsById[id];
      if (e) delete e.banner;
    }),

  setActiveSession: (id) =>
    set((s) => {
      s.activeSessionId = id;
    }),

  applySessionEvent: (id, event) =>
    set((s) => {
      const entry = s.sessionsById[id];
      if (!entry) return;
      switch (event.type) {
        case "state": {
          const prev = entry.state;
          entry.state = event.state;
          if (event.state.phase === "reconnecting") {
            if (prev.phase !== "reconnecting" || prev.stage !== event.state.stage) entry.stageSinceMs = Date.now();
            if (prev.phase !== "reconnecting") entry.lostAtMs = Date.now();
          } else {
            entry.stageSinceMs = null;
            if (event.state.phase === "connected") entry.lostAtMs = null;
          }
          if (event.state.phase === "connected") {
            entry.info = event.state.info;
            entry.color = event.state.info.color;
            entry.label = event.state.info.label;
          }
          break;
        }
        case "prompt":
          s.prompts[id] = event.prompt;
          break;
        case "promptClosed":
          if (s.prompts[id]?.id === event.promptId) delete s.prompts[id];
          break;
        case "banner":
          entry.banner = { hop: event.hop, text: event.text };
          break;
        case "stats":
          if (!sameStats(s.sessionStats[id], event)) {
            s.sessionStats[id] = { txBytesPerSec: event.txBytesPerSec, rxBytesPerSec: event.rxBytesPerSec };
          }
          break;
        case "bookmarkUpdated":
          // The connection itself never changes; the tab name, color and (live) bandwidth limits do.
          entry.label = event.label;
          entry.color = event.color;
          entry.needsReconnect = event.needsReconnect;
          if (entry.info) entry.info = { ...entry.info, limitOverrides: event.limitOverrides, color: event.color };
          break;
        case "warning":
          // Surfaced as a notice by sessionActions (needs the notices id counter).
          break;
        default:
          assertNever(event);
      }
    }),
});
