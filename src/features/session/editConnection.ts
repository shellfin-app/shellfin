import { bookmarkDialog } from "../../lib/dialogs";
import { useStore } from "../../state/store";

/**
 * A session this edit may replace: one that failed before it ever connected (no workspace, no files, no terminals).
 * Anything else is left alone: closing it would drop unsaved buffers.
 */
export function canReplaceSession(sessionId: string): boolean {
  const state = useStore.getState();
  const entry = state.sessionsById[sessionId];
  return entry !== undefined && entry.state.phase === "disconnected" && entry.info === undefined && state.workspaces[sessionId] === undefined;
}

/**
 * Opens the bookmark dialog for a session's target (single implementation behind the failure panel, the Connection
 * menu, the tab menu and the palette):
 *  - a saved bookmark opens its editor;
 *  - an ssh-config host opens a NEW bookmark prefilled with its alias (the copy is created only when the user saves;
 *    an existing bookmark for that alias is edited instead of duplicated);
 *  - a quick connect opens a new-connection form prefilled from its `user@host:port`.
 * The tab is replaced when the dialog connects, but only if the session failed before connecting.
 */
export function editConnection(sessionId: string, options: { replace?: boolean } = {}): void {
  const store = useStore.getState();
  const entry = store.sessionsById[sessionId];
  if (!entry) return;
  const replace = options.replace !== false && canReplaceSession(sessionId) ? sessionId : undefined;
  const { target } = entry;
  switch (target.kind) {
    case "bookmark":
      store.openDialog(bookmarkDialog(target.id, undefined, replace));
      return;
    case "sshConfig": {
      const existing = store.bookmarks?.bookmarks.find((b) => b.useSshConfig && b.host === target.alias);
      store.openDialog(
        existing
          ? bookmarkDialog(existing.id, undefined, replace)
          : bookmarkDialog(null, { spec: target.alias, name: target.alias, sshConfig: true }, replace),
      );
      return;
    }
    case "quick":
      store.openDialog(bookmarkDialog(null, { spec: target.spec }, replace));
      return;
  }
}

/** Label of the action: an ssh-config host without a bookmark yet is "copied" into one. */
export function editConnectionNeedsCopy(sessionId: string): boolean {
  const state = useStore.getState();
  const target = state.sessionsById[sessionId]?.target;
  return target?.kind === "sshConfig" && !state.bookmarks?.bookmarks.some((b) => b.useSshConfig && b.host === target.alias);
}
