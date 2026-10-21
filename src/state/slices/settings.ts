import type { Settings } from "../../ipc/types";
import type { SliceCreator } from "./types";

export interface SettingsSlice {
  settings: Settings | null;
  setSettings(settings: Settings): void;
}

export const createSettingsSlice: SliceCreator<SettingsSlice> = (set) => ({
  settings: null,
  setSettings: (settings) =>
    set((s) => {
      s.settings = settings;
    }),
});
