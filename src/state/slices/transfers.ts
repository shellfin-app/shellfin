import type {
  AppError,
  ConflictSample,
  DecisionRequest,
  FolderSummary,
  ItemIssue,
  SkipCounts,
  TransferDirection,
  TransferKind,
} from "../../ipc/types";
import type { SliceCreator } from "./types";

/** The backend reports at most this many issues of a folder job one by one (§6.6); the store keeps no more either. */
export const MAX_FOLDER_ISSUES = 1000;

/** One skipped or failed entry of a folder job. `relPath` is server text (render through displayPath). */
export interface FolderIssue {
  itemId: number | null;
  relPath: string;
  issue: ItemIssue;
}

/** Folder jobs only (`kind: "folder"`): the phases and counters of one job (§6.6 "Folders"). */
export interface FolderState {
  /** `awaiting`: the job waits for `pending` (root exists, large tree, low space). */
  phase: "scanning" | "awaiting" | "copying" | "finished";
  /** Entries found so far (`scanProgress`, then `scanned`). */
  scan: { files: number; dirs: number; bytes: number };
  filesDone: number;
  filesTotal: number | null;
  /** The name of one active file (server text). */
  current: string | null;
  skipped: SkipCounts;
  failed: number;
  /** Names changed to be valid local names. */
  renamed: number;
  /** The first `MAX_FOLDER_ISSUES` issues; the counters above count all of them. */
  issues: FolderIssue[];
  pending: DecisionRequest | null;
  /**
   * Counts the `awaitingDecision` events of the job. An answer is sent while the next request may already be on its
   * way: only a request with the seq the user answered may be cleared or restored (`decideTransfer`).
   */
  decisionSeq: number;
  /** Files whose local target exists (`onConflict = ask`), set aside until `transfer_decide`. */
  conflicts: { count: number; sample: ConflictSample[]; round: number } | null;
  /** Counts the `conflicts` events (same purpose as `decisionSeq`). */
  conflictsSeq: number;
  /**
   * The last conflict `round` the user answered (`null`: none). A `conflicts` event of that round or an earlier one
   * was sent before the job saw the answer and is dropped (it must not reopen the dialog).
   */
  conflictsAnswered: number | null;
  summary: FolderSummary | null;
}

export interface TransferRow {
  transferId: string;
  sessionId: string;
  /** Remote or local file name (server text for downloads: render through displayName). */
  name: string;
  direction: TransferDirection;
  kind: TransferKind;
  bytes: number;
  total: number | null;
  /** `skipped`: the remote file existed and `transfers.onConflict` is "skip" (finished, nothing written, not retryable). */
  status: "running" | "done" | "failed" | "cancelled" | "skipped";
  /** Failed and retryable, or (folder) done with failed items that `transfer_retry` can copy again. */
  retryable: boolean;
  error: AppError | null;
  /** Upload target directory (the tree refreshes it when the upload is done); `null` for downloads. */
  remoteDir: string | null;
  folder?: FolderState;
}

export function emptySkipCounts(): SkipCounts {
  return { symlink: 0, special: 0, invalidName: 0, lossyName: 0, nameCollision: 0, unknownType: 0, exists: 0 };
}

export function skippedTotal(counts: SkipCounts): number {
  return counts.symlink + counts.special + counts.invalidName + counts.lossyName + counts.nameCollision + counts.unknownType + counts.exists;
}

export function newFolderState(): FolderState {
  return {
    phase: "scanning",
    scan: { files: 0, dirs: 0, bytes: 0 },
    filesDone: 0,
    filesTotal: null,
    current: null,
    skipped: emptySkipCounts(),
    failed: 0,
    renamed: 0,
    issues: [],
    pending: null,
    decisionSeq: 0,
    conflicts: null,
    conflictsSeq: 0,
    conflictsAnswered: null,
    summary: null,
  };
}

export interface TransfersSlice {
  transfers: TransferRow[];
  upsertTransfer(row: TransferRow): void;
  patchTransfer(transferId: string, patch: Partial<Omit<TransferRow, "transferId">>): void;
  /** Folder rows: merges `patch` into `row.folder` (no-op for file rows). */
  patchFolder(transferId: string, patch: Partial<FolderState>): void;
  /** Folder rows: appends an issue unless `MAX_FOLDER_ISSUES` are listed already. */
  addFolderIssue(transferId: string, issue: FolderIssue): void;
  removeTransfer(transferId: string): void;
  clearFinishedTransfers(): void;
}

export const createTransfersSlice: SliceCreator<TransfersSlice> = (set) => ({
  transfers: [],
  upsertTransfer: (row) =>
    set((s) => {
      const i = s.transfers.findIndex((t) => t.transferId === row.transferId);
      if (i >= 0) s.transfers[i] = row;
      else s.transfers.push(row);
    }),
  patchTransfer: (transferId, patch) =>
    set((s) => {
      const row = s.transfers.find((t) => t.transferId === transferId);
      if (row) Object.assign(row, patch);
    }),
  patchFolder: (transferId, patch) =>
    set((s) => {
      const folder = s.transfers.find((t) => t.transferId === transferId)?.folder;
      if (folder) Object.assign(folder, patch);
    }),
  addFolderIssue: (transferId, issue) =>
    set((s) => {
      const folder = s.transfers.find((t) => t.transferId === transferId)?.folder;
      if (folder && folder.issues.length < MAX_FOLDER_ISSUES) folder.issues.push(issue);
    }),
  removeTransfer: (transferId) =>
    set((s) => {
      s.transfers = s.transfers.filter((t) => t.transferId !== transferId);
    }),
  // Running transfers and work that can still be retried stay; everything else is history.
  clearFinishedTransfers: () =>
    set((s) => {
      s.transfers = s.transfers.filter((t) => t.status === "running" || (t.retryable && (t.status === "failed" || t.status === "done")));
    }),
});
