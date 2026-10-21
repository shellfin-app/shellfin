import type { RootState } from "../../state/store";

type PromptView = Pick<RootState, "prompts" | "activeSessionId" | "promptSession">;

/**
 * Which session's prompt is shown as a dialog right now. Only the ACTIVE session's prompt opens by
 * itself; a prompt of a background tab (or of a test-connection probe) never takes focus: it waits behind a badge
 * until the user activates that tab or explicitly asks to answer it (`promptSession`).
 */
export function visiblePromptSession(s: PromptView): string | null {
  if (s.activeSessionId && s.prompts[s.activeSessionId]) return s.activeSessionId;
  if (s.promptSession && s.prompts[s.promptSession]) return s.promptSession;
  return null;
}

/** True while any modal surface (dialog, palette, prompt) is open: global shortcuts must stay inert. */
export function isModalOpen(s: PromptView & Pick<RootState, "dialog" | "palette">): boolean {
  return s.dialog !== null || s.palette !== null || visiblePromptSession(s) !== null;
}
