import type { AppError, UpdateInfo } from "../../ipc/types";
import type { SliceCreator } from "./types";

/**
 * Updater: what the update dialog shows.
 * - `available`: the offer (Update and restart / Later / Skip this version, or Download for a .deb/.rpm install);
 * - `downloading`: progress, cancellable;
 * - `restarting`: downloaded and verified; the quit flow runs (it may ask about unsaved changes);
 * - `failed`: the download, verification or install failed (`error`, rendered through `errors.<code>`).
 */
export type UpdatePhase = "available" | "downloading" | "restarting" | "failed";

export interface UpdateUi {
  info: UpdateInfo;
  phase: UpdatePhase;
  downloaded: number;
  total: number | null;
  error: AppError | null;
}

export interface UpdatesSlice {
  /** The update being offered or installed; `null` = nothing to show. */
  update: UpdateUi | null;
  setUpdateInfo(info: UpdateInfo | null): void;
  setUpdatePhase(phase: UpdatePhase, error?: AppError | null): void;
  setUpdateProgress(downloaded: number, total: number | null): void;
}

export const createUpdatesSlice: SliceCreator<UpdatesSlice> = (set) => ({
  update: null,
  setUpdateInfo: (info) =>
    set((s) => {
      s.update = info ? { info, phase: "available", downloaded: 0, total: null, error: null } : null;
    }),
  setUpdatePhase: (phase, error = null) =>
    set((s) => {
      if (!s.update) return;
      s.update.phase = phase;
      s.update.error = error;
      if (phase === "available") {
        s.update.downloaded = 0;
        s.update.total = null;
      }
    }),
  setUpdateProgress: (downloaded, total) =>
    set((s) => {
      if (!s.update) return;
      s.update.downloaded = downloaded;
      s.update.total = total;
    }),
});
