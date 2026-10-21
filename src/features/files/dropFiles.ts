// OS file drops onto the file tree (docs/ARCHITECTURE.md §6.6). The backend owns the paths: it sends `drop:hover`
// with a position, `drop:leave`, and `drop:files` with a `dropId` token plus names and `isDir`. The webview only
// hit-tests the position against `[data-remote-dir]` and hands the token back to `transfer_upload_dropped`.
import { getBackend } from "../../ipc";
import type { Unlisten } from "../../ipc";
import type { DropFilesPayload, DropHoverPayload } from "../../ipc/types";
import { SEL_REMOTE_DIR, SEL_TREE_SESSION } from "../../lib/domIds";
import { DROP_NO_TARGET, FOLDERS_UNSUPPORTED } from "../../lib/notices";
import { guard } from "../../lib/reportError";
import { useStore } from "../../state/store";
import { uploadDropped } from "../transfers/transferActions";

export interface DropTarget {
  sessionId: string;
  dir: string;
}

type HitTest = (x: number, y: number) => Element | null;

const defaultHitTest: HitTest = (x, y) => (typeof document.elementFromPoint === "function" ? document.elementFromPoint(x, y) : null);

/** The remote folder under a window position, if the position is over the tree (rows, or the empty area). */
export function dropTargetAt(x: number, y: number, hitTest: HitTest = defaultHitTest): DropTarget | null {
  const el = hitTest(x, y);
  const holder = el?.closest<HTMLElement>(SEL_REMOTE_DIR);
  const dir = holder?.dataset["remoteDir"];
  const sessionId = holder?.closest<HTMLElement>(SEL_TREE_SESSION)?.dataset["treeSession"];
  return dir && sessionId ? { sessionId, dir } : null;
}

/** May files be dropped on this session's tree right now? (connected, SFTP, and it is the visible session) */
function acceptsDrops(sessionId: string): boolean {
  const s = useStore.getState();
  const entry = s.sessionsById[sessionId];
  return s.activeSessionId === sessionId && entry?.state.phase === "connected" && entry.info?.sftpAvailable === true;
}

export function onDropHover(payload: DropHoverPayload, hitTest?: HitTest): void {
  const target = dropTargetAt(payload.positionX, payload.positionY, hitTest);
  const store = useStore.getState();
  store.setDropTarget(target && acceptsDrops(target.sessionId) ? target.dir : null);
}

export function onDropLeave(): void {
  useStore.getState().setDropTarget(null);
}

export function onDropFiles(payload: DropFilesPayload, hitTest?: HitTest): void {
  const store = useStore.getState();
  store.setDropTarget(null);
  const target = dropTargetAt(payload.positionX, payload.positionY, hitTest);
  if (!target || !acceptsDrops(target.sessionId)) {
    store.pushInfo(DROP_NO_TARGET);
    return;
  }
  const hasFolders = payload.entries.some((e) => e.isDir);
  if (hasFolders) store.pushInfo(FOLDERS_UNSUPPORTED);
  if (payload.entries.every((e) => e.isDir)) return; // nothing uploadable; the drop token simply expires (60 s)
  guard(uploadDropped(target.sessionId, payload.dropId, target.dir));
}

/** Registers the three drop listeners. */
export async function installDropHandlers(): Promise<Unlisten[]> {
  const backend = getBackend();
  return Promise.all([
    backend.listen("drop:hover", (p) => onDropHover(p)),
    backend.listen("drop:leave", () => onDropLeave()),
    backend.listen("drop:files", (p) => onDropFiles(p)),
  ]);
}
