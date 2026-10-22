// Bookmark import / export (ARCHITECTURE §7.7, §8.10). The webview never sees a path: the backend opens the native
// dialogs. Passwords are never exported or imported.
import { t } from "../../i18n/translate";
import { getBackend } from "../../ipc";
import type { Backend } from "../../ipc/backend";
import type { BookmarkExportFormat, ImportStrategy } from "../../ipc/types";
import { bookmarksImportDialog, bookmarksIoDialog, bookmarksIoResultDialog } from "../../lib/dialogs";
import { useStore } from "../../state/store";

/** Opens the export options dialog (format). */
export function openBookmarksExport(): void {
  useStore.getState().openDialog(bookmarksIoDialog());
}

/**
 * Import step 1: the native Open dialog appears right away and the file is parsed, nothing is saved. A cancelled
 * dialog does nothing; otherwise the review dialog opens. Rejects with the backend error.
 */
export async function startBookmarksImport(): Promise<void> {
  const preview = await getBackend().bookmarksImportOpen(t("bookmarksIo.importDialogTitle"));
  if (!preview.picked || preview.token === null) return;
  useStore.getState().openDialog(bookmarksImportDialog(preview, preview.token));
}

/**
 * Import step 2: applies the reviewed import. The bookmark slice is refreshed and the summary replaces the review
 * dialog. Rejects with the backend error (the pending import is gone after any attempt).
 */
export async function applyBookmarksImport(token: string, strategy: ImportStrategy, keepStartupCommands: boolean): Promise<void> {
  const result = await getBackend().bookmarksImportApply(token, strategy, keepStartupCommands);
  const store = useStore.getState();
  if (result.bookmarks) store.setBookmarks(result.bookmarks); // no event is emitted for imports
  store.openDialog(bookmarksIoResultDialog({ mode: "import", data: result }));
}

/** Drops the pending import (review closed without importing). Never rejects. */
export function cancelBookmarksImport(token: string, backend: Backend = getBackend()): void {
  backend
    .bookmarksImportCancel(token)
    .catch(() => undefined);
}

/** Runs the export (native Save dialog). Cancelled: the options dialog stays; saved: the summary replaces it. */
export async function exportBookmarks(format: BookmarkExportFormat): Promise<void> {
  const result = await getBackend().bookmarksExport(format, t("bookmarksIo.exportDialogTitle"));
  if (!result.saved) return;
  useStore.getState().openDialog(bookmarksIoResultDialog({ mode: "export", data: result }));
}
