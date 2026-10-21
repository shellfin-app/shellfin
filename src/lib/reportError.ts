import { toIpcError } from "../ipc/call";
import { useStore } from "../state/store";

/**
 * The single sink for failures of user-initiated actions: logs and shows a dismissible notice
 * (<Notices/>). Never throws. Use as `promise.catch(reportError)` instead of a bare `void promise`.
 */
export function reportError(e: unknown): void {
  const error = toIpcError(e);
  console.error(error.error.code, error.error.detail ?? "");
  useStore.getState().pushNotice(error.error);
}

/** Fire-and-forget with error reporting. */
export function guard(promise: Promise<unknown>): void {
  promise.catch(reportError);
}
