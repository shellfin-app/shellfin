// Editor glue between the Backend and the store: opening, saving (conflicts, pending saves), encodings, closing.
// Text and undo history live in the buffer registry; this module keeps the Zustand metadata in step with it.
import i18next from "i18next";
import { getBackend } from "../../ipc";
import { IpcError, toIpcError } from "../../ipc/call";
import type { Eol } from "../../ipc/types";
import { remoteChangedError, notConnectedError } from "../../lib/appErrors";
import { closeBufferDialog, conflictDialog } from "../../lib/dialogs";
import { mixedEolNotice } from "../../lib/notices";
import { basename } from "../../lib/remotePath";
import { guard, reportError } from "../../lib/reportError";
import { isUnder } from "../../state/slices/workspaces";
import type { BufferMeta } from "../../state/slices/buffers";
import { useStore } from "../../state/store";
import { saveTransformChanges } from "../../lib/saveTransforms";
import { loadBufferRegistry, loadedBufferRegistry } from "./bufferLoader";
import {
  bufferOptionsFromSettings,
  dropBuffer,
  getBufferState,
  initBuffer,
  isDocDirty,
  markSaved,
  moveBuffer,
  onBufferDropped,
  setBufferState,
} from "./bufferStore";

export function bufferIdFor(sessionId: string, path: string): string {
  return `${sessionId}:${path}`;
}

/** Open failures the editor area explains by itself (no extra toast). */
const EXPECTED_OPEN_FAILURES: ReadonlySet<string> = new Set([
  "fileIsBinary",
  "fileTooLarge",
  "decodeFailed",
  "unsupportedEncoding",
  "isADirectory",
  "notARegularFile", // a FIFO, socket or device; the failure page shows the error text
]);

// ---- language ----------------------------------------------------------------------------------------------------

// The search panel, fold markers, announcements and the accessible name of every open buffer follow the UI language
//. The view that is mounted is re-worded by `CodeEditor`; this covers the stored states of the other buffers.
const LANGUAGE_CHANGED = "languageChanged"; // i18next event name (data, not UI copy)
i18next.on(LANGUAGE_CHANGED, () => {
  const buffers = useStore.getState().buffers;
  // Without the CodeMirror chunk there is no stored state to re-word.
  loadedBufferRegistry()?.applyLocaleToAll((id) => buffers[id]?.path ?? null);
});

// ---- focus -------------------------------------------------------------------------------------------------------

let focusEditorFn: (() => void) | null = null;

/** The mounted CodeEditor registers how to focus its view. */
export function registerEditorFocus(fn: (() => void) | null): void {
  focusEditorFn = fn;
}

/** Moves keyboard focus into the editor (after the active buffer's view has mounted). */
export function focusEditor(): void {
  setTimeout(() => focusEditorFn?.(), 0);
}

// ---- dirty tracking ----------------------------------------------------------------------------------------------

/** Recomputes `dirty` from the live document and the encoding/EOL flag. Returns the result. */
export function refreshDirty(bufferId: string): boolean {
  const meta = useStore.getState().buffers[bufferId];
  if (!meta || meta.load !== "ready") return false;
  if (!getBufferState(bufferId)) return meta.dirty; // no live document to compare (flag set directly)
  const dirty = isDocDirty(bufferId) || meta.metaDirty;
  // Undone back to the saved text while a save was queued: nothing is left to save.
  if (dirty !== meta.dirty || (!dirty && meta.pendingSave)) {
    useStore.getState().patchBuffer(bufferId, dirty ? { dirty } : { dirty, pendingSave: false });
  }
  return dirty;
}

/** Buffers with unsaved changes (optionally of one session), after re-checking each against its saved text. */
export function dirtyBuffers(sessionId?: string): BufferMeta[] {
  const out: BufferMeta[] = [];
  for (const id of Object.keys(useStore.getState().buffers)) {
    const meta = useStore.getState().buffers[id];
    if (!meta || (sessionId !== undefined && meta.sessionId !== sessionId)) continue;
    if (refreshDirty(id)) {
      const fresh = useStore.getState().buffers[id];
      if (fresh) out.push(fresh);
    }
  }
  return out;
}

// ---- opening -----------------------------------------------------------------------------------------------------

const loadSeq = new Map<string, number>();

/** Buffers of files that were just created by the user ("New file…"): the next load applies `editor.newFileEol`. */
const newFileIds = new Set<string>();

/** Call right before opening a file this app just created. */
export function markNewFile(sessionId: string, path: string): void {
  newFileIds.add(bufferIdFor(sessionId, path));
}

/**
 * Reads the file into the buffer. `keep`: the buffer already shows a file (reload / reopen with another encoding),
 * so a failing read must leave it as it is instead of turning the tab into an error page.
 */
async function loadBuffer(id: string, encoding: string | null, keep: boolean): Promise<boolean> {
  const meta = useStore.getState().buffers[id];
  if (!meta) return false;
  const seq = (loadSeq.get(id) ?? 0) + 1;
  loadSeq.set(id, seq);
  try {
    // The CodeMirror chunk loads while the file is read.
    const [doc, registry] = await Promise.all([getBackend().sftpReadText(meta.sessionId, meta.path, encoding), loadBufferRegistry()]);
    // The tab (or the whole session) may have been closed while reading, or a newer read superseded this one.
    const current = useStore.getState().buffers[id];
    if (!current || loadSeq.get(id) !== seq) return false;
    const editor = useStore.getState().settings?.editor;
    initBuffer(id, registry.createBufferState(current.path, doc.text, bufferOptionsFromSettings(editor)));
    useStore.getState().patchBuffer(id, {
      load: "ready",
      failure: null,
      encoding: doc.encoding,
      hasBom: doc.hasBom,
      eol: doc.eol,
      mixedEol: doc.mixedEol,
      etag: doc.etag,
      resolvedPath: doc.resolvedPath ?? null,
      savedAtMs: null, // opening is not saving: nothing to say until the user saves
      dirty: false,
      metaDirty: false,
      pendingSave: false,
      saving: false,
      conflict: null,
      rev: current.rev + 1,
    });
    useStore.getState().setCursor(id, { line: 1, col: 1 });
    if (doc.mixedEol) useStore.getState().pushInfo(mixedEolNotice(basename(current.path)));
    // `editor.newFileEol`: a file created in Shellfin starts with the chosen line endings (existing files keep theirs).
    // The flag is not "unsaved": an empty file stays clean, the first save writes the chosen endings.
    if (newFileIds.delete(id) && doc.text === "" && useStore.getState().settings?.editor.newFileEol === "crlf") {
      useStore.getState().patchBuffer(id, { eol: "crlf" });
    }
    return true;
  } catch (e) {
    const current = useStore.getState().buffers[id];
    if (!current || loadSeq.get(id) !== seq) return false;
    const error = toIpcError(e).error;
    if (keep && current.load === "ready") {
      reportError(e);
      return false;
    }
    useStore.getState().patchBuffer(id, { load: "error", failure: error });
    if (!EXPECTED_OPEN_FAILURES.has(error.code)) reportError(e);
    return false;
  }
}

/** Opens (or activates) a remote text file in the editor. A tab that failed to open is retried. */
export async function openFile(sessionId: string, path: string, options: { focus?: boolean } = {}): Promise<void> {
  const id = bufferIdFor(sessionId, path);
  const store = useStore.getState();
  const existing = store.buffers[id];
  if (existing) {
    store.openEditorTab(sessionId, id);
    if (existing.load === "error") {
      store.patchBuffer(id, { load: "loading", failure: null });
      await loadBuffer(id, null, false);
    }
  } else {
    store.addBuffer({ id, sessionId, path });
    store.openEditorTab(sessionId, id);
    await loadBuffer(id, null, false);
  }
  if (options.focus && useStore.getState().buffers[id]?.load === "ready") focusEditor();
}

/** Re-reads the file with `encoding`, discarding local changes (the caller confirms). */
export async function reopenWithEncoding(bufferId: string, encoding: string): Promise<boolean> {
  return loadBuffer(bufferId, encoding, true);
}

/** Re-reads the file from the server in its current encoding, discarding local changes. */
export async function reloadFromServer(bufferId: string): Promise<boolean> {
  const meta = useStore.getState().buffers[bufferId];
  if (!meta) return false;
  return loadBuffer(bufferId, meta.load === "ready" ? meta.encoding : null, meta.load === "ready");
}

// ---- encoding / EOL ----------------------------------------------------------------------------------------------

/** Save with another encoding from now on (the next save re-encodes the text). */
export function setSaveEncoding(bufferId: string, encoding: string): void {
  const meta = useStore.getState().buffers[bufferId];
  if (!meta || meta.load !== "ready" || meta.encoding === encoding) return;
  useStore.getState().patchBuffer(bufferId, { encoding, hasBom: false, metaDirty: true, dirty: true });
}

function nextEol(eol: Eol): Eol {
  return eol === "lf" ? "crlf" : "lf";
}

/** Status bar EOL click: LF <-> CRLF (a lone CR file goes to LF). Marks the buffer dirty. */
export function toggleEol(bufferId: string): void {
  const meta = useStore.getState().buffers[bufferId];
  if (!meta || meta.load !== "ready") return;
  useStore.getState().patchBuffer(bufferId, { eol: nextEol(meta.eol), mixedEol: false, metaDirty: true, dirty: true });
}

// ---- saving ------------------------------------------------------------------------------------------------------

export type SaveOutcome = "saved" | "conflict" | "pending" | "skipped";

export interface SaveOptions {
  /** Overwrite a server file that changed meanwhile (the conflict dialog's "Overwrite"). */
  force?: boolean;
  /** Open the conflict dialog on `remoteChanged` (default). Quit-time "Save all" reports instead. */
  interactive?: boolean;
}

const inflight = new Map<string, Promise<SaveOutcome>>();

/**
 * Saves a buffer (§6.4): expected etag, `force` only after the user chose Overwrite. While the session is not
 * connected the save is queued (`pendingSave`) and runs after the reconnect (§5.8). `remoteChanged` opens the
 * conflict dialog. Other failures reject (the buffer stays dirty).
 */
export function saveBuffer(bufferId: string, options: SaveOptions = {}): Promise<SaveOutcome> {
  snoozed.delete(bufferId);
  const running = inflight.get(bufferId);
  if (running) {
    // One save at a time per buffer; a second request afterwards saves again only if there is still something to save.
    return running.catch(() => undefined).then(() => saveBuffer(bufferId, options));
  }
  const promise = doSave(bufferId, options).finally(() => inflight.delete(bufferId));
  inflight.set(bufferId, promise);
  return promise;
}

async function doSave(bufferId: string, options: SaveOptions): Promise<SaveOutcome> {
  const store = useStore.getState();
  const meta = store.buffers[bufferId];
  let state = getBufferState(bufferId);
  if (!meta || !state || meta.load !== "ready") return "skipped";
  if (!options.force && !refreshDirty(bufferId)) {
    if (meta.pendingSave) store.patchBuffer(bufferId, { pendingSave: false });
    return "skipped";
  }

  const session = store.sessionsById[meta.sessionId];
  if (!session || session.state.phase !== "connected") {
    store.patchBuffer(bufferId, { pendingSave: true });
    return "pending";
  }

  // Save-time transforms (settings): applied to the buffer as ONE undoable transaction before the text is sent, so
  // the editor shows exactly what is saved. Dirty tracking compares against the document saved below (markSaved),
  // and the etag handling is untouched (the same expected etag is sent).
  const editorSettings = store.settings?.editor;
  const changes = editorSettings ? saveTransformChanges(state.doc, editorSettings) : [];
  if (changes.length > 0) {
    state = state.update({ changes, userEvent: "save.transform" }).state;
    setBufferState(bufferId, state);
    store.patchBuffer(bufferId, { rev: meta.rev + 1 }); // the mounted view swaps to the transformed state
  }

  const doc = state.doc;
  const sent = { encoding: meta.encoding, eol: meta.eol };
  store.patchBuffer(bufferId, { saving: true });
  try {
    const result = await getBackend().sftpWriteText(meta.sessionId, {
      path: meta.path,
      text: doc.toString(),
      encoding: meta.encoding,
      hasBom: meta.hasBom,
      eol: meta.eol,
      expected: meta.etag,
      force: options.force === true,
      ...(meta.resolvedPath !== null ? { expectedTarget: meta.resolvedPath } : {}),
    });
    const after = useStore.getState();
    const current = after.buffers[bufferId];
    if (!current) return "saved"; // closed while saving
    markSaved(bufferId, doc);
    after.patchBuffer(bufferId, {
      etag: result.etag,
      // After a forced save onto a re-pointed link the buffer now stands for the file that was written.
      ...(meta.conflict?.target !== undefined && options.force === true ? { resolvedPath: meta.conflict.target } : {}),
      savedAtMs: Date.now(),
      saving: false,
      pendingSave: false,
      conflict: null,
      metaDirty: current.encoding !== sent.encoding || current.eol !== sent.eol,
    });
    refreshDirty(bufferId);
    for (const warning of result.warnings) after.pushNotice(warning, "warning");
    return "saved";
  } catch (e) {
    const error = toIpcError(e).error;
    const after = useStore.getState();
    if (!after.buffers[bufferId]) return "skipped";
    after.patchBuffer(bufferId, { saving: false });
    if (error.code === "remoteChanged") {
      const { size, mtime, target } = error.params;
      after.patchBuffer(bufferId, { conflict: target === undefined ? { size, mtime } : { size, mtime, target }, pendingSave: false });
      if (options.interactive !== false) after.openDialog(conflictDialog(bufferId));
      return "conflict";
    }
    if (error.code === "notConnected") {
      after.patchBuffer(bufferId, { pendingSave: true });
      return "pending";
    }
    // Any other failure is reported by the caller; nothing stays queued.
    after.patchBuffer(bufferId, { pendingSave: false });
    throw e;
  }
}

/** Failure of a strict "save all" that could not save a buffer for a reason other than a thrown backend error. */
function unsavedError(meta: BufferMeta, outcome: SaveOutcome): IpcError {
  if (outcome === "conflict") return new IpcError(remoteChangedError(meta.path, meta.conflict?.size ?? 0, meta.conflict?.mtime ?? 0, meta.conflict?.target));
  return new IpcError(notConnectedError(meta.sessionId));
}

/**
 * Saves every dirty buffer (of one session, or all), in tab order. Strict (default, used by the quit dialog): stops
 * at the first buffer that is not saved and throws why. `interactive` (Mod+Alt+S) keeps going and lets conflicts
 * open the dialog.
 */
export async function saveAllBuffers(options: { sessionId?: string; interactive?: boolean } = {}): Promise<void> {
  const interactive = options.interactive === true;
  for (const meta of dirtyBuffers(options.sessionId)) {
    const outcome = await saveBuffer(meta.id, { interactive });
    if (interactive) continue;
    if (outcome === "conflict" || outcome === "pending") {
      throw unsavedError(useStore.getState().buffers[meta.id] ?? meta, outcome);
    }
  }
}

/** After a reconnect: runs the saves that were requested while the connection was down (with etag checks). */
export async function runPendingSaves(sessionId: string): Promise<void> {
  const pending = Object.values(useStore.getState().buffers).filter((b) => b.sessionId === sessionId && b.pendingSave);
  let conflicts = false;
  for (const meta of pending) {
    try {
      // Not user-initiated: a conflict must never replace a dialog the user has open.
      if ((await saveBuffer(meta.id, { interactive: false })) === "conflict") conflicts = true;
    } catch (e) {
      reportError(e);
    }
  }
  if (conflicts) openConflictWhenFree();
}

let conflictWatch: (() => void) | null = null;

/** Opens the next conflict now if no dialog is open, otherwise as soon as the open dialog has closed. */
function openConflictWhenFree(): void {
  if (useStore.getState().dialog === null) {
    openNextConflict();
    return;
  }
  if (conflictWatch) return;
  conflictWatch = useStore.subscribe((state) => {
    if (state.dialog !== null) return;
    conflictWatch?.();
    conflictWatch = null;
    openNextConflict();
  });
}

/** Buffers whose conflict dialog was cancelled: not offered again until the user saves them again. */
const snoozed = new Set<string>();

// Buffers dropped without `closeBuffer` (a closed session) must not leave entries in the per-buffer maps.
onBufferDropped((bufferId) => {
  loadSeq.delete(bufferId);
  snoozed.delete(bufferId);
  newFileIds.delete(bufferId);
});

/** Cancel in the conflict dialog: keep the edits and the conflict flag, but do not ask again by itself. */
export function snoozeConflict(bufferId: string): void {
  snoozed.add(bufferId);
}

/** Opens the conflict dialog for the next buffer that still has an unresolved conflict (if nothing else is open). */
export function openNextConflict(): void {
  const store = useStore.getState();
  if (store.dialog !== null) return;
  const next = Object.values(store.buffers).find((b) => b.conflict !== null && !snoozed.has(b.id));
  if (next) store.openDialog(conflictDialog(next.id));
}

// ---- closing / renaming ------------------------------------------------------------------------------------------

/** Closes a tab without asking (the caller has handled unsaved changes). */
export function closeBuffer(sessionId: string, bufferId: string): void {
  loadSeq.delete(bufferId);
  inflight.delete(bufferId);
  snoozed.delete(bufferId);
  dropBuffer(bufferId);
  useStore.getState().closeEditorTab(sessionId, bufferId);
}

/** Closes a tab; unsaved changes are confirmed first (Save / Don't save / Cancel). */
export function requestCloseBuffer(bufferId: string): void {
  const meta = useStore.getState().buffers[bufferId];
  if (!meta) return;
  if (meta.load === "ready" && refreshDirty(bufferId)) {
    useStore.getState().openDialog(closeBufferDialog(bufferId));
    return;
  }
  closeBuffer(meta.sessionId, bufferId);
}

/** Closes the active tab of a session (Mod+W). */
export function closeActiveBuffer(sessionId: string): void {
  const id = useStore.getState().workspaces[sessionId]?.activeBuffer;
  if (id) requestCloseBuffer(id);
}

/** Ctrl+Tab / Ctrl+Shift+Tab. */
export function cycleBuffer(sessionId: string, delta: 1 | -1): void {
  const ws = useStore.getState().workspaces[sessionId];
  if (!ws || ws.editorTabs.length < 2) return;
  const index = ws.activeBuffer ? ws.editorTabs.indexOf(ws.activeBuffer) : -1;
  const next = ws.editorTabs[(index + delta + ws.editorTabs.length) % ws.editorTabs.length];
  if (next) useStore.getState().setActiveBuffer(sessionId, next);
}

/** The remote file or folder `from` was renamed/moved to `to`: open buffers follow it (state and undo history kept). */
export function renameBufferPaths(sessionId: string, from: string, to: string): void {
  for (const meta of Object.values(useStore.getState().buffers)) {
    if (meta.sessionId !== sessionId || !isUnder(meta.path, from)) continue;
    const newPath = `${to}${meta.path.slice(from.length)}`;
    const newId = bufferIdFor(sessionId, newPath);
    moveBuffer(meta.id, newId);
    const seq = loadSeq.get(meta.id);
    if (seq !== undefined) {
      loadSeq.delete(meta.id);
      loadSeq.set(newId, seq);
    }
    useStore.getState().renameBuffer(meta.id, newId, newPath);
  }
}

/** The remote file or folder `path` was deleted: clean buffers close, edited ones stay and can recreate the file. */
export function bufferPathsDeleted(sessionId: string, path: string): void {
  for (const meta of Object.values(useStore.getState().buffers)) {
    if (meta.sessionId !== sessionId || !isUnder(meta.path, path)) continue;
    if (meta.load === "ready" && refreshDirty(meta.id)) {
      useStore.getState().patchBuffer(meta.id, { etag: null, conflict: null });
    } else {
      closeBuffer(sessionId, meta.id);
    }
  }
}

/** Fire-and-forget Save for keyboard shortcuts and menus. */
export function saveActiveBuffer(sessionId: string): void {
  const id = useStore.getState().workspaces[sessionId]?.activeBuffer;
  if (id) guard(saveBuffer(id));
}
