// Uploads and downloads (docs/ARCHITECTURE.md §6.6). The webview never supplies local paths: the native dialogs and
// the OS drop (a `dropId` token from the backend) do. Progress arrives as `TransferEvent`s on a per-call channel.
import { getBackend } from "../../ipc";
import { toIpcError } from "../../ipc/call";
import type { TransferEvent } from "../../ipc/types";
import { t } from "../../i18n/translate";
import { replaceTransfersDialog } from "../../lib/dialogs";
import type { TransferRow } from "../../state/slices/transfers";
import { useStore } from "../../state/store";
import { joinPath } from "../../lib/remotePath";
import { refreshDirectory, toggleDirectory } from "../files/fileActions";

const REFRESH_DELAY_MS = 150;
const PROMPT_DELAY_MS = 200;

const refreshTimers = new Map<string, ReturnType<typeof setTimeout>>();
const promptTimers = new Map<string, ReturnType<typeof setTimeout>>();

/** Refresh a directory listing once after a burst of finished uploads. */
function scheduleRefresh(sessionId: string, dir: string): void {
  const key = `${sessionId}\u0000${dir}`;
  if (refreshTimers.has(key)) return;
  refreshTimers.set(
    key,
    setTimeout(() => {
      refreshTimers.delete(key);
      const tree = useStore.getState().workspaces[sessionId]?.tree;
      if (tree?.listings[dir]) refreshDirectory(sessionId, dir).catch(() => undefined);
    }, REFRESH_DELAY_MS),
  );
}

/** `transfers.onConflict === "ask"` (also while the settings are unknown): only then is the Replace dialog shown. */
function askOnConflict(): boolean {
  return (useStore.getState().settings?.transfers.onConflict ?? "ask") === "ask";
}

/** One "Replace?" dialog for all files of a multi-file upload that hit existing remote files. */
function schedulePrompt(sessionId: string): void {
  if (promptTimers.has(sessionId)) return;
  promptTimers.set(
    sessionId,
    setTimeout(() => {
      promptTimers.delete(sessionId);
      const s = useStore.getState();
      const waiting = replaceableRows(s.transfers, sessionId).length > 0;
      // Never push over another dialog or the palette: the popover still offers "Replace" per file.
      if (waiting && s.dialog === null && s.palette === null && s.activeSessionId === sessionId) {
        s.openDialog(replaceTransfersDialog(sessionId));
      }
    }, PROMPT_DELAY_MS),
  );
}

/** A failed upload that only failed because the remote file exists: the user may choose to replace it. */
export function isReplaceable(row: TransferRow): boolean {
  return row.status === "failed" && row.retryable && row.error?.code === "remoteExists";
}

/** The rows a "Replace?" dialog asks about: one per (session, remote dir, name), the newest wins. */
export function replaceableRows(rows: TransferRow[], sessionId: string): TransferRow[] {
  const byKey = new Map<string, TransferRow>();
  for (const r of rows) {
    if (r.sessionId === sessionId && isReplaceable(r)) byKey.set(`${r.remoteDir ?? ""}\u0000${r.name}`, r);
  }
  return rows.filter((r) => byKey.get(`${r.remoteDir ?? ""}\u0000${r.name}`) === r && r.sessionId === sessionId && isReplaceable(r));
}

/** The user declined to replace: the conflicting transfers are dismissed here and forgotten by the backend. */
export function dismissTransfers(rows: TransferRow[]): void {
  for (const r of rows) {
    useStore.getState().patchTransfer(r.transferId, { status: "cancelled", retryable: false });
    // `transfer_cancel` on a failed transfer forgets its retryable record (backend-to-frontend.md §11).
    getBackend()
      .transferCancel(r.transferId)
      .catch(() => undefined);
  }
}

/** Applies one transfer event to the store (exported for tests). */
export function applyTransferEvent(sessionId: string, remoteDir: string | null, event: TransferEvent): void {
  const store = useStore.getState();
  switch (event.type) {
    case "started":
      store.upsertTransfer({
        transferId: event.transferId,
        sessionId,
        name: event.name,
        direction: event.direction,
        bytes: 0,
        total: event.total,
        status: "running",
        retryable: false,
        error: null,
        remoteDir: event.direction === "upload" ? remoteDir : null,
      });
      break;
    case "progress":
      store.patchTransfer(event.transferId, { bytes: event.bytes, total: event.total });
      break;
    case "done": {
      store.patchTransfer(event.transferId, { status: "done", bytes: event.bytes, error: null });
      const row = useStore.getState().transfers.find((r) => r.transferId === event.transferId);
      if (row?.direction === "upload" && row.remoteDir) scheduleRefresh(sessionId, row.remoteDir);
      break;
    }
    case "failed": {
      // A folder in a drop: the "folders are not supported yet" notice already told the user.
      if (event.error.code === "localIsADirectory") {
        store.removeTransfer(event.transferId);
        break;
      }
      if (!useStore.getState().transfers.some((r) => r.transferId === event.transferId)) {
        store.upsertTransfer({
          transferId: event.transferId,
          sessionId,
          name: "",
          direction: "upload",
          bytes: 0,
          total: null,
          status: "running",
          retryable: false,
          error: null,
          remoteDir,
        });
      }
      if (event.error.code === "remoteExists" && event.retryable) {
        // An older, still pending conflict for the same file (e.g. an earlier drop) is superseded by this one.
        const me = useStore.getState().transfers.find((r) => r.transferId === event.transferId);
        const stale = useStore
          .getState()
          .transfers.filter(
            (r) =>
              r.transferId !== event.transferId &&
              r.sessionId === sessionId &&
              isReplaceable(r) &&
              r.name === me?.name &&
              r.remoteDir === me?.remoteDir,
          );
        dismissTransfers(stale);
      }
      store.patchTransfer(event.transferId, { status: "failed", error: event.error, retryable: event.retryable });
      // The Replace dialog belongs to `onConflict = "ask"` only (the backend never reports remoteExists otherwise).
      if (event.error.code === "remoteExists" && event.retryable && askOnConflict()) schedulePrompt(sessionId);
      break;
    }
    case "cancelled":
      store.patchTransfer(event.transferId, { status: "cancelled" });
      break;
    case "skipped":
      store.patchTransfer(event.transferId, { status: "skipped", retryable: false });
      break;
  }
}

function handler(sessionId: string, remoteDir: string | null): (event: TransferEvent) => void {
  return (event) => applyTransferEvent(sessionId, remoteDir, event);
}

/** "Upload files…": the backend shows the native multi-file picker. Resolves with the started transfer ids. */
export async function uploadFiles(sessionId: string, remoteDir: string): Promise<string[]> {
  return getBackend().transferUploadPick(sessionId, remoteDir, t("transfers.uploadDialogTitle"), false, handler(sessionId, remoteDir));
}

/** Uploads the files of an OS drop (`dropId` is consumed by the first call, §6.6). */
export async function uploadDropped(sessionId: string, dropId: string, remoteDir: string): Promise<string[]> {
  return getBackend().transferUploadDropped(sessionId, dropId, remoteDir, false, handler(sessionId, remoteDir));
}

/** "Download…": the backend shows the native save dialog. Resolves with the transfer id, or `null` if cancelled. */
export async function downloadFile(sessionId: string, remotePath: string): Promise<string | null> {
  return getBackend().transferDownload(sessionId, remotePath, t("transfers.downloadDialogTitle"), handler(sessionId, null));
}

/** Retries a failed transfer (only the conflicting file is resent). `overwrite` answers "Replace?". */
export async function retryTransfer(transferId: string, overwrite: boolean): Promise<void> {
  const row = useStore.getState().transfers.find((r) => r.transferId === transferId);
  if (!row) return;
  try {
    await getBackend().transferRetry(transferId, overwrite, handler(row.sessionId, row.remoteDir));
    useStore.getState().removeTransfer(transferId); // the retry runs as a new transfer with its own row
  } catch (e) {
    // The backend forgets failed transfers after 10 minutes or at session close.
    if (toIpcError(e).error.code === "notFound") useStore.getState().patchTransfer(transferId, { retryable: false });
    throw e;
  }
}

export async function cancelTransfer(transferId: string): Promise<void> {
  await getBackend().transferCancel(transferId);
}

/** "Show in tree": opens the folder an upload went to and selects the uploaded file. */
export function showInTree(row: TransferRow): void {
  if (row.remoteDir === null) return;
  const store = useStore.getState();
  const tree = store.workspaces[row.sessionId]?.tree;
  if (!tree) return;
  store.setActivity("files");
  if (!tree.expanded[row.remoteDir] && row.remoteDir !== tree.root) {
    toggleDirectory(row.sessionId, row.remoteDir)
      .then(() => useStore.getState().selectTreeEntry(row.sessionId, joinPath(row.remoteDir ?? "", row.name)))
      .catch(() => undefined);
    return;
  }
  store.selectTreeEntry(row.sessionId, joinPath(row.remoteDir, row.name));
}
