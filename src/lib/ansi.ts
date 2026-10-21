// Terminal-inserted lines (docs/ARCHITECTURE.md §4.5, §8.2.2): translated text with dim ANSI styling, never
// containing server text.

/** CAN returns xterm's parser to the ground state from any OSC/DCS/CSI state; then SGR reset. */
export const CAN_SGR_RESET = "\x18\x1b[0m";

const DIM = "\x1b[2m";
const RESET = "\x1b[0m";

/** A dim, newline-delimited notice line to write into a terminal. */
export function dimNotice(text: string): string {
  return `\r\n${DIM}${text}${RESET}\r\n`;
}
