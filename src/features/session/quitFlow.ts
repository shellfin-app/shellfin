// Quit flow, frontend half (docs/ARCHITECTURE.md §8.9).
//   app:quit-requested -> app_quit_ack immediately; no dirty buffers -> app_quit(lastDirs);
//   dirty buffers -> <QuitConfirmDialog> (Save all / Discard / Cancel). A repeated request while the dialog is
//   open only re-focuses it. Cancel -> app_quit_cancel.
import { getBackend } from "../../ipc";
import type { Unlisten } from "../../ipc";
import { CONFIRM_QUIT } from "../../lib/identifiers";
import { guard, reportError } from "../../lib/reportError";
import { useStore, type RootState } from "../../state/store";
import { saveAllBuffers } from "../editor/editorActions";
import { onUpdateQuitCancelled } from "../updates/updateActions";

export function hasDirtyBuffers(): boolean {
  return Object.values(useStore.getState().buffers).some((b) => b.dirty);
}

/** Connections that quitting would end: established ones and those being re-established. */
export function countOpenConnections(s: Pick<RootState, "sessionOrder" | "sessionsById">): number {
  return s.sessionOrder.filter((id) => {
    const phase = s.sessionsById[id]?.state.phase;
    return phase === "connected" || phase === "reconnecting";
  }).length;
}

export function openConnectionCount(): number {
  return countOpenConnections(useStore.getState());
}

/**
 * Does quitting ask even though nothing is unsaved? `general.confirmQuit`: always / only with open connections
 * (default) / never. (`confirmCloseWithTerminals` now only governs closing a connection or a terminal tab.)
 */
export function needsQuitConfirm(): boolean {
  const mode = useStore.getState().settings?.general.confirmQuit ?? CONFIRM_QUIT.withConnections;
  if (mode === CONFIRM_QUIT.always) return true;
  if (mode === CONFIRM_QUIT.never) return false;
  return openConnectionCount() > 0;
}

export function collectLastDirs(): Record<string, string | null> {
  const { sessionOrder, workspaces } = useStore.getState();
  const dirs: Record<string, string | null> = {};
  for (const id of sessionOrder) dirs[id] = workspaces[id]?.tree.root ?? null;
  return dirs;
}

export async function handleQuitRequested(): Promise<void> {
  const backend = getBackend();
  const store = useStore.getState();
  await backend.appQuitAck();
  if (hasDirtyBuffers() || needsQuitConfirm()) {
    // First request, or a repeated one: (re)open the dialog if it is not showing (the backend stays in
    // `confirming` until it is answered, so it must always be reachable), otherwise just refocus it.
    store.setQuitState("confirming");
    if (store.dialog?.kind === "quit") store.bumpQuitFocus();
    else store.openDialog({ kind: "quit" });
    return;
  }
  store.setQuitState("idle");
  await backend.appQuit(collectLastDirs());
}

function finishDialog(): void {
  const store = useStore.getState();
  store.closeDialog("quit");
  store.setQuitState("idle");
}

/** QuitConfirmDialog: Cancel. */
export async function quitCancel(): Promise<void> {
  finishDialog();
  onUpdateQuitCancelled(); // "Update and restart" started this quit: back to the offer (the backend drops the download)
  await getBackend().appQuitCancel();
}

/** QuitConfirmDialog: Discard — quit without saving. */
export async function quitDiscard(): Promise<void> {
  finishDialog();
  await getBackend().appQuit(collectLastDirs());
}

/** QuitConfirmDialog: Save all, then quit. A failing save keeps the dialog open and shows the error. */
export async function quitSaveAll(): Promise<void> {
  try {
    await saveAllBuffers();
  } catch (e) {
    reportError(e);
    return;
  }
  finishDialog();
  await getBackend().appQuit(collectLastDirs());
}

/** Registers the `app:quit-requested` listener. */
export function installQuitFlow(): Promise<Unlisten> {
  return getBackend().listen("app:quit-requested", () => {
    guard(handleQuitRequested());
  });
}
