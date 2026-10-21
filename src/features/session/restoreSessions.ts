// `general.onStartup = "restore"`: reopens the bookmarks that were open at the last quit (`AppBootstrap.restoreBookmarkIds`,
// ARCHITECTURE §7.3, §8.11). Credentials are prompted as usual and bookmarks with "Confirm before connecting" still ask first.
// The backend repeats the list until the next quit (a webview reload sees it again), so this runs once per app start.
import { reportError } from "../../lib/reportError";
import { bookmarkTarget } from "../../lib/targets";
import { useStore } from "../../state/store";
import { requestOpenSession } from "./sessionActions";

let restored = false;

export async function restoreBookmarkSessions(ids: readonly string[]): Promise<void> {
  if (restored) return;
  restored = true;
  for (const id of ids) {
    const bookmark = useStore.getState().bookmarks?.bookmarks.find((b) => b.id === id);
    if (!bookmark) continue;
    try {
      await requestOpenSession(bookmarkTarget(id), bookmark.name, undefined, { background: true });
    } catch (e) {
      reportError(e); // one bookmark failing to open must not stop the others
    }
  }
}

/** Test hook: lets the next `restoreBookmarkSessions` run again. */
export function resetRestoreForTests(): void {
  restored = false;
}
