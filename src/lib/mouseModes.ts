// DECSET mouse-tracking modes, for `terminal.mouseReporting = false` (the terminal registry swallows them so a remote
// program can never take the mouse away from text selection).

/** 9 X10, 1000 normal, 1001 highlight, 1002 button-event, 1003 any-event; 1005/1006/1015/1016 are encodings. */
export const MOUSE_MODES: readonly number[] = [9, 1000, 1001, 1002, 1003, 1005, 1006, 1015, 1016];

export interface SplitModes {
  mouse: number[];
  rest: (number | number[])[];
}

/** Separates the mouse modes from the other parameters of a `CSI ? Pm h` sequence. */
export function splitMouseModes(params: readonly (number | number[])[]): SplitModes {
  const mouse: number[] = [];
  const rest: (number | number[])[] = [];
  for (const p of params) {
    if (typeof p === "number" && MOUSE_MODES.includes(p)) mouse.push(p);
    else rest.push(p);
  }
  return { mouse, rest };
}

/**
 * `CSI ? Pm l` for the given mouse modes, or `""` for none. A sequence that mixes mouse modes with others is applied
 * by xterm as a whole (so the other modes keep their order relative to the output around them) and then undone for
 * the mouse modes with this.
 */
export function resetModesSequence(mouse: readonly number[]): string {
  return mouse.length === 0 ? "" : `\u001b[?${mouse.join(";")}l`;
}

/** Resets every mouse mode in the local emulator (applied when mouse reporting is switched off live). */
export const RESET_MOUSE_MODES = MOUSE_MODES.map((m) => `\u001b[?${m}l`).join("");
