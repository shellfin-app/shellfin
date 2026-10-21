import type { AppBootstrap, AppError, Platform } from "../../ipc/types";
import type { SliceCreator } from "./types";

export type QuitState = "idle" | "requested" | "confirming";

export interface AppSlice {
  /** True once `app_frontend_ready` has resolved. */
  ready: boolean;
  appVersion: string;
  platform: Platform;
  warnings: AppError[];
  quit: QuitState;
  /** Set when `app_frontend_ready` (or backend selection) failed; the UI shows a retry screen. */
  bootError: AppError | null;
  setBootError(error: AppError | null): void;
  setBootstrap(bootstrap: AppBootstrap): void;
  setQuitState(state: QuitState): void;
}

export const createAppSlice: SliceCreator<AppSlice> = (set) => ({
  ready: false,
  appVersion: "",
  platform: "windows",
  warnings: [],
  quit: "idle",
  bootError: null,
  setBootError: (error) =>
    set((s) => {
      s.bootError = error;
    }),
  setBootstrap: (b) =>
    set((s) => {
      s.ready = true;
      s.bootError = null;
      s.appVersion = b.appVersion;
      s.platform = b.platform;
      s.warnings = b.warnings;
      s.settings = b.settings;
      s.sshConfig = b.sshConfig;
      s.bookmarks = b.bookmarks;
      s.recents = b.recents;
      s.windowState = b.window;
    }),
  setQuitState: (state) =>
    set((s) => {
      s.quit = state;
    }),
});
