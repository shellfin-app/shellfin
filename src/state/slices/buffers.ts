import type { AppError, Eol, Etag } from "../../ipc/types";
import type { SliceCreator } from "./types";

export type BufferLoadState = "loading" | "ready" | "error";

/** Buffer metadata only. The CodeMirror `EditorState` (text, undo history) lives in the editor's buffer registry. */
export interface BufferMeta {
  id: string;
  sessionId: string;
  path: string;
  load: BufferLoadState;
  /** Why the file could not be opened (`load: "error"`): binary, too large, not decodable ... */
  failure: AppError | null;
  /** Text differs from the last saved/loaded text, or encoding/EOL changed since then. */
  dirty: boolean;
  /** Encoding, BOM or EOL changed since the last save (a save is needed even though the text is the same). */
  metaDirty: boolean;
  encoding: string;
  hasBom: boolean;
  eol: Eol;
  mixedEol: boolean;
  etag: Etag | null;
  /** The file really read when the opened path is a symlink (`TextDocument.resolvedPath`); sent back as `expectedTarget`. */
  resolvedPath: string | null;
  savedAtMs: number | null;
  /** Bumped whenever the registry state was replaced from outside (load, reload) so the view re-syncs. */
  rev: number;
  /** A save was requested while the session was not connected: it runs after the reconnect (§5.8). */
  pendingSave: boolean;
  saving: boolean;
  /** The server file changed under us (`remoteChanged`); the conflict dialog resolves it. */
  conflict: { size: number; mtime: number; /** The file a forced save would write (the path resolves through a symlink). */ target?: string } | null;
}

/** What callers of `addBuffer` must provide; the rest gets defaults. */
export type BufferInit = Pick<BufferMeta, "id" | "sessionId" | "path"> & Partial<Omit<BufferMeta, "id" | "sessionId" | "path">>;

/** 1-based line and column of the main selection. */
export interface EditorCursor {
  line: number;
  col: number;
}

export interface BuffersSlice {
  buffers: Record<string, BufferMeta>;
  /**
   * Cursor positions by buffer id. They change on every key press and cursor move, so they live apart from `buffers`:
   * only the status bar subscribes, and no `buffers` subscriber re-renders while the user types.
   */
  cursors: Record<string, EditorCursor>;
  setCursor(id: string, cursor: EditorCursor): void;
  addBuffer(meta: BufferInit): void;
  patchBuffer(id: string, patch: Partial<Omit<BufferMeta, "id" | "sessionId">>): void;
  /** Moves a buffer to a new id/path (the remote file was renamed or moved). */
  renameBuffer(oldId: string, newId: string, newPath: string): void;
}

export function newBuffer(init: BufferInit): BufferMeta {
  return {
    load: "loading",
    failure: null,
    dirty: false,
    metaDirty: false,
    encoding: "utf-8",
    hasBom: false,
    eol: "lf",
    mixedEol: false,
    etag: null,
    resolvedPath: null,
    savedAtMs: null,
    rev: 0,
    pendingSave: false,
    saving: false,
    conflict: null,
    ...init,
  };
}

export const createBuffersSlice: SliceCreator<BuffersSlice> = (set) => ({
  buffers: {},
  cursors: {},
  setCursor: (id, cursor) =>
    set((s) => {
      const old = s.cursors[id];
      if (s.buffers[id] === undefined || (old && old.line === cursor.line && old.col === cursor.col)) return;
      s.cursors[id] = cursor;
    }),
  addBuffer: (init) =>
    set((s) => {
      s.buffers[init.id] = newBuffer(init);
      delete s.cursors[init.id];
    }),
  patchBuffer: (id, patch) =>
    set((s) => {
      const b = s.buffers[id];
      if (b) Object.assign(b, patch);
    }),
  renameBuffer: (oldId, newId, newPath) =>
    set((s) => {
      const b = s.buffers[oldId];
      if (!b || oldId === newId) return;
      delete s.buffers[oldId];
      s.buffers[newId] = { ...b, id: newId, path: newPath };
      const cursor = s.cursors[oldId];
      delete s.cursors[oldId];
      if (cursor) s.cursors[newId] = cursor;
      const ws = s.workspaces[b.sessionId];
      if (!ws) return;
      ws.editorTabs = ws.editorTabs.map((x) => (x === oldId ? newId : x));
      if (ws.activeBuffer === oldId) ws.activeBuffer = newId;
    }),
});
