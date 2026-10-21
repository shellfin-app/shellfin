// Context menu of a bookmark-list row: Connect, Open in new tab, Edit, Duplicate, Copy ssh command, Copy host,
// Copy to bookmarks, Delete. Rows are found by `data-host-key` (`bm:<id>` / `ssh:<alias>`, see hostRows.ts).
import { getBackend } from "../../ipc";
import type { Bookmark, BookmarkInput, RecentTarget } from "../../ipc/types";
import { t } from "../../i18n/translate";
import { bookmarkDialog, confirmDeleteDialog, deleteGroupDialog, moveBookmarkDialog } from "../../lib/dialogs";
import { guard } from "../../lib/reportError";
import { sshCommand } from "../../lib/sshCommand";
import { bookmarkTarget, sshConfigTarget } from "../../lib/targets";
import { useStore } from "../../state/store";
import type { MenuItem } from "../menu/menuStore";
import { canDisconnect, disconnectSession } from "../session/connectionActions";
import { findSession, requestCloseSession, requestOpenSession } from "../session/sessionActions";
import { copyFromSshConfig, moveBookmark, refreshBookmarks } from "./hostActions";
import { stepTarget } from "./bookmarkOrder";

function writeClipboard(text: string): void {
  navigator.clipboard.writeText(text).catch(() => undefined);
}

function inputFrom(b: Bookmark, name: string): BookmarkInput {
  const auth: BookmarkInput["auth"] = b.auth.method === "key" ? { method: "key", identityFile: b.auth.identityFile } : { method: b.auth.method };
  return {
    id: null,
    name,
    host: b.host,
    port: b.port,
    user: b.user,
    auth,
    proxyJump: b.proxyJump,
    groupId: b.groupId,
    color: b.color,
    tags: [...b.tags],
    startDir: b.startDir,
    useSshConfig: b.useSshConfig,
  };
}

/** A copy named "<name> (copy)"; a saved password is not copied (it is bound to the original). */
export async function duplicateBookmark(id: string): Promise<void> {
  const original = useStore.getState().bookmarks?.bookmarks.find((b) => b.id === id);
  if (!original) return;
  await getBackend().bookmarkSave(inputFrom(original, t("hosts.copyName", { name: original.name })));
  await refreshBookmarks();
}

/**
 * Connect / new tab, or - when the target already has a session - Go to session, Disconnect, Close connection and
 * Open another connection (the default action of the row is the same as the first item).
 */
function connectItems(target: RecentTarget, label: string): MenuItem[] {
  const sessionId = findSession(target);
  if (sessionId === null) {
    return [
      { id: "connect", label: t("hosts.menu.connect"), onSelect: () => guard(requestOpenSession(target, label)) },
      { id: "newtab", label: t("hosts.menu.openInNewTab"), onSelect: () => guard(requestOpenSession(target, label, undefined, { forceNew: true })) },
    ];
  }
  const state = useStore.getState();
  return [
    { id: "gotosession", label: t("hosts.menu.goToSession"), onSelect: () => state.setActiveSession(sessionId) },
    { id: "disconnect", label: t("action.disconnect"), disabled: !canDisconnect(state, sessionId), onSelect: () => disconnectSession(sessionId) },
    { id: "closeconnection", label: t("action.closeConnection"), onSelect: () => requestCloseSession(sessionId) },
    { id: "newtab", label: t("hosts.menu.openAnother"), onSelect: () => guard(requestOpenSession(target, label, undefined, { forceNew: true })) },
  ];
}

/** Context menu of a group header: "Delete group…" (empty: confirm; otherwise the bookmarks are moved, never deleted). */
export function groupMenuItems(groupId: string): MenuItem[] {
  const state = useStore.getState();
  if (!state.bookmarks?.groups.some((g) => g.id === groupId)) return [];
  return [
    {
      id: "deletegroup",
      label: t("hosts.menu.deleteGroup"),
      shortcut: t("shortcut.delete"),
      danger: true,
      disabled: state.bookmarks.readOnly,
      onSelect: () => state.openDialog(deleteGroupDialog(groupId)),
    },
  ];
}

/** The items for the row `key`, or an empty list when the row no longer exists. */
export function hostMenuItems(key: string): MenuItem[] {
  const state = useStore.getState();
  const readOnly = state.bookmarks?.readOnly ?? false;
  const bookmarks = state.bookmarks?.bookmarks ?? [];
  const hosts = state.sshConfig?.hosts ?? [];

  if (key.startsWith("bm:")) {
    const bookmark = bookmarks.find((b) => b.id === key.slice(3));
    if (!bookmark) return [];
    const target = bookmarkTarget(bookmark.id);
    const command = sshCommand(target, bookmarks, hosts);
    const dto = state.bookmarks;
    const up = dto ? stepTarget(dto, bookmark.id, -1) : null;
    const down = dto ? stepTarget(dto, bookmark.id, 1) : null;
    return [
      ...connectItems(target, bookmark.name),
      {
        id: "edit",
        label: t("hosts.menu.edit"),
        separatorBefore: true,
        shortcut: t("shortcut.rename"),
        disabled: readOnly,
        onSelect: () => state.openDialog(bookmarkDialog(bookmark.id)),
      },
      { id: "duplicate", label: t("hosts.menu.duplicate"), disabled: readOnly, onSelect: () => guard(duplicateBookmark(bookmark.id)) },
      {
        id: "moveup",
        label: t("hosts.menu.moveUp"),
        shortcut: t("shortcut.moveUp"),
        separatorBefore: true,
        disabled: readOnly || up === null,
        onSelect: () => up && guard(moveBookmark(bookmark.id, up.groupId, up.index)),
      },
      {
        id: "movedown",
        label: t("hosts.menu.moveDown"),
        shortcut: t("shortcut.moveDown"),
        disabled: readOnly || down === null,
        onSelect: () => down && guard(moveBookmark(bookmark.id, down.groupId, down.index)),
      },
      {
        id: "movetogroup",
        label: t("hosts.menu.moveToGroup"),
        disabled: readOnly,
        onSelect: () => state.openDialog(moveBookmarkDialog(bookmark.id)),
      },
      {
        id: "copycommand",
        label: t("hosts.menu.copySshCommand"),
        separatorBefore: true,
        disabled: command === null,
        onSelect: () => command && writeClipboard(command),
      },
      { id: "copyhost", label: t("hosts.menu.copyHost"), onSelect: () => writeClipboard(bookmark.host) },
      {
        id: "delete",
        label: t("hosts.menu.delete"),
        shortcut: t("shortcut.delete"),
        separatorBefore: true,
        danger: true,
        disabled: readOnly,
        onSelect: () => state.openDialog(confirmDeleteDialog(bookmark.id)),
      },
    ];
  }

  if (key.startsWith("ssh:")) {
    const host = hosts.find((h) => h.alias === key.slice(4));
    if (!host) return [];
    const target = sshConfigTarget(host.alias);
    const command = sshCommand(target, bookmarks, hosts);
    return [
      ...connectItems(target, host.alias),
      {
        id: "copytobookmarks",
        label: t("hosts.menu.copyToBookmarks"),
        separatorBefore: true,
        disabled: readOnly,
        onSelect: () => guard(copyFromSshConfig(host.alias)),
      },
      {
        id: "copycommand",
        label: t("hosts.menu.copySshCommand"),
        disabled: command === null,
        onSelect: () => command && writeClipboard(command),
      },
      { id: "copyhost", label: t("hosts.menu.copyHost"), onSelect: () => writeClipboard(host.hostname) },
    ];
  }
  return [];
}
