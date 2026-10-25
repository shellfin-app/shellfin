// Uploads and downloads (docs/ARCHITECTURE.md §6.6). The webview never supplies local paths: the native dialogs and
// the OS drop (a `dropId` token from the backend) do. Progress arrives as `TransferEvent`s on a per-call channel.
import { getBackend } from "../../ipc";
import { toIpcError } from "../../ipc/call";
import type { TransferDecision, TransferEvent } from "../../ipc/types";
import { t } from "../../i18n/translate";
import { replaceTransfersDialog, transferConflictsDialog, transferDecisionDialog } from "../../lib/dialogs";
import { type FolderState, type TransferRow, newFolderState } from "../../state/slices/transfers";
import { useStore } from "../../state/store";
import { joinPath } from "../../lib/remotePath";
import { CONFLICT_DECISIONS, DECISION } from "../../lib/transferDecisions";
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

/** A folder job that waits for the user: a pending decision (before the copy) or deferred conflicts. */
export function needsFolderDecision(row: TransferRow): boolean {
  return row.status === "running" && row.folder !== undefined && (row.folder.pending !== null || row.folder.conflicts !== null);
}

function findRow(transferId: string): TransferRow | undefined {
  return useStore.getState().transfers.find((r) => r.transferId === transferId);
}

/**
 * Opens the decision or conflicts dialog of a folder job shortly after the event, but only when nothing else is open
 * and the job's session is the active tab (same rule as the Replace dialog); otherwise the popover row offers it.
 */
function scheduleFolderDialog(sessionId: string, transferId: string, which: "decision" | "conflicts"): void {
  const key = `${which}\u0000${transferId}`;
  if (promptTimers.has(key)) return;
  promptTimers.set(
    key,
    setTimeout(() => {
      promptTimers.delete(key);
      const s = useStore.getState();
      const row = findRow(transferId);
      const folder = row?.status === "running" ? row.folder : undefined;
      const waiting = which === "decision" ? folder?.pending != null : folder?.conflicts != null;
      if (waiting && s.dialog === null && s.palette === null && s.activeSessionId === sessionId) {
        s.openDialog(which === "decision" ? transferDecisionDialog(sessionId, transferId) : transferConflictsDialog(sessionId, transferId));
      }
    }, PROMPT_DELAY_MS),
  );
}

/** Opens the dialog a waiting folder job needs (popover button). */
export function openFolderDecision(row: TransferRow): void {
  const folder = row.folder;
  if (!folder || row.status !== "running") return;
  if (folder.pending) useStore.getState().openDialog(transferDecisionDialog(row.sessionId, row.transferId));
  else if (folder.conflicts) useStore.getState().openDialog(transferConflictsDialog(row.sessionId, row.transferId));
}

/** The final events of a folder job: nothing is pending any more. */
const FOLDER_IDLE = { pending: null, conflicts: null, current: null, phase: "finished" } as const;

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
        kind: event.kind,
        bytes: 0,
        total: event.total,
        status: "running",
        retryable: false,
        error: null,
        remoteDir: event.direction === "upload" ? remoteDir : null,
        // A retry starts with the remaining bytes and goes straight to copying (no scan, no `scanned`).
        ...(event.kind === "folder" ? { folder: { ...newFolderState(), ...(event.total !== null ? { phase: "copying" as const } : {}) } } : {}),
      });
      break;
    case "scanProgress":
      store.patchFolder(event.transferId, { scan: { files: event.files, dirs: event.dirs, bytes: event.bytes } });
      break;
    case "scanned":
      store.patchTransfer(event.transferId, { total: event.bytes });
      store.patchFolder(event.transferId, {
        phase: "copying",
        scan: { files: event.files, dirs: event.dirs, bytes: event.bytes },
        filesTotal: event.files,
        skipped: event.skipped,
        renamed: event.renamed,
      });
      break;
    case "awaitingDecision":
      store.patchFolder(event.transferId, {
        phase: "awaiting",
        pending: event.request,
        decisionSeq: (findRow(event.transferId)?.folder?.decisionSeq ?? 0) + 1,
      });
      scheduleFolderDialog(sessionId, event.transferId, "decision");
      break;
    case "progress": {
      store.patchTransfer(event.transferId, { bytes: event.bytes, total: event.total });
      // The next `progress` after a decision means copying (the backend sends no "resumed" event).
      const patch: Partial<FolderState> = { phase: "copying", pending: null };
      if (event.filesDone !== undefined) patch.filesDone = event.filesDone;
      if (event.filesTotal !== undefined) patch.filesTotal = event.filesTotal;
      if (event.current !== undefined) patch.current = event.current;
      if (event.failed !== undefined) patch.failed = event.failed;
      if (event.skipped !== undefined) patch.skipped = event.skipped;
      store.patchFolder(event.transferId, patch);
      break;
    }
    case "itemIssue":
      store.addFolderIssue(event.transferId, { itemId: event.itemId, relPath: event.relPath, issue: event.issue });
      break;
    case "conflicts": {
      // Later events only update the count: keep the sample of the first one when they carry none.
      const folder = findRow(event.transferId)?.folder;
      // Sent before the job saw the answer to this round: it must not bring the answered dialog back.
      if (folder?.conflictsAnswered != null && event.round <= folder.conflictsAnswered) break;
      const prev = folder?.conflicts;
      const sample = event.sample.length > 0 || !prev ? event.sample : prev.sample;
      store.patchFolder(event.transferId, {
        conflicts: { count: event.count, sample, round: event.round },
        conflictsSeq: (folder?.conflictsSeq ?? 0) + 1,
      });
      if (!prev) scheduleFolderDialog(sessionId, event.transferId, "conflicts");
      break;
    }
    case "done": {
      store.patchTransfer(event.transferId, { status: "done", bytes: event.bytes, error: null, retryable: event.summary?.retryable === true });
      const summary = event.summary;
      if (summary) {
        store.patchFolder(event.transferId, {
          ...FOLDER_IDLE,
          summary,
          filesDone: summary.filesDone,
          skipped: summary.skipped,
          failed: summary.failed,
          renamed: summary.renamed,
        });
      } else {
        store.patchFolder(event.transferId, FOLDER_IDLE);
      }
      const row = findRow(event.transferId);
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
          kind: "file",
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
      store.patchFolder(event.transferId, FOLDER_IDLE);
      // The Replace dialog belongs to `onConflict = "ask"` only (the backend never reports remoteExists otherwise).
      if (event.error.code === "remoteExists" && event.retryable && askOnConflict()) schedulePrompt(sessionId);
      break;
    }
    case "cancelled":
      store.patchTransfer(event.transferId, { status: "cancelled", retryable: false });
      store.patchFolder(event.transferId, FOLDER_IDLE);
      break;
    case "skipped":
      store.patchTransfer(event.transferId, { status: "skipped", retryable: false });
      store.patchFolder(event.transferId, FOLDER_IDLE);
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

/**
 * "Download folder…": the backend shows the native folder picker (the parent) and copies the tree as one job.
 * Resolves with the transfer id, or `null` if the picker was cancelled.
 */
export async function downloadFolder(sessionId: string, remotePath: string): Promise<string | null> {
  return getBackend().transferDownloadFolder(sessionId, remotePath, t("transfers.downloadFolderDialogTitle"), handler(sessionId, null));
}

/**
 * Answers a waiting folder job. The backend sends no "resumed" event, so the answered request (or the conflicts, for
 * Replace all / Skip all / Replace older) is cleared BEFORE the call: the next `awaitingDecision` (large tree ->
 * root exists -> low space) or new conflicts can arrive while the call runs and must never be wiped. A `conflicts`
 * event of the answered round that was already on its way is dropped by its `round`. When the call
 * fails (the job's decision queue is full, ...), the answered request comes back, unless something newer arrived
 * meanwhile or the job is gone (`notFound`: its final event tells how it ended).
 */
export async function decideTransfer(transferId: string, decision: TransferDecision): Promise<void> {
  const store = useStore.getState();
  const before = findRow(transferId)?.folder;
  const conflict = CONFLICT_DECISIONS.includes(decision);
  const seq = conflict ? before?.conflictsSeq : before?.decisionSeq;
  if (before) {
    // From now on, a late `conflicts` event of the answered round is dropped (see `conflictsAnswered`).
    if (conflict) store.patchFolder(transferId, { conflicts: null, conflictsAnswered: before.conflicts?.round ?? before.conflictsAnswered });
    else store.patchFolder(transferId, { pending: null, phase: decision === DECISION.cancel ? "finished" : "copying" });
  }
  try {
    await getBackend().transferDecide(transferId, decision);
  } catch (e) {
    const row = findRow(transferId);
    const now = row?.folder;
    if (before && now && row.status === "running" && toIpcError(e).error.code !== "notFound") {
      if (conflict && now.conflicts === null && now.conflictsSeq === seq) {
        store.patchFolder(transferId, { conflicts: before.conflicts, conflictsAnswered: before.conflictsAnswered });
      }
      if (!conflict && now.pending === null && now.decisionSeq === seq) store.patchFolder(transferId, { pending: before.pending, phase: before.phase });
    }
    throw e;
  }
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
