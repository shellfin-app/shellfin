/** Longest OSC window title kept (a program may send megabytes). */
export const MAX_TITLE_LENGTH = 1024;

export interface TitleCoalescer {
  /** The latest title wins; the store is patched once, on the next animation frame. */
  push(title: string): void;
  /** Drops a pending title (the terminal is being disposed). */
  cancel(): void;
}

/**
 * Coalesces OSC title updates: a program that sets the title in a loop produces one `apply` per animation
 * frame with the last title, instead of one store update per sequence. An empty title means "no title" (`null`).
 */
export function createTitleCoalescer(apply: (title: string | null) => void): TitleCoalescer {
  let pending: string | null | undefined;
  let frame = 0;
  const flush = () => {
    frame = 0;
    const title = pending;
    pending = undefined;
    if (title !== undefined) apply(title);
  };
  return {
    push(title) {
      pending = title === "" ? null : title.slice(0, MAX_TITLE_LENGTH);
      if (frame === 0) frame = requestAnimationFrame(flush);
    },
    cancel() {
      if (frame !== 0) cancelAnimationFrame(frame);
      frame = 0;
      pending = undefined;
    },
  };
}
