// Bookmark startup command: sent once, with a trailing Enter, to the FIRST terminal of a user-initiated
// connection after `term_open` resolved. Not for later terminals, not after `term_restart`, not on the respawn after a
// reconnect (ARCHITECTURE §8.11). The text is never logged and never put in the store.
import { getBackend } from "../../ipc";
import { useStore } from "../../state/store";
import type { RecentTarget } from "../../ipc/types";

const pending = new Map<string, string>();

/** The startup command of the bookmark behind `target` (or of the draft), `null` when there is none. */
export function startupCommandFor(target: RecentTarget, draftCommand?: string | null): string | null {
  const command =
    draftCommand ??
    (target.kind === "bookmark" ? (useStore.getState().bookmarks?.bookmarks.find((b) => b.id === target.id)?.startupCommand ?? null) : null);
  return command !== null && command.trim() !== "" ? command : null;
}

/** Remembers the command for the session's first terminal. */
export function armStartupCommand(sessionId: string, command: string | null): void {
  if (command === null) pending.delete(sessionId);
  else pending.set(sessionId, command);
}

export function disarmStartupCommand(sessionId: string): void {
  pending.delete(sessionId);
}

/** Called when a terminal of the session is open: sends the waiting command, once. */
export function sendStartupCommand(sessionId: string, termId: string): void {
  const command = pending.get(sessionId);
  if (command === undefined) return;
  pending.delete(sessionId);
  const bytes = new TextEncoder().encode(`${command}\r`);
  getBackend()
    .termWrite(termId, bytes)
    .catch(() => undefined);
}
