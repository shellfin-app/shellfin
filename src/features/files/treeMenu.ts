// Context menu of the file tree (rows and the empty area), built on the app's menu system (features/menu).
import { t } from "../../i18n/translate";
import { goToFolderDialog, moveEntryDialog } from "../../lib/dialogs";
import { ID_GO_INTO, ID_NEW_FILE, ID_NEW_FOLDER, ID_TERMINAL_HERE, MENU_COPY_PATH } from "../../lib/domIds";
import { enterable, identityOf, writability } from "../../lib/permissions";
import { dirname } from "../../lib/remotePath";
import { guard } from "../../lib/reportError";
import { useStore } from "../../state/store";
import { openFile } from "../editor/editorActions";
import type { MenuItem } from "../menu/menuStore";
import { downloadFile, uploadFiles } from "../transfers/transferActions";
import {
  changeRoot,
  copyPath,
  isConnected,
  isDirLike,
  isOpenable,
  isPosixSession,
  lookupEntry,
  openTerminalHere,
  refreshDirectory,
  removeHint,
  requestDelete,
  startNewEntry,
  startRename,
} from "./fileActions";

/** Items for a right-click on the tree entry `path` (the root row and the empty area use the root path). */
export function treeMenuItems(sessionId: string, path: string): MenuItem[] {
  const store = useStore.getState();
  const tree = store.workspaces[sessionId]?.tree;
  if (!tree) return [];
  const entry = lookupEntry(tree, path);
  if (!entry) return [];
  const isRoot = path === tree.root;
  const dir = isDirLike(entry);
  const online = isConnected(sessionId);
  // A name with U+FFFD cannot be sent back to the server: every operation on it is off (§6.2).
  const ok = online && !entry.lossyName;
  const targetDir = dir ? path : dirname(path);
  // Permission hint: the parent folder decides whether an entry can be renamed, moved or deleted.
  const identity = identityOf(store.sessionsById[sessionId]?.info);
  // Hints WARN (tooltip + confirmation text), they never disable: ACLs, capabilities and other server rules are invisible here.
  const lockedTitle = ok && removeHint(sessionId, path) === "no" ? t("files.parentNotWritable") : undefined;
  const targetEntry = dir ? entry : lookupEntry(tree, targetDir);
  const createTitle =
    ok && (writability(targetEntry, identity) === "no" || (dir && enterable(entry, identity) === "no")) ? t("files.folderNotWritable") : undefined;
  const shortcutRename = t("shortcut.rename");
  const shortcutDelete = t("shortcut.delete");

  const items: MenuItem[] = [];
  if (!dir) {
    items.push({
      id: "open",
      label: t("files.open"),
      disabled: !ok || !isOpenable(entry),
      onSelect: () => guard(openFile(sessionId, path, { focus: true })),
    });
  } else if (!isRoot) {
    // Re-root the tree one level down at this folder (not "open as root": the connection stays the same).
    items.push({
      id: ID_GO_INTO,
      label: t("files.goInto"),
      disabled: !ok,
      onSelect: () => guard(changeRoot(sessionId, path)),
    });
  }
  items.push(
    {
      id: ID_NEW_FILE,
      label: t("files.newFile"),
      separatorBefore: !dir || !isRoot,
      disabled: !ok,
      title: createTitle,
      onSelect: () => startNewEntry(sessionId, targetDir, "file"),
    },
    { id: ID_NEW_FOLDER, label: t("files.newFolder"), disabled: !ok, title: createTitle, onSelect: () => startNewEntry(sessionId, targetDir, "dir") },
  );
  if (dir) {
    items.push({
      id: "upload",
      label: t("files.upload"),
      separatorBefore: true,
      disabled: !ok,
      onSelect: () => guard(uploadFiles(sessionId, path)),
    });
  } else {
    items.push({
      id: "download",
      label: t("files.download"),
      separatorBefore: true,
      disabled: !ok || entry.kind === "other",
      onSelect: () => guard(downloadFile(sessionId, path)),
    });
  }
  items.push({
    id: ID_TERMINAL_HERE,
    label: t("files.openTerminalHere"),
    disabled: !ok || !isPosixSession(sessionId),
    onSelect: () => guard(openTerminalHere(sessionId, path, entry)),
  });
  if (!isRoot) {
    items.push(
      { id: "rename", label: t("files.rename"), shortcut: shortcutRename, separatorBefore: true, disabled: !ok, title: lockedTitle, onSelect: () => startRename(sessionId, path) },
      { id: "move", label: t("files.move"), disabled: !ok, title: lockedTitle, onSelect: () => store.openDialog(moveEntryDialog(sessionId, path)) },
      {
        id: "delete",
        label: t("files.delete"),
        shortcut: shortcutDelete,
        disabled: !ok,
        title: lockedTitle,
        onSelect: () => requestDelete(sessionId, path, dir ? "dir" : "file"),
      },
    );
  }
  items.push(
    { id: MENU_COPY_PATH, label: t("files.copyPath"), separatorBefore: true, disabled: entry.lossyName, onSelect: () => copyPath(path) },
    { id: "refresh", label: t("files.refresh"), disabled: !ok, onSelect: () => guard(refreshDirectory(sessionId, targetDir)) },
    { id: "goto", label: t("files.goToFolder"), shortcut: t(store.platform === "macos" ? "shortcut.goToFolderMac" : "shortcut.goToFolder"), disabled: !online, onSelect: () => store.openDialog(goToFolderDialog(sessionId)) },
  );
  return items;
}
