// Output flow control (docs/ARCHITECTURE.md §4.5): the frontend reports processed bytes to the backend in
// batches of at least 256 KiB, counted from xterm's `write(data, callback)` completions.

export const ACK_THRESHOLD = 256 * 1024;

export class AckTracker {
  private pending = 0;

  constructor(private readonly threshold = ACK_THRESHOLD) {}

  /** `bytes` were processed by xterm. Returns the number of bytes to acknowledge now, or 0. */
  processed(bytes: number): number {
    this.pending += bytes;
    if (this.pending < this.threshold) return 0;
    const n = this.pending;
    this.pending = 0;
    return n;
  }

  get unacked(): number {
    return this.pending;
  }
}
