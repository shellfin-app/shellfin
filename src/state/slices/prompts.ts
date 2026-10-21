import type { Prompt } from "../../ipc/types";
import type { SliceCreator } from "./types";

export interface PromptsSlice {
  /** Current prompt per session (one open prompt per session, §5.4). Answers never enter the store. */
  prompts: Record<string, Prompt>;
  clearPrompt(sessionId: string): void;
}

export const createPromptsSlice: SliceCreator<PromptsSlice> = (set) => ({
  prompts: {},
  clearPrompt: (sessionId) =>
    set((s) => {
      delete s.prompts[sessionId];
    }),
});
