// A small line diff for the conflict dialog's "compare-lite" view (docs/ARCHITECTURE.md §6.4). Not a full diff
// viewer: counts, plus the first changed lines. Pure functions, no UI text.

export interface DiffLine {
  /** `del`: only in the first text (your version), `add`: only in the second (the server's). */
  op: "add" | "del";
  /** 1-based line number in the text the line belongs to. */
  line: number;
  text: string;
}

export interface DiffResult {
  added: number;
  removed: number;
  /** The first changed lines in document order; empty when `exact` is false. */
  lines: DiffLine[];
  /** More changed lines exist than `lines` holds. */
  truncated: boolean;
  /** False when the texts were too long for a line-by-line comparison (counts are then approximate). */
  exact: boolean;
}

/** Above this many lines per side (after trimming the common start and end) only approximate counts are given. */
const MAX_LINES = 2000;

function splitLines(text: string): string[] {
  const lines = text.split("\n");
  if (lines.length > 1 && lines[lines.length - 1] === "") lines.pop(); // a final newline is not an extra line
  return text === "" ? [] : lines;
}

/** Approximate counts for very large texts: lines present on one side only (multiset difference). */
function approximate(a: string[], b: string[]): DiffResult {
  const counts = new Map<string, number>();
  for (const line of a) counts.set(line, (counts.get(line) ?? 0) + 1);
  let added = 0;
  for (const line of b) {
    const n = counts.get(line) ?? 0;
    if (n > 0) counts.set(line, n - 1);
    else added += 1;
  }
  let removed = 0;
  for (const n of counts.values()) removed += n;
  return { added, removed, lines: [], truncated: false, exact: false };
}

/** Diffs `mine` (the buffer) against `theirs` (the server). Lines are compared as whole strings. */
export function diffLines(mine: string, theirs: string, maxPreview = 40): DiffResult {
  const a = splitLines(mine);
  const b = splitLines(theirs);
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start += 1;
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA -= 1;
    endB -= 1;
  }
  const midA = a.slice(start, endA);
  const midB = b.slice(start, endB);
  if (midA.length > MAX_LINES || midB.length > MAX_LINES) return approximate(midA, midB);

  const n = midA.length;
  const m = midB.length;
  // lcs[i][j] = LCS length of midA[i..] and midB[j..]
  const width = m + 1;
  const lcs = new Uint16Array((n + 1) * width);
  for (let i = n - 1; i >= 0; i -= 1) {
    for (let j = m - 1; j >= 0; j -= 1) {
      lcs[i * width + j] =
        midA[i] === midB[j]
          ? (lcs[(i + 1) * width + j + 1] ?? 0) + 1
          : Math.max(lcs[(i + 1) * width + j] ?? 0, lcs[i * width + j + 1] ?? 0);
    }
  }

  const lines: DiffLine[] = [];
  let added = 0;
  let removed = 0;
  let i = 0;
  let j = 0;
  const push = (line: DiffLine) => {
    if (lines.length < maxPreview) lines.push(line);
  };
  while (i < n || j < m) {
    if (i < n && j < m && midA[i] === midB[j]) {
      i += 1;
      j += 1;
    } else if (j >= m || (i < n && (lcs[(i + 1) * width + j] ?? 0) >= (lcs[i * width + j + 1] ?? 0))) {
      removed += 1;
      push({ op: "del", line: start + i + 1, text: midA[i] ?? "" });
      i += 1;
    } else {
      added += 1;
      push({ op: "add", line: start + j + 1, text: midB[j] ?? "" });
      j += 1;
    }
  }
  return { added, removed, lines, truncated: added + removed > lines.length, exact: true };
}

/** Number of lines in a text (a trailing newline does not start a new line). */
export function countLines(text: string): number {
  return splitLines(text).length;
}
