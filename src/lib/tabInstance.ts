/**
 * The instance number (>= 1) for a session with `label`: `preferred` when no other open session with the same
 * `label` uses it (a reopened tab keeps its number), otherwise the smallest free one.
 */
export function nextInstance(open: readonly { label: string; instance: number }[], label: string, preferred?: number): number {
  const used = new Set(open.filter((e) => e.label === label).map((e) => e.instance));
  if (preferred !== undefined && preferred >= 1 && !used.has(preferred)) return preferred;
  let n = 1;
  while (used.has(n)) n += 1;
  return n;
}
