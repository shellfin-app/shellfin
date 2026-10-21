import { useStore } from "../../state/store";
import { HostKeyDialog } from "./HostKeyDialog";
import { KbdInteractiveDialog } from "./KbdInteractiveDialog";
import { SecretPrompt } from "./SecretPrompt";
import { visiblePromptSession } from "./promptVisibility";
import "./session.css";

/**
 * Shows the pending prompt of the active session (or of a session the user explicitly asked for). One prompt
 * per session (§5.4); keyed by prompt id so every new prompt (e.g. a retry with `lastError`) starts with clean
 * local state. Background prompts are announced by a tab badge, never by a focus-stealing dialog.
 */
export function PromptHost() {
  const prompts = useStore((s) => s.prompts);
  const quitOpen = useStore((s) => s.dialog?.kind === "quit");
  const shown = useStore(visiblePromptSession);
  // The quit confirmation takes precedence over everything (§8.9); prompts wait behind it.
  const sessionId = quitOpen ? null : shown;
  const prompt = sessionId ? prompts[sessionId] : undefined;
  if (!sessionId || !prompt) return null;

  switch (prompt.kind) {
    case "hostKeyUnknown":
      return <HostKeyDialog key={prompt.id} sessionId={sessionId} prompt={prompt} />;
    case "passphrase":
    case "password":
      return <SecretPrompt key={`${prompt.id}:${prompt.attemptsLeft}`} sessionId={sessionId} prompt={prompt} />;
    case "keyboardInteractive":
      return <KbdInteractiveDialog key={prompt.id} sessionId={sessionId} prompt={prompt} />;
  }
}
