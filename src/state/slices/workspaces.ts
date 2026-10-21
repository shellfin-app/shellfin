import type { DirListing } from "../../ipc/types";
import { sameListing } from "../../lib/sameListing";
import type { SliceCreator } from "./types";

/** An unchanged listing re-fetched within this time keeps its old `fetchedAtMs` (no store churn). */
const LISTING_RENEW_MS = 15_000;

export interface CachedListing {
  listing: DirListing;
  fetchedAtMs: number;
  stale: boolean;
}

/** Inline name editing in the tree: rename an entry, or name a new file/folder inside `parent`. */
export type TreeEdit = { kind: "rename"; path: string } | { kind: "new"; parent: string; entry: "file" | "dir" };

export interface TreeState {
  root: string | null;
  expanded: Record<string, true>;
  listings: Record<string, CachedListing>;
  selection: string | null;
  edit: TreeEdit | null;
}

/** Is `path` the same as `base` or inside it? */
export function isUnder(path: string, base: string): boolean {
  return path === base || path.startsWith(base.endsWith("/") ? base : `${base}/`);
}

/** Per-session UI workspace (§8.3). */
export interface Workspace {
  tree: TreeState;
  editorTabs: string[];
  activeBuffer: string | null;
  terminalTabs: string[];
  activeTerminal: string | null;
  panelOpen: boolean;
  /** The terminal panel covers the editor area (the editor is hidden, not closed). Session-only, never persisted. */
  panelMaximized: boolean;
  /** Whether the root listing worked. */
  filesStatus: "ok" | "failed";
}

export interface WorkspacesSlice {
  workspaces: Record<string, Workspace>;
  ensureWorkspace(sessionId: string): void;
  setTreeRoot(sessionId: string, root: string): void;
  toggleExpanded(sessionId: string, path: string): void;
  setListing(sessionId: string, path: string, listing: DirListing, fetchedAtMs: number): void;
  selectTreeEntry(sessionId: string, path: string | null): void;
  setTreeEdit(sessionId: string, edit: TreeEdit | null): void;
  /** Marks every cached listing stale (the connection was lost; they are refreshed after the reconnect). */
  markListingsStale(sessionId: string): void;
  /** Forgets the cache and expansion state of `path` and everything below it. */
  forgetTreePath(sessionId: string, path: string): void;
  /** `from` was renamed/moved to `to`: re-keys expansion and selection, drops the stale listings below `from`. */
  moveTreePath(sessionId: string, from: string, to: string): void;
  openEditorTab(sessionId: string, bufferId: string): void;
  closeEditorTab(sessionId: string, bufferId: string): void;
  setActiveBuffer(sessionId: string, bufferId: string | null): void;
  addTerminalTab(sessionId: string, termId: string): void;
  removeTerminalTab(sessionId: string, termId: string): void;
  setActiveTerminal(sessionId: string, termId: string | null): void;
  setPanelOpen(sessionId: string, open: boolean): void;
  /** Maximizing also opens the panel; hiding the panel restores the editor. */
  setPanelMaximized(sessionId: string, maximized: boolean): void;
  setFilesStatus(sessionId: string, status: Workspace["filesStatus"]): void;
}

export function emptyWorkspace(panelOpen = true): Workspace {
  return {
    tree: { root: null, expanded: {}, listings: {}, selection: null, edit: null },
    editorTabs: [],
    activeBuffer: null,
    terminalTabs: [],
    activeTerminal: null,
    panelOpen,
    panelMaximized: false,
    filesStatus: "ok",
  };
}

export const createWorkspacesSlice: SliceCreator<WorkspacesSlice> = (set) => ({
  workspaces: {},

  ensureWorkspace: (sessionId) =>
    set((s) => {
      if (!s.workspaces[sessionId]) {
        s.workspaces[sessionId] = emptyWorkspace(s.settings?.ui.terminalPanelOpen ?? true);
      }
    }),

  setTreeRoot: (sessionId, root) =>
    set((s) => {
      const ws = s.workspaces[sessionId];
      if (!ws) return;
      ws.tree.root = root;
      ws.tree.expanded[root] = true;
    }),

  toggleExpanded: (sessionId, path) =>
    set((s) => {
      const tree = s.workspaces[sessionId]?.tree;
      if (!tree) return;
      if (tree.expanded[path]) delete tree.expanded[path];
      else tree.expanded[path] = true;
    }),

  setListing: (sessionId, path, listing, fetchedAtMs) =>
    set((s) => {
      const tree = s.workspaces[sessionId]?.tree;
      if (!tree) return;
      const cached = tree.listings[path];
      // An unchanged listing keeps its object (sort cache, row identity); only the age is renewed, and only
      // when it matters. A stale entry is always replaced so the flag clears.
      if (cached && !cached.stale && sameListing(cached.listing, listing)) {
        if (fetchedAtMs - cached.fetchedAtMs >= LISTING_RENEW_MS) cached.fetchedAtMs = fetchedAtMs;
        return;
      }
      tree.listings[path] = { listing, fetchedAtMs, stale: false };
    }),

  selectTreeEntry: (sessionId, path) =>
    set((s) => {
      const tree = s.workspaces[sessionId]?.tree;
      if (tree) tree.selection = path;
    }),

  setTreeEdit: (sessionId, edit) =>
    set((s) => {
      const tree = s.workspaces[sessionId]?.tree;
      if (!tree) return;
      tree.edit = edit;
      if (edit?.kind === "new") tree.expanded[edit.parent] = true;
    }),

  markListingsStale: (sessionId) =>
    set((s) => {
      const tree = s.workspaces[sessionId]?.tree;
      if (!tree) return;
      for (const cached of Object.values(tree.listings)) cached.stale = true;
    }),

  forgetTreePath: (sessionId, path) =>
    set((s) => {
      const tree = s.workspaces[sessionId]?.tree;
      if (!tree) return;
      for (const key of Object.keys(tree.listings)) if (isUnder(key, path)) delete tree.listings[key];
      for (const key of Object.keys(tree.expanded)) if (isUnder(key, path)) delete tree.expanded[key];
      if (tree.selection !== null && isUnder(tree.selection, path)) tree.selection = null;
    }),

  moveTreePath: (sessionId, from, to) =>
    set((s) => {
      const tree = s.workspaces[sessionId]?.tree;
      if (!tree) return;
      const moved = (p: string) => `${to}${p.slice(from.length)}`;
      for (const key of Object.keys(tree.listings)) if (isUnder(key, from)) delete tree.listings[key];
      for (const key of Object.keys(tree.expanded)) {
        if (!isUnder(key, from)) continue;
        delete tree.expanded[key];
        tree.expanded[moved(key)] = true;
      }
      if (tree.selection !== null && isUnder(tree.selection, from)) tree.selection = moved(tree.selection);
    }),

  openEditorTab: (sessionId, bufferId) =>
    set((s) => {
      const ws = s.workspaces[sessionId];
      if (!ws) return;
      if (!ws.editorTabs.includes(bufferId)) ws.editorTabs.push(bufferId);
      ws.activeBuffer = bufferId;
      ws.panelMaximized = false; // a file that was just opened must be visible
    }),

  closeEditorTab: (sessionId, bufferId) =>
    set((s) => {
      const ws = s.workspaces[sessionId];
      if (!ws) return;
      const index = ws.editorTabs.indexOf(bufferId);
      if (index < 0) return;
      ws.editorTabs.splice(index, 1);
      if (ws.activeBuffer === bufferId) {
        ws.activeBuffer = ws.editorTabs[Math.min(index, ws.editorTabs.length - 1)] ?? null;
      }
      delete s.buffers[bufferId];
      delete s.cursors[bufferId];
    }),

  setActiveBuffer: (sessionId, bufferId) =>
    set((s) => {
      const ws = s.workspaces[sessionId];
      if (ws) ws.activeBuffer = bufferId;
    }),

  addTerminalTab: (sessionId, termId) =>
    set((s) => {
      const ws = s.workspaces[sessionId];
      if (!ws) return;
      if (!ws.terminalTabs.includes(termId)) ws.terminalTabs.push(termId);
      ws.activeTerminal = termId;
    }),

  removeTerminalTab: (sessionId, termId) =>
    set((s) => {
      const ws = s.workspaces[sessionId];
      if (!ws) return;
      const index = ws.terminalTabs.indexOf(termId);
      if (index < 0) return;
      ws.terminalTabs.splice(index, 1);
      if (ws.activeTerminal === termId) {
        ws.activeTerminal = ws.terminalTabs[Math.min(index, ws.terminalTabs.length - 1)] ?? null;
      }
      delete s.terminals[termId];
      if (s.terminalSearch === termId) s.terminalSearch = null;
      if (s.renamingTerminal === termId) s.renamingTerminal = null;
    }),

  setActiveTerminal: (sessionId, termId) =>
    set((s) => {
      const ws = s.workspaces[sessionId];
      if (ws) ws.activeTerminal = termId;
    }),

  setFilesStatus: (sessionId, status) =>
    set((s) => {
      const ws = s.workspaces[sessionId];
      if (ws) ws.filesStatus = status;
    }),

  setPanelOpen: (sessionId, open) =>
    set((s) => {
      const ws = s.workspaces[sessionId];
      if (!ws) return;
      ws.panelOpen = open;
      if (!open) ws.panelMaximized = false;
    }),

  setPanelMaximized: (sessionId, maximized) =>
    set((s) => {
      const ws = s.workspaces[sessionId];
      if (!ws) return;
      ws.panelMaximized = maximized;
      if (maximized) ws.panelOpen = true;
    }),
});
