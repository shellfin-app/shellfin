// Mock of the backend's terminal output pipeline (docs/ARCHITECTURE.md §4.5): a per-terminal backlog, a window of
// unacknowledged bytes, overflow handling with `outputDropped`, and a generator for flood output.
import type { TermEvent } from "../types";
import type { TermMessage } from "../backend";

export const FLOW_WINDOW = 4 * 1024 * 1024; // max unacknowledged bytes in flight
export const BACKLOG_LIMIT = 8 * 1024 * 1024; // beyond this, new output is discarded
export const BACKLOG_RESUME = 1024 * 1024; // outputDropped is sent once the backlog is below this again
export const FLOOD_CHUNK = 64 * 1024;

type Item = { kind: "bytes"; bytes: Uint8Array } | { kind: "event"; event: TermEvent };

export class TerminalOutput {
  private items: Item[] = [];
  private backlogBytes = 0;
  private unackedBytes = 0;
  private droppedBytes = 0;

  constructor(private readonly deliver: (message: TermMessage) => void) {}

  /** Output from the remote side. Dropped (and counted) while the backlog is full. */
  push(bytes: Uint8Array): void {
    if (this.backlogBytes + bytes.length > BACKLOG_LIMIT) {
      this.droppedBytes += bytes.length;
      return;
    }
    this.items.push({ kind: "bytes", bytes: bytes.slice() });
    this.backlogBytes += bytes.length;
    this.flush();
  }

  /** A control event, delivered in order behind pending output. */
  event(event: TermEvent): void {
    this.items.push({ kind: "event", event });
    this.flush();
  }

  /** `term_ack`. */
  ack(bytes: number): void {
    this.unackedBytes = Math.max(0, this.unackedBytes - bytes);
    this.flush();
  }

  get unacked(): number {
    return this.unackedBytes;
  }

  get backlog(): number {
    return this.backlogBytes;
  }

  private flush(): void {
    for (;;) {
      const item = this.items[0];
      if (!item) break;
      if (item.kind === "bytes") {
        if (this.unackedBytes >= FLOW_WINDOW) break;
        this.items.shift();
        this.backlogBytes -= item.bytes.length;
        this.unackedBytes += item.bytes.length;
        this.deliver(item.bytes.slice().buffer);
      } else {
        this.items.shift();
        this.deliver(item.event);
      }
    }
    if (this.droppedBytes > 0 && this.backlogBytes < BACKLOG_RESUME) {
      const bytes = this.droppedBytes;
      this.droppedBytes = 0;
      this.deliver({ type: "outputDropped", bytes });
    }
  }
}

const encoder = new TextEncoder();

/**
 * `totalBytes` of colourful line output in 64 KiB chunks. Chunk boundaries deliberately fall inside escape
 * sequences, so a dropped chunk can leave the frontend's parser mid-sequence (the reason for CAN, §4.5).
 */
export function floodChunks(totalBytes: number): Uint8Array[] {
  const lines: string[] = [];
  for (let i = 0; i < 7; i++) {
    lines.push(`\x1b[3${i + 1};1m${String(i).padStart(6, "0")}\x1b[0m the quick brown fox jumps over the lazy dog \x1b[2m#${i}\x1b[0m\r\n`);
  }
  const unit = lines.join("");
  let text = "";
  while (text.length < FLOOD_CHUNK) text += unit;
  const base = encoder.encode(text).subarray(0, FLOOD_CHUNK);
  const chunks: Uint8Array[] = [];
  for (let sent = 0; sent < totalBytes; sent += FLOOD_CHUNK) chunks.push(base);
  return chunks;
}
