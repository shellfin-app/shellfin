/** How long (ms) a newly opened authentication prompt ignores the keyboard. Tests set it to 0. */
let guardMs = 500;

export function promptInputGuardMs(): number {
  return guardMs;
}

export function setPromptInputGuardMs(ms: number): void {
  guardMs = ms;
}
