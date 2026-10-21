// Bookmark import / export (ARCHITECTURE §7.7, §8.10). The webview never sees a path: the backend opens the native
// dialogs. Passwords are never exported or imported.
import { t } from "../../i18n/translate";
import { getBackend } from "../../ipc";
import type { BookmarkExportFormat, ImportStrategy } from "../../ipc/types";
import { bookmarksIoDialog, bookmarksIoResultDialog } from "../../lib/dialogs";
import { useStore } from "../../state/store";

/** Opens the options dialog (conflict strategy for import, format for export). */
export function openBookmarksIo(mode: "import" | "export"): void {
  useStore.getState().openDialog(bookmarksIoDialog(mode));
}

/**
 * Runs the import with the chosen strategy (the native Open dialog appears now). A cancelled dialog leaves the options
 * dialog open; otherwise the bookmark slice is refreshed and the summary replaces it. Rejects with the backend error.
 */
export async function importBookmarks(strategy: ImportStrategy): Promise<void> {
  const result = await getBackend().bookmarksImport(strategy, t("bookmarksIo.importDialogTitle"));
  if (!result.picked) return;
  const store = useStore.getState();
  if (result.bookmarks) store.setBookmarks(result.bookmarks); // no event is emitted for imports
  store.openDialog(bookmarksIoResultDialog({ mode: "import", data: result }));
}

/** Runs the export (native Save dialog). Cancelled: the options dialog stays; saved: the summary replaces it. */
export async function exportBookmarks(format: BookmarkExportFormat): Promise<void> {
  const result = await getBackend().bookmarksExport(format, t("bookmarksIo.exportDialogTitle"));
  if (!result.saved) return;
  useStore.getState().openDialog(bookmarksIoResultDialog({ mode: "export", data: result }));
}
