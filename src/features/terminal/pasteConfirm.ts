// Multi-line paste confirmation (§8.6): a store-driven dialog; the terminal registry awaits the answer.
import { CONFIRM_PASTE_KIND } from "../../lib/dialogs";
import { pasteLineCount } from "../../lib/paste";
import { displayServerText } from "../../lib/serverText";
import { useStore } from "../../state/store";

const PREVIEW_LINES = 5;

let pending: ((ok: boolean) => void) | null = null;

/** Opens the confirmation for `text` (already sanitised). Resolves `true` when the user pastes anyway. */
export function requestPasteConfirm(text: string): Promise<boolean> {
  pending?.(false); // a newer request supersedes an unanswered one
  return new Promise<boolean>((resolve) => {
    pending = resolve;
    const lines = text
      .split(/\r\n|\r|\n/)
      .slice(0, PREVIEW_LINES)
      .map((l) => displayServerText(l, 120));
    useStore.getState().openDialog({ kind: CONFIRM_PASTE_KIND, lines: pasteLineCount(text), preview: lines.join("\n") });
    // openDialog is ignored while the quit confirmation owns the screen.
    if (useStore.getState().dialog?.kind !== CONFIRM_PASTE_KIND) answerPasteConfirm(false);
  });
}

export function answerPasteConfirm(ok: boolean): void {
  useStore.getState().closeDialog(CONFIRM_PASTE_KIND);
  const resolve = pending;
  pending = null;
  resolve?.(ok);
}
