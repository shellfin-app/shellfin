// `editor.detectIndentation`: tabs or spaces, and the width, from the file's own content when that is unambiguous.
// Pure text analysis, no CodeMirror.

export type DetectedIndent = { tabs: true } | { tabs: false; width: number };

const MAX_LINES = 2000;
const MIN_EVIDENCE = 3;
const WIDTHS = [2, 3, 4, 8];

/**
 * `null` when the text gives no clear answer (too few indented lines, tabs and spaces mixed about equally, no
 * dominant indentation step), so the caller keeps the settings.
 */
export function detectIndent(text: string): DetectedIndent | null {
  let tabLines = 0;
  let spaceLines = 0;
  const steps = new Map<number, number>();
  let previous = 0; // indentation (in columns) of the previous space-indented line; tabs reset it
  let lineCount = 0;
  let start = 0;
  while (start <= text.length && lineCount < MAX_LINES) {
    let end = text.indexOf("\n", start);
    if (end < 0) end = text.length;
    lineCount += 1;
    const first = text.charCodeAt(start);
    if (end > start && first === 0x09) {
      tabLines += 1;
      previous = 0;
    } else if (end > start && first === 0x20) {
      let n = 0;
      while (start + n < end && text.charCodeAt(start + n) === 0x20) n += 1;
      const blank = start + n >= end || (text.charCodeAt(start + n) === 0x0d && start + n + 1 >= end);
      // A single leading space is usually a block comment continuation (` * text`), a blank line proves nothing.
      if (!blank && n >= 2) {
        spaceLines += 1;
        const step = Math.abs(n - previous);
        if (step > 0) steps.set(step, (steps.get(step) ?? 0) + 1);
        previous = n;
      }
    } else if (end > start && text.charCodeAt(start) !== 0x0d) {
      previous = 0;
    }
    start = end + 1;
  }

  if (tabLines >= MIN_EVIDENCE && tabLines >= spaceLines * 3) return { tabs: true };
  if (spaceLines < MIN_EVIDENCE || spaceLines < tabLines * 3) return null;

  let total = 0;
  for (const count of steps.values()) total += count;
  let best = 0;
  let bestCount = 0;
  for (const w of WIDTHS) {
    // A step of 2*w also counts for w (a block that goes two levels deeper at once).
    let count = steps.get(w) ?? 0;
    for (const [step, c] of steps) if (step !== w && step % w === 0 && step <= w * 3) count += c / 2;
    if (count > bestCount) {
      best = w;
      bestCount = count;
    }
  }
  if (best === 0 || bestCount < 2 || bestCount < total * 0.5) return null;
  return { tabs: false, width: best };
}
