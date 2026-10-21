// Connection-level actions shared by the menu bar, the session tab menu and the palette.
import { getBackend } from "../../ipc";
import { bookmarkDialog } from "../../lib/dialogs";
import type { RootState } from "../../state/slices/types";
import { useStore } from "../../state/store";
import { guard } from "../../lib/reportError";
import { editConnection } from "./editConnection";
import { reconnectNow, requestOpenSession } from "./sessionActions";

/** A reconnect makes sense: waiting for the next automatic attempt, or a failed connection that can be retried. */
export function canReconnect(state: RootState, sessionId: string | null): boolean {
  if (!sessionId) return false;
  const phase = state.sessionsById[sessionId]?.state;
  if (!phase) return false;
  if (phase.phase === "reconnecting") return phase.stage === "waiting";
  return phase.phase === "disconnected" && phase.canRetry;
}

/** Quick connects and ssh-config hosts can be kept; a saved bookmark already is one. */
export function canSaveAsBookmark(state: RootState, sessionId: string | null): boolean {
  const entry = sessionId ? state.sessionsById[sessionId] : undefined;
  return entry !== undefined && entry.target.kind !== "bookmark";
}

export function reconnectSession(sessionId: string): void {
  guard(reconnectNow(sessionId));
}

/** Backend `session_disconnect`: the connection is dropped, the tab, its files and terminals stay ("Reconnect" brings it back). */
export function disconnectSession(sessionId: string): void {
  guard(getBackend().sessionDisconnect(sessionId));
}

/** Disconnect makes sense only for a live connection. */
export function canDisconnect(state: RootState, sessionId: string | null): boolean {
  return sessionId !== null && state.sessionsById[sessionId]?.state.phase === "connected";
}

/** A second session to the same target, in a new tab. */
export function duplicateSession(sessionId: string): void {
  const entry = useStore.getState().sessionsById[sessionId];
  if (!entry) return;
  guard(requestOpenSession(entry.target, entry.label, undefined, { forceNew: true }));
}

/** "Save as bookmark…": the bookmark dialog prefilled from the session (nothing is saved until the user saves). */
export function saveSessionAsBookmark(sessionId: string): void {
  const entry = useStore.getState().sessionsById[sessionId];
  if (!entry) return;
  if (entry.target.kind === "quick") useStore.getState().openDialog(bookmarkDialog(null, { spec: entry.target.spec, name: entry.label }));
  else if (entry.target.kind === "sshConfig") editConnection(sessionId, { replace: false });
}
