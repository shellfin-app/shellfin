import type { AppError, TransferDirection } from "../../ipc/types";
import type { SliceCreator } from "./types";

export interface TransferRow {
  transferId: string;
  sessionId: string;
  /** Remote or local file name (server text for downloads: render through displayName). */
  name: string;
  direction: TransferDirection;
  bytes: number;
  total: number | null;
  /** `skipped`: the remote file existed and `transfers.onConflict` is "skip" (finished, nothing written, not retryable). */
  status: "running" | "done" | "failed" | "cancelled" | "skipped";
  retryable: boolean;
  error: AppError | null;
  /** Upload target directory (the tree refreshes it when the upload is done); `null` for downloads. */
  remoteDir: string | null;
}

export interface TransfersSlice {
  transfers: TransferRow[];
  upsertTransfer(row: TransferRow): void;
  patchTransfer(transferId: string, patch: Partial<Omit<TransferRow, "transferId">>): void;
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
  removeTransfer: (transferId) =>
    set((s) => {
      s.transfers = s.transfers.filter((t) => t.transferId !== transferId);
    }),
  // Running transfers and failures that can still be retried stay; everything else is history.
  clearFinishedTransfers: () =>
    set((s) => {
      s.transfers = s.transfers.filter((t) => t.status === "running" || (t.status === "failed" && t.retryable));
    }),
});
