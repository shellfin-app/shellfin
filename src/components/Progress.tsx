/** Thin progress bar (`value` 0..1). Decorative: the surrounding text carries the information. */
export function Progress({ value }: { value: number }) {
  const pct = Math.max(0, Math.min(1, value)) * 100;
  return (
    <div className="progress" aria-hidden="true">
      <div className="progress__bar" style={{ width: `${pct}%` }} />
    </div>
  );
}
