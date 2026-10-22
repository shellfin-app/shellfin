// File tree actions (docs/ARCHITECTURE.md §6.2): lazy listings with a freshness rule, file operations, copy path,
// "Open terminal here". Remote paths are only manipulated through lib/remotePath.
import { getBackend } from "../../ipc";
import { IpcError, toIpcError } from "../../ipc/call";
import type { FileEntry } from "../../ipc/types";
import { invalidArgumentError, notConnectedError } from "../../lib/appErrors";
import { deleteEntryDialog } from "../../lib/dialogs";
import { canRemove, identityOf, type Writability } from "../../lib/permissions";
import { basename, dirname, hasNul, joinPath } from "../../lib/remotePath";
import { guard, reportError } from "../../lib/reportError";
import { displayServerText } from "../../lib/serverText";
import { isUnder, type TreeState } from "../../state/slices/workspaces";
import { useStore } from "../../state/store";
import { bufferPathsDeleted, markNewFile, openFile, renameBufferPaths } from "../editor/editorActions";
import { openTerminalTab } from "../terminal/terminalActions";

/** A cached listing older than this is refreshed when its directory is expanded (§6.2). */
export const LISTING_MAX_AGE_MS = 30_000;

// ---- session facts -----------------------------------------------------------------------------------------------

export function isConnected(sessionId: string): boolean {
  return useStore.getState().sessionsById[sessionId]?.state.phase === "connected";
}

/** "Open terminal here" needs a POSIX remote (§4.5). */
export function isPosixSession(sessionId: string): boolean {
  return useStore.getState().sessionsById[sessionId]?.info?.remotePathStyle === "posix";
}

// ---- listings ----------------------------------------------------------------------------------------------------

/** Loads the root listing of a session; a failure is reported once and shown as "could not load" instead of an endless "Loading…". */
export async function loadRoot(sessionId: string, path: string): Promise<void> {
  try {
    await loadDirectory(sessionId, path);
    useStore.getState().setFilesStatus(sessionId, "ok");
    loadRootParent(sessionId, path);
  } catch (e) {
    // A session the user closed meanwhile is not an error to report.
    if (!useStore.getState().sessionsById[sessionId]) return;
    useStore.getState().setFilesStatus(sessionId, "failed");
    reportError(e);
  }
}

/** Loads (or refreshes) a directory listing into the session's tree cache. */
export async function loadDirectory(sessionId: string, path: string): Promise<void> {
  const listing = await getBackend().sftpList(sessionId, path);
  useStore.getState().setListing(sessionId, path, listing, Date.now());
}

/** Normalises what the user typed in "Go to folder…": `~` is the home folder, the rest must be absolute. */
export function resolveTypedPath(input: string, home: string | null): string {
  let text = input.trim();
  if (text === "~" || text.startsWith("~/")) {
    if (home === null) throw new IpcError(invalidArgumentError("path"));
    text = text === "~" ? home : joinPath(home, text.slice(2));
  }
  if (text === "" || !text.startsWith("/") || hasNul(text)) throw new IpcError(invalidArgumentError("path"));
  const parts: string[] = [];
  for (const part of text.split("/")) {
    if (part === "" || part === ".") continue;
    if (part === "..") parts.pop();
    else parts.push(part);
  }
  return `/${parts.join("/")}`;
}

/**
 * Re-roots the tree at `path`. The listing is fetched first, so a failure (not found, permission) leaves the tree
 * where it was. Expanded folders and cached listings are keyed by absolute path and stay valid for any root.
 */
export async function changeRoot(sessionId: string, path: string): Promise<void> {
  requireConnected(sessionId);
  const listing = await getBackend().sftpList(sessionId, path);
  const store = useStore.getState();
  if (!store.workspaces[sessionId]) return;
  store.setListing(sessionId, path, listing, Date.now());
  store.setTreeRoot(sessionId, path);
  store.setFilesStatus(sessionId, "ok");
  store.selectTreeEntry(sessionId, path);
  loadRootParent(sessionId, path);
}

/**
 * Permission hints for the root's own children need the root folder's mode bits, which only the PARENT's listing has
 * (a root reached through "Go to folder" never listed it). Fetches that listing in the background and caches it; when it
 * cannot be read the hint stays "unknown" (no hint) instead of a false "allowed".
 */
export function loadRootParent(sessionId: string, root: string): void {
  const parent = dirname(root);
  if (parent === root) return;
  const tree = useStore.getState().workspaces[sessionId]?.tree;
  if (!tree || tree.listings[parent]) return;
  getBackend()
    .sftpList(sessionId, parent)
    .then((listing) => {
      const current = useStore.getState().workspaces[sessionId]?.tree;
      if (current?.root === root) useStore.getState().setListing(sessionId, parent, listing, Date.now());
    })
    .catch(() => undefined);
}

/** Hint whether `path` can be renamed, moved or deleted (write permission on its parent folder). Never blocks an action. */
export function removeHint(sessionId: string, path: string): Writability {
  const state = useStore.getState();
  const tree = state.workspaces[sessionId]?.tree;
  if (!tree || path === tree.root) return "unknown";
  return canRemove(lookupEntry(tree, dirname(path)), lookupEntry(tree, path), identityOf(state.sessionsById[sessionId]?.info));
}

/** "Up": re-roots at the parent of the current root (no-op at "/"). */
export async function goUp(sessionId: string): Promise<void> {
  const root = useStore.getState().workspaces[sessionId]?.tree.root;
  if (!root || root === "/") return;
  await changeRoot(sessionId, dirname(root));
}

/** "Go to folder…": the typed path (absolute, or `~`-relative) becomes the tree root. */
export async function goToFolder(sessionId: string, input: string): Promise<void> {
  const home = useStore.getState().sessionsById[sessionId]?.info?.homeDir ?? null;
  await changeRoot(sessionId, resolveTypedPath(input, home));
}

/** Explicit refresh of one directory (Refresh menu item, after own mutations and uploads). */
export async function refreshDirectory(sessionId: string, path: string): Promise<void> {
  try {
    await loadDirectory(sessionId, path);
  } catch (e) {
    // The folder vanished (deleted or renamed elsewhere): forget it instead of showing a frozen listing.
    if (toIpcError(e).error.code === "remoteNotFound" && path !== useStore.getState().workspaces[sessionId]?.tree.root) {
      useStore.getState().forgetTreePath(sessionId, path);
      await refreshDirectory(sessionId, dirname(path));
      return;
    }
    throw e;
  }
}

/** Expands/collapses a directory; loads its listing on first expansion and refreshes an old one (> 30 s, stale). */
export async function toggleDirectory(sessionId: string, path: string): Promise<void> {
  const store = useStore.getState();
  const tree = store.workspaces[sessionId]?.tree;
  if (!tree) return;
  const willExpand = tree.expanded[path] !== true;
  store.toggleExpanded(sessionId, path);
  if (!willExpand) return;
  const cached = tree.listings[path];
  const old = cached !== undefined && (cached.stale || Date.now() - cached.fetchedAtMs > LISTING_MAX_AGE_MS);
  if (cached && !old) return;
  try {
    await loadDirectory(sessionId, path);
  } catch (e) {
    if (cached) {
      // Showing the old listing is better than collapsing; tell the user why it is not fresh.
      reportError(e);
      return;
    }
    store.toggleExpanded(sessionId, path); // collapse again and tell the user why
    reportError(e);
  }
}

/** After a reconnect (§5.8): refresh the root and every expanded directory, shallow ones first. */
export async function refreshTree(sessionId: string): Promise<void> {
  const tree = useStore.getState().workspaces[sessionId]?.tree;
  if (!tree || tree.root === null) return;
  const dirs = Object.keys(tree.expanded).sort((a, b) => a.length - b.length);
  if (!dirs.includes(tree.root)) dirs.unshift(tree.root);
  let reported = false;
  for (const dir of dirs) {
    if (!useStore.getState().workspaces[sessionId]) return;
    try {
      await refreshDirectory(sessionId, dir);
    } catch (e) {
      if (!reported) reportError(e); // one notice is enough; the rest of the tree stays on the stale cache
      reported = true;
      if (toIpcError(e).error.code === "notConnected") return;
    }
  }
  useStore.getState().setFilesStatus(sessionId, "ok");
}

// ---- entries -----------------------------------------------------------------------------------------------------

/** The listing entry for `path`, or a synthetic directory entry for the tree root. */
export function lookupEntry(tree: TreeState, path: string): FileEntry | undefined {
  const parent = dirname(path);
  const found = tree.listings[parent]?.listing.entries.find((e) => e.path === path);
  if (found) return found;
  if (path === tree.root) {
    return {
      name: basename(path),
      path,
      kind: "dir",
      linkTargetKind: "dir",
      size: null,
      mtime: null,
      permissions: null,
      uid: null,
      gid: null,
      owner: null,
      group: null,
      lossyName: false,
    };
  }
  return undefined;
}

/** Is `path` shown as a symbolic link in the tree? (Saving or replacing through it writes to its target.) */
export function isSymlink(sessionId: string, path: string): boolean {
  const tree = useStore.getState().workspaces[sessionId]?.tree;
  return tree ? lookupEntry(tree, path)?.kind === "symlink" : false;
}

/** A directory, or a symlink that points to one (the tree lists and expands both). */
export function isDirLike(entry: Pick<FileEntry, "kind" | "linkTargetKind">): boolean {
  return entry.kind === "dir" || (entry.kind === "symlink" && entry.linkTargetKind === "dir");
}

/** Files the editor can try to open: regular files and symlinks to files (or of unknown target). */
export function isOpenable(entry: Pick<FileEntry, "kind" | "linkTargetKind" | "lossyName">): boolean {
  if (entry.lossyName) return false;
  return entry.kind === "file" || (entry.kind === "symlink" && entry.linkTargetKind !== "dir" && entry.linkTargetKind !== "other");
}

export type NameProblem = "empty" | "slash" | "dots" | "nul";

/** A remote entry name must be a single path component. */
export function nameProblem(name: string): NameProblem | null {
  if (name.trim() === "") return "empty";
  if (name.includes("/")) return "slash";
  if (name === "." || name === "..") return "dots";
  if (hasNul(name)) return "nul";
  return null;
}

function requireConnected(sessionId: string): void {
  if (!isConnected(sessionId)) throw new IpcError(notConnectedError(sessionId));
}

/** Starts naming a new file/folder inside `parent` (the tree shows an input row). */
export function startNewEntry(sessionId: string, parent: string, entry: "file" | "dir"): void {
  const store = useStore.getState();
  store.setTreeEdit(sessionId, { kind: "new", parent, entry });
  // The parent must be open (and loaded) for the input row to show up.
  const tree = store.workspaces[sessionId]?.tree;
  if (tree && !tree.listings[parent]) guard(loadDirectory(sessionId, parent));
}

export function startRename(sessionId: string, path: string): void {
  useStore.getState().setTreeEdit(sessionId, { kind: "rename", path });
}

export function cancelTreeEdit(sessionId: string): void {
  useStore.getState().setTreeEdit(sessionId, null);
}

/** Creates the named file or folder in `parent`. Throws the backend error (the input stays open). */
export async function createEntry(sessionId: string, parent: string, kind: "file" | "dir", name: string): Promise<void> {
  requireConnected(sessionId);
  const path = joinPath(parent, name);
  const backend = getBackend();
  const entry = kind === "file" ? await backend.sftpCreateFile(sessionId, path) : await backend.sftpMkdir(sessionId, path);
  const store = useStore.getState();
  store.setTreeEdit(sessionId, null);
  await refreshDirectory(sessionId, parent).catch(reportError);
  store.selectTreeEntry(sessionId, entry.path);
  if (kind === "file") {
    markNewFile(sessionId, entry.path);
    await openFile(sessionId, entry.path, { focus: true });
  }
}

/** After a rename/move: forget what the tree knew about `from`, follow open files, reload the affected folders. */
async function afterMove(sessionId: string, from: string, to: string): Promise<void> {
  const store = useStore.getState();
  store.moveTreePath(sessionId, from, to);
  renameBufferPaths(sessionId, from, to);
  store.setTreeEdit(sessionId, null);
  const parents = new Set([dirname(from), dirname(to)]);
  for (const dir of parents) {
    if (useStore.getState().workspaces[sessionId]?.tree.listings[dir]) await refreshDirectory(sessionId, dir).catch(reportError);
  }
  // Expanded folders below the moved one were re-keyed without listings: load them again.
  const tree = useStore.getState().workspaces[sessionId]?.tree;
  for (const dir of Object.keys(tree?.expanded ?? {})) {
    if (isUnder(dir, to) && !useStore.getState().workspaces[sessionId]?.tree.listings[dir]) await loadDirectory(sessionId, dir).catch(reportError);
  }
  useStore.getState().selectTreeEntry(sessionId, to);
}

/** Renames an entry inside its folder. Throws the backend error (`remoteExists` ...; the input stays open). */
export async function renameEntry(sessionId: string, path: string, newName: string): Promise<void> {
  requireConnected(sessionId);
  const to = joinPath(dirname(path), newName);
  if (to === path) {
    cancelTreeEdit(sessionId);
    return;
  }
  const moved = await getBackend().sftpRename(sessionId, path, to);
  await afterMove(sessionId, path, moved.path);
}

/** Moves an entry into another folder (the context-menu replacement for drag-to-move, which Windows lacks). */
export async function moveEntry(sessionId: string, path: string, destDir: string): Promise<void> {
  requireConnected(sessionId);
  if (!destDir.startsWith("/") || hasNul(destDir)) {
    throw new IpcError(invalidArgumentError("destination"));
  }
  const dest = destDir.length > 1 ? destDir.replace(/\/+$/, "") : destDir;
  const to = joinPath(dest, basename(path));
  if (isUnder(to, path)) throw new IpcError(invalidArgumentError("destination"));
  if (to === path) return;
  const moved = await getBackend().sftpRename(sessionId, path, to);
  await afterMove(sessionId, path, moved.path);
}

/** Deletes a file or an empty folder (`directoryNotEmpty` otherwise; there is no recursive delete in v1). */
export async function deleteEntry(sessionId: string, path: string): Promise<void> {
  requireConnected(sessionId);
  await getBackend().sftpDelete(sessionId, path);
  const store = useStore.getState();
  store.forgetTreePath(sessionId, path);
  bufferPathsDeleted(sessionId, path);
  await refreshDirectory(sessionId, dirname(path)).catch(reportError);
}

/**
 * User asked to delete an entry: opens the confirmation, or deletes straight away when the setting
 * `files.confirmDelete` is off (failures such as `directoryNotEmpty` are then reported as a notice).
 */
export function requestDelete(sessionId: string, path: string, entry: "file" | "dir"): void {
  const store = useStore.getState();
  if (store.settings?.files.confirmDelete === false) {
    guard(deleteEntry(sessionId, path));
    return;
  }
  store.openDialog(deleteEntryDialog(sessionId, path, entry));
}

/** True if showing `s` needs no escaping (it contains nothing `displayServerText` escapes). */
function isPlainText(s: string): boolean {
  return displayServerText(s, Number.MAX_SAFE_INTEGER) === s;
}

/** Copies a remote path. A path with characters that `displayServerText` escapes (control, bidi,
 * invisible or variation-selector characters) is copied in its escaped, visible form. */
export function copyPath(path: string): void {
  const text = isPlainText(path) ? path : displayServerText(path, Number.MAX_SAFE_INTEGER);
  navigator.clipboard.writeText(text).catch(() => undefined);
}

/** "Open terminal here": a new terminal in the folder (or the folder of a file). POSIX sessions only. */
export async function openTerminalHere(sessionId: string, path: string, entry: Pick<FileEntry, "kind" | "linkTargetKind">): Promise<void> {
  const dir = isDirLike(entry) ? path : dirname(path);
  await openTerminalTab(sessionId, dir);
}
