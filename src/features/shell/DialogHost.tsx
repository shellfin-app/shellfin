import { Component, Suspense, lazy, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { Dialog } from "../../components/Dialog";
import { Icon } from "../../components/Icon";
import { CONFIRM_DELETE_KIND } from "../../lib/dialogs";
import { displayServerText } from "../../lib/serverText";
import { guard, reportError } from "../../lib/reportError";
import { useStore } from "../../state/store";
import { CloseBufferDialog, CloseSessionDialog } from "../editor/CloseDialogs";
import { ConflictDialog } from "../editor/ConflictDialog";
import { EncodingPicker } from "../editor/EncodingPicker";
import { DeleteEntryDialog, GoToFolderDialog, MoveEntryDialog } from "../files/EntryDialogs";
import { ReplaceDialog } from "../transfers/ReplaceDialog";
import { BookmarkDialog } from "../hosts/BookmarkDialog";
import { deleteBookmark } from "../hosts/hostActions";
import { DeleteGroupDialog } from "../hosts/DeleteGroupDialog";
import { MoveBookmarkDialog } from "../hosts/MoveBookmarkDialog";
import { ConnectConfirmDialog } from "../session/ConnectConfirmDialog";
import { PromptHost } from "../session/PromptHost";
import { PasteConfirmDialog } from "../terminal/PasteConfirmDialog";
import { CloseTerminalsDialog } from "../terminal/CloseTerminalsDialog";
import { UpdateDialog } from "../updates/UpdateDialog";
import { ConfigWarningsDialog } from "../welcome/ConfigWarningsDialog";
import { QuitConfirmDialog } from "./QuitConfirmDialog";

// Big, rarely used dialogs are their own chunks: Settings (all its sections), SSH keys, the bookmark
// import/export dialogs, the command palette (cmdk) and the help dialogs. Chunks load from the app bundle, so the wait
// is a frame or two and the placeholder is empty.
const SettingsDialog = lazy(() => import("../settings/SettingsDialog").then((m) => ({ default: m.SettingsDialog })));
const KeysDialog = lazy(() => import("../keys/KeysDialog").then((m) => ({ default: m.KeysDialog })));
const CommandPalette = lazy(() => import("../palette/CommandPalette").then((m) => ({ default: m.CommandPalette })));
const ImportBookmarksDialog = lazy(() => import("../hosts/BookmarksIoDialogs").then((m) => ({ default: m.ImportBookmarksDialog })));
const ExportBookmarksDialog = lazy(() => import("../hosts/BookmarksIoDialogs").then((m) => ({ default: m.ExportBookmarksDialog })));
const BookmarksIoResultDialog = lazy(() => import("../hosts/BookmarksIoDialogs").then((m) => ({ default: m.BookmarksIoResultDialog })));
const ShortcutsDialog = lazy(() => import("./HelpDialogs").then((m) => ({ default: m.ShortcutsDialog })));
const AboutDialog = lazy(() => import("./HelpDialogs").then((m) => ({ default: m.AboutDialog })));

/**
 * Contains a lazy dialog that failed (its chunk did not load, or it threw while rendering): the error becomes a notice
 * and `onError` closes the dialog. Without it the app-wide boundary would replace the whole window, and its Retry
 * would fail again at once because the dialog is still open in the store. Keyed by the dialog, so the next one
 * starts with a fresh boundary.
 */
export class LazyDialogBoundary extends Component<{ onError: () => void; children: ReactNode }, { failed: boolean }> {
  state = { failed: false };

  static getDerivedStateFromError(): { failed: boolean } {
    return { failed: true };
  }

  componentDidCatch(error: Error): void {
    reportError(error);
    this.props.onError();
  }

  render() {
    return this.state.failed ? null : this.props.children;
  }
}

function ConfirmDeleteBookmark({ bookmarkId }: { bookmarkId: string }) {
  const { t } = useTranslation();
  const name = useStore((s) => s.bookmarks?.bookmarks.find((b) => b.id === bookmarkId)?.name);
  const closeStoreDialog = useStore((s) => s.closeDialog);
  const close = () => closeStoreDialog(CONFIRM_DELETE_KIND);
  return (
    <Dialog
      title={t("hosts.deleteTitle")}
      tone="danger"
      role="alertdialog"
      width={440}
      icon={<Icon name="trash" size={20} strokeWidth={1.8} />}
      onClose={close}
      footer={
        <>
          <Button onClick={close} data-autofocus>
            {t("common.cancel")}
          </Button>
          <Button
            variant="primary"
            onClick={() => {
              close();
              guard(deleteBookmark(bookmarkId));
            }}
          >
            {t("common.delete")}
          </Button>
        </>
      }
    >
      <p className="dialog__text">{t("hosts.deleteBody", { name: displayServerText(name ?? "", 256) })}</p>
    </Dialog>
  );
}

/** Mounts every store-driven modal: dialogs, the command palette and the authentication prompts. */
export function DialogHost() {
  const dialog = useStore((s) => s.dialog);
  const palette = useStore((s) => s.palette);
  const kind = dialog?.kind;
  const closeLazyDialog = () => {
    if (kind !== undefined) useStore.getState().closeDialog(kind);
  };
  const closePalette = () => useStore.getState().closePalette();
  // Order matters: later siblings are on top. The quit confirmation is last because it takes precedence over
  // everything else (the backend waits for its answer, §8.9).
  return (
    <>
      {dialog?.kind === "bookmark" ? <BookmarkDialog key={dialog.bookmarkId ?? `new:${dialog.prefill?.spec ?? ""}`} bookmarkId={dialog.bookmarkId} prefill={dialog.prefill} replaceSession={dialog.replaceSession} /> : null}
      {dialog?.kind === "confirmDeleteBookmark" ? <ConfirmDeleteBookmark bookmarkId={dialog.bookmarkId} /> : null}
      {dialog?.kind === "moveBookmark" ? <MoveBookmarkDialog key={dialog.bookmarkId} bookmarkId={dialog.bookmarkId} /> : null}
      {dialog?.kind === "deleteGroup" ? <DeleteGroupDialog key={dialog.groupId} groupId={dialog.groupId} /> : null}
      <LazyDialogBoundary key={`help:${kind ?? ""}`} onError={closeLazyDialog}>
        <Suspense fallback={null}>
          {dialog?.kind === "settings" ? <SettingsDialog /> : null}
          {dialog?.kind === "keys" ? <KeysDialog /> : null}
          {dialog?.kind === "shortcuts" ? <ShortcutsDialog /> : null}
          {dialog?.kind === "about" ? <AboutDialog /> : null}
        </Suspense>
      </LazyDialogBoundary>
      {dialog?.kind === "confirmPaste" ? <PasteConfirmDialog /> : null}
      {dialog?.kind === "confirmConnect" ? <ConnectConfirmDialog /> : null}
      {dialog?.kind === "conflict" ? <ConflictDialog key={dialog.bufferId} bufferId={dialog.bufferId} /> : null}
      {dialog?.kind === "encoding" ? <EncodingPicker key={dialog.bufferId} bufferId={dialog.bufferId} /> : null}
      {dialog?.kind === "closeBuffer" ? <CloseBufferDialog key={dialog.bufferId} bufferId={dialog.bufferId} /> : null}
      {dialog?.kind === "closeSession" ? <CloseSessionDialog key={dialog.sessionId} sessionId={dialog.sessionId} reopen={dialog.reopen} /> : null}
      <LazyDialogBoundary key={`io:${kind ?? ""}`} onError={closeLazyDialog}>
        <Suspense fallback={null}>
          {dialog?.kind === "bookmarksIo" ? <ExportBookmarksDialog key="export" /> : null}
          {dialog?.kind === "bookmarksImport" ? <ImportBookmarksDialog key={dialog.token} preview={dialog.preview} token={dialog.token} /> : null}
          {dialog?.kind === "bookmarksIoResult" ? <BookmarksIoResultDialog result={dialog.result} /> : null}
        </Suspense>
      </LazyDialogBoundary>
      {dialog?.kind === "confirmCloseTerminals" ? (
        <CloseTerminalsDialog key={`${dialog.sessionId}:${dialog.termIds?.join(",") ?? ""}`} sessionId={dialog.sessionId} termIds={dialog.termIds} />
      ) : null}
      {dialog?.kind === "deleteEntry" ? (
        <DeleteEntryDialog key={dialog.path} sessionId={dialog.sessionId} path={dialog.path} entry={dialog.entry} />
      ) : null}
      {dialog?.kind === "moveEntry" ? <MoveEntryDialog key={dialog.path} sessionId={dialog.sessionId} path={dialog.path} /> : null}
      {dialog?.kind === "goToFolder" ? <GoToFolderDialog key={dialog.sessionId} sessionId={dialog.sessionId} /> : null}
      {dialog?.kind === "replaceTransfers" ? <ReplaceDialog sessionId={dialog.sessionId} /> : null}
      {dialog?.kind === "update" ? <UpdateDialog /> : null}
      {dialog?.kind === "configWarnings" ? <ConfigWarningsDialog /> : null}
      <LazyDialogBoundary key={`palette:${palette ?? ""}`} onError={closePalette}>
        <Suspense fallback={null}>{palette ? <CommandPalette key={palette} mode={palette} /> : null}</Suspense>
      </LazyDialogBoundary>
      <PromptHost />
      {dialog?.kind === "quit" ? <QuitConfirmDialog /> : null}
    </>
  );
}
