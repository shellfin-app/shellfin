// Terminal paste sanitiser (docs/ARCHITECTURE.md §8.6, §9.5). Pasted text is attacker-influenced (web pages put
// ESC[201~ into the clipboard to break out of bracketed paste and run commands). Everything that reaches the
// remote shell through a paste event passes through `sanitizePaste` exactly once.

const TAB = 0x09;
const LF = 0x0a;
const CR = 0x0d;

/**
 * Removes every control character except TAB, LF and CR: ESC (so `ESC[201~` cannot close a bracketed paste),
 * C1 controls including 0x9B (single-byte CSI), NUL, BEL, BS, DEL ... Compared numerically so formatters cannot
 * strip or rewrite invisible characters in this file.
 */
export function sanitizePaste(text: string): string {
  let out = "";
  for (const ch of text) {
    const code = ch.codePointAt(0) ?? 0;
    const isControl = code < 0x20 || (code >= 0x7f && code <= 0x9f);
    if (isControl && code !== TAB && code !== LF && code !== CR) continue;
    out += ch;
  }
  return out;
}

/** Number of lines in `text` (a trailing line break does not start a new line). 0 for empty text. */
export function pasteLineCount(text: string): number {
  if (text === "") return 0;
  const lines = text.split(/\r\n|\r|\n/);
  if (lines[lines.length - 1] === "") lines.pop();
  return lines.length;
}

/** Outside bracketed-paste mode a newline is executed by the shell: ask first (§8.6). */
export function needsPasteConfirm(text: string, bracketedPasteMode: boolean): boolean {
  return !bracketedPasteMode && /[\r\n]/.test(text);
}

export interface PasteTarget {
  /** Sends the (already sanitised) text to the terminal (`Terminal.paste`). */
  paste(text: string): void;
  bracketedPasteMode(): boolean;
  /** Whether the terminal currently accepts input (running shell). */
  acceptsInput(): boolean;
  /** Multi-line confirmation; resolves `true` to go ahead. */
  confirm(text: string): Promise<boolean>;
  /** `terminal.multilinePasteWarning`: ask at all? (default yes) */
  warnMultiline?(): boolean;
}

/** Sanitises `raw` and pastes it (after the multi-line confirmation if needed). Every paste path ends here. */
export function pasteText(target: PasteTarget, raw: string): void {
  if (!target.acceptsInput()) return;
  const clean = sanitizePaste(raw);
  if (clean === "") return;
  if (target.warnMultiline?.() === false || !needsPasteConfirm(clean, target.bracketedPasteMode())) {
    target.paste(clean);
    return;
  }
  void target.confirm(clean).then((ok) => {
    if (ok && target.acceptsInput()) target.paste(clean);
  });
}

/**
 * The single paste listener of a terminal. Register it ONCE on the terminal's host element with
 * `{ capture: true }`: it runs before xterm's own listeners on the textarea/element, cancels the event
 * (`preventDefault` + `stopImmediatePropagation`) and pastes the sanitised text itself, so xterm's unsanitised
 * handlers can never see the clipboard data.
 */
export function createPasteListener(target: PasteTarget): (event: ClipboardEvent) => void {
  return (event) => {
    event.preventDefault();
    event.stopImmediatePropagation();
    pasteText(target, event.clipboardData?.getData("text/plain") ?? "");
  };
}
