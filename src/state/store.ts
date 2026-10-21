import { create } from "zustand";
import { immer } from "zustand/middleware/immer";
import { createAppSlice } from "./slices/app";
import { createBuffersSlice } from "./slices/buffers";
import { createHostsSlice } from "./slices/hosts";
import { createPromptsSlice } from "./slices/prompts";
import { createSessionsSlice } from "./slices/sessions";
import { createSettingsSlice } from "./slices/settings";
import { createTerminalsSlice } from "./slices/terminals";
import { createTransfersSlice } from "./slices/transfers";
import type { RootState } from "./slices/types";
import { createUiSlice } from "./slices/ui";
import { createUpdatesSlice } from "./slices/updates";
import { createWorkspacesSlice } from "./slices/workspaces";

export type { RootState } from "./slices/types";

/** The root store. Works outside React too: `useStore.getState()`. */
export const useStore = create<RootState>()(
  immer((...a) => ({
    ...createAppSlice(...a),
    ...createSettingsSlice(...a),
    ...createHostsSlice(...a),
    ...createSessionsSlice(...a),
    ...createWorkspacesSlice(...a),
    ...createBuffersSlice(...a),
    ...createTerminalsSlice(...a),
    ...createPromptsSlice(...a),
    ...createTransfersSlice(...a),
    ...createUiSlice(...a),
    ...createUpdatesSlice(...a),
  })),
);

/** Test helper: back to the initial state. */
export function resetStore(): void {
  useStore.setState(useStore.getInitialState(), true);
}
