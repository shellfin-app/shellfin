// `files.autoRefresh`: reloads the expanded folders of every connected session every N seconds (5/10/30/60).
// Never overlapping (a slow server just skips ticks), paused while the window is hidden and for sessions that are not
// connected, silent on errors (the explicit refresh and the reconnect refresh are the ones that report).
import { toIpcError } from "../../ipc/call";
import { useStore } from "../../state/store";
import { refreshDirectory } from "./fileActions";

const running = new Set<string>();

function parentOf(path: string): string | null {
  if (path === "/") return null;
  const cut = path.lastIndexOf("/");
  return cut <= 0 ? "/" : path.slice(0, cut);
}

/**
 * The folders whose listing is on screen: the root plus every expanded folder below it whose ancestors (up to the
 * root) are expanded too. `expanded` keeps folders of other roots and of collapsed parents; those are not listed.
 */
export function visibleFolders(root: string, expanded: Readonly<Record<string, unknown>>): string[] {
  const visible = (dir: string): boolean => {
    for (let p: string | null = dir; p !== null; p = parentOf(p)) {
      if (p === root) return true;
      if (!(p in expanded)) return false;
    }
    return false;
  };
  const dirs = Object.keys(expanded)
    .filter((d) => d !== root && visible(d))
    .sort((a, b) => a.length - b.length);
  return [root, ...dirs];
}

/** One pass over a session: root and expanded folders, shallow ones first. Resolves when done. */
export async function autoRefreshSession(sessionId: string): Promise<void> {
  if (running.has(sessionId)) return;
  const state = useStore.getState();
  if (state.sessionsById[sessionId]?.state.phase !== "connected") return;
  const tree = state.workspaces[sessionId]?.tree;
  if (!tree || tree.root === null || tree.edit !== null) return; // do not move rows under an inline name input
  running.add(sessionId);
  try {
    for (const dir of visibleFolders(tree.root, tree.expanded)) {
      // The connection may have dropped or the user navigated away meanwhile.
      if (useStore.getState().sessionsById[sessionId]?.state.phase !== "connected") return;
      if (document.visibilityState === "hidden") return;
      try {
        await refreshDirectory(sessionId, dir);
      } catch (e) {
        if (toIpcError(e).error.code === "notConnected") return;
        // anything else: keep the cached listing, try again next tick
      }
    }
  } finally {
    running.delete(sessionId);
  }
}

/** One tick for every session; does nothing while the window is hidden. */
export function autoRefreshTick(): void {
  if (document.visibilityState === "hidden") return;
  for (const id of useStore.getState().sessionOrder) void autoRefreshSession(id);
}

/** Starts the timer for the current setting and follows changes. Returns the uninstall function. */
export function installAutoRefresh(): () => void {
  let timer: ReturnType<typeof setInterval> | null = null;
  let seconds = 0;
  const apply = () => {
    const wanted = useStore.getState().settings?.files.autoRefresh ?? 0;
    if (wanted === seconds) return;
    seconds = wanted;
    if (timer !== null) clearInterval(timer);
    timer = wanted > 0 ? setInterval(autoRefreshTick, wanted * 1000) : null;
  };
  apply();
  const unsubscribe = useStore.subscribe(apply);
  return () => {
    unsubscribe();
    if (timer !== null) clearInterval(timer);
  };
}

/** Test hook. */
export function isAutoRefreshing(sessionId: string): boolean {
  return running.has(sessionId);
}
