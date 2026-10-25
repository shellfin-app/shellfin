import { isUnexpectedError, reportUnexpected, sourceFromAppError } from "../features/errorReport/errorReport";
import { toIpcError } from "../ipc/call";
import { useStore } from "../state/store";

/**
 * The single sink for failures of user-initiated actions: logs and shows a dismissible notice
 * (<Notices/>). Never throws. Use as `promise.catch(reportError)` instead of a bare `void promise`.
 * A backend `internal` / `bug` error is a defect, not something the user can act on: it opens the error details
 * dialog instead (§8.14). Rejections that did not come from the backend (a JS exception, a denied clipboard) keep
 * the notice, which offers "Details…".
 */
export function reportError(e: unknown): void {
  const error = toIpcError(e);
  console.error(error.error.code, error.error.detail ?? "");
  if (!error.wrapped && isUnexpectedError(error.error)) {
    reportUnexpected(sourceFromAppError(error.error));
    return;
  }
  useStore.getState().pushNotice(error.error);
}

/** Fire-and-forget with error reporting. */
export function guard(promise: Promise<unknown>): void {
  promise.catch(reportError);
}
