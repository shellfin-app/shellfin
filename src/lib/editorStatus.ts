import type { BufferMeta } from "../state/slices/buffers";

/** What the right side of the breadcrumb says about the buffer. The text never claims a save that did not happen. */
export type EditorStatus =
  | { kind: "saving" }
  | { kind: "conflict" }
  | { kind: "pendingSave" }
  /** Unsaved edits and the connection is down. */
  | { kind: "offlineDirty" }
  | { kind: "unsaved"; savedAtMs: number | null }
  | { kind: "saved"; savedAtMs: number }
  | { kind: "none" };

export function editorStatus(b: BufferMeta, connected: boolean): EditorStatus {
  if (b.saving) return { kind: "saving" };
  if (b.conflict) return { kind: "conflict" };
  if (b.pendingSave) return { kind: "pendingSave" };
  if (b.dirty) return connected ? { kind: "unsaved", savedAtMs: b.savedAtMs } : { kind: "offlineDirty" };
  if (b.savedAtMs !== null && b.load === "ready") return { kind: "saved", savedAtMs: b.savedAtMs };
  return { kind: "none" };
}
