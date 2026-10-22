import type { BookmarksImportPreview } from "../ipc/types";
import type { BookmarksIoResult, DialogState } from "../state/slices/ui";

// Constructors for store-driven dialogs (discriminant values are identifiers, not UI copy).
/**
 * `prefill`: a `[user@]host[:port]` spec (+ optional name) for a new bookmark. `replaceSession`: a failed session whose
 * tab is closed when the dialog connects (Edit connection... on the failure panel).
 */
export const bookmarkDialog = (
  bookmarkId: string | null,
  prefill?: { spec: string; name?: string; sshConfig?: boolean },
  replaceSession?: string,
): DialogState => ({
  kind: "bookmark",
  bookmarkId,
  ...(prefill ? { prefill } : {}),
  ...(replaceSession ? { replaceSession } : {}),
});
export const confirmDeleteDialog = (bookmarkId: string): DialogState => ({ kind: "confirmDeleteBookmark", bookmarkId });
export const settingsDialog = (): DialogState => ({ kind: "settings" });
export const keysDialog = (): DialogState => ({ kind: "keys" });
export const configWarningsDialog = (): DialogState => ({ kind: "configWarnings" });

export const CONFIRM_DELETE_KIND = "confirmDeleteBookmark" as const;
export const CONFIRM_PASTE_KIND = "confirmPaste" as const;
export const CONFIRM_CONNECT_KIND = "confirmConnect" as const;

// Dialogs: editor and file-tree flows.
export const conflictDialog = (bufferId: string): DialogState => ({ kind: "conflict", bufferId });
export const encodingDialog = (bufferId: string): DialogState => ({ kind: "encoding", bufferId });
export const closeBufferDialog = (bufferId: string): DialogState => ({ kind: "closeBuffer", bufferId });
export const closeSessionDialog = (sessionId: string, reopen?: boolean): DialogState => ({ kind: "closeSession", sessionId, reopen });
export const moveBookmarkDialog = (bookmarkId: string): DialogState => ({ kind: "moveBookmark", bookmarkId });
export const deleteGroupDialog = (groupId: string): DialogState => ({ kind: "deleteGroup", groupId });
export const deleteEntryDialog = (sessionId: string, path: string, entry: "file" | "dir"): DialogState => ({
  kind: "deleteEntry",
  sessionId,
  path,
  entry,
});
export const moveEntryDialog = (sessionId: string, path: string): DialogState => ({ kind: "moveEntry", sessionId, path });
export const goToFolderDialog = (sessionId: string): DialogState => ({ kind: "goToFolder", sessionId });
export const replaceTransfersDialog = (sessionId: string): DialogState => ({ kind: "replaceTransfers", sessionId });

export const confirmCloseTerminalsDialog = (sessionId: string, termIds?: string[]): DialogState =>
  termIds === undefined ? { kind: "confirmCloseTerminals", sessionId } : { kind: "confirmCloseTerminals", sessionId, termIds };
export const bookmarksIoDialog = (): DialogState => ({ kind: "bookmarksIo" });
export const bookmarksImportDialog = (preview: BookmarksImportPreview, token: string): DialogState => ({ kind: "bookmarksImport", preview, token });
export const bookmarksIoResultDialog = (result: BookmarksIoResult): DialogState => ({ kind: "bookmarksIoResult", result });

/** `closeDialog(kind)` arguments. */
export const DIALOG_KIND = {
  confirmCloseTerminals: "confirmCloseTerminals",
  bookmarksIo: "bookmarksIo",
  bookmarksImport: "bookmarksImport",
  bookmarksIoResult: "bookmarksIoResult",
  conflict: "conflict",
  encoding: "encoding",
  closeBuffer: "closeBuffer",
  closeSession: "closeSession",
  moveBookmark: "moveBookmark",
  deleteGroup: "deleteGroup",
  deleteEntry: "deleteEntry",
  moveEntry: "moveEntry",
  goToFolder: "goToFolder",
  replaceTransfers: "replaceTransfers",
  configWarnings: "configWarnings",
} as const satisfies Record<string, DialogState["kind"]>;
