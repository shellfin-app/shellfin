// Error details dialog: building the report source, deciding when it opens by itself, and the global handlers
// (docs/ARCHITECTURE.md §8.14). Only unexpected failures open it automatically; every other error keeps its toast
// or inline UI and offers "Details…".
import i18next from "i18next";
import { errorMessage } from "../../i18n/errorMessage";
import { IpcError, isAppError } from "../../ipc/call";
import type { AppError } from "../../ipc/types";
import { DIALOG_KIND } from "../../lib/dialogs";
import type { ErrorReportSource } from "../../state/slices/ui";
import { useStore } from "../../state/store";
import { isModalOpen } from "../session/promptVisibility";

/** Code of failures that happen in the webview itself (render crash, uncaught exception, unhandled rejection). */
export const FRONTEND_CODE = "frontend";

/** Backend codes that mean "a bug in Shellfin": `internal` and the panic code `bug` (§4.2). */
export function isUnexpectedError(error: AppError): boolean {
  return error.code === "internal" || error.code === "bug";
}

export function sourceFromAppError(
  error: AppError,
  extra: { context?: string; stack?: string; occurredAtMs?: number } = {},
): ErrorReportSource {
  const context = error.code === "bug" ? error.params.command : extra.context;
  return {
    code: error.code,
    ...("params" in error ? { params: error.params } : {}),
    ...(error.detail !== undefined ? { detail: error.detail } : {}),
    message: errorMessage(error),
    ...(context !== undefined ? { context } : {}),
    ...(extra.stack !== undefined ? { stack: extra.stack } : {}),
    occurredAtMs: extra.occurredAtMs ?? Date.now(),
  };
}

/** A failure of the webview code: no backend error behind it. */
export function sourceFromFrontend(detail: string, context: string, stack?: string): ErrorReportSource {
  return {
    code: FRONTEND_CODE,
    detail,
    message: i18next.t("errorReport.frontendMessage"),
    context,
    ...(stack !== undefined ? { stack } : {}),
    occurredAtMs: Date.now(),
  };
}

/** Opens the dialog for `source` (`null` = environment only). An explicit user action: it replaces other dialogs. */
export function openErrorDetails(source: ErrorReportSource | null): void {
  useStore.getState().openDialog({ kind: DIALOG_KIND.errorReport, source, more: 0, auto: false });
}

/**
 * An unexpected failure. Opens the dialog if nothing else is in the way; while it is open, repeats only bump its
 * counter; while another dialog, the palette or a prompt is open the user is not interrupted: an error notice with
 * "Details…" is queued instead. `force`: the whole UI that showed those surfaces is gone (the app-wide ErrorBoundary
 * caught a crash), so the stale dialog/palette state is replaced.
 */
export function reportUnexpected(source: ErrorReportSource, force = false): void {
  const s = useStore.getState();
  const dialog = s.dialog;
  if (dialog?.kind === "errorReport") {
    s.openDialog({ ...dialog, more: dialog.more + 1 });
    return;
  }
  if (force) {
    s.closePalette();
    s.openDialog({ kind: DIALOG_KIND.errorReport, source, more: 0, auto: true });
    return;
  }
  if (isModalOpen(s)) {
    const error: AppError = { code: "internal", ...(source.detail !== undefined ? { detail: source.detail } : {}) };
    s.pushNotice(error, "error", source);
    return;
  }
  s.openDialog({ kind: DIALOG_KIND.errorReport, source, more: 0, auto: true });
}

// The browser reports this one for harmless layout feedback loops; it is not a failure.
const IGNORED_MESSAGES = ["ResizeObserver loop"];
/** English developer detail (goes to the report, never to the UI) for a rejection that carries no message. */
const UNKNOWN_DETAIL = "unknown error";

function describe(reason: unknown): { detail: string; stack?: string } {
  if (reason instanceof Error) return { detail: reason.message, ...(reason.stack ? { stack: reason.stack } : {}) };
  if (typeof reason === "string") return { detail: reason };
  return { detail: UNKNOWN_DETAIL };
}

let handlersInstalled = false;

/** Installs the `error` and `unhandledrejection` listeners once (bootstrap). */
export function installGlobalErrorHandlers(): void {
  if (handlersInstalled) return;
  handlersInstalled = true;
  window.addEventListener("error", (event) => {
    const message = event.message ?? "";
    if (IGNORED_MESSAGES.some((m) => message.startsWith(m))) return;
    const { detail, stack } = event.error !== undefined && event.error !== null ? describe(event.error) : { detail: message };
    reportUnexpected(sourceFromFrontend(detail, "window.error", stack));
  });
  window.addEventListener("unhandledrejection", (event) => {
    const reason: unknown = event.reason;
    const backendError = reason instanceof IpcError && !reason.wrapped ? reason.error : isAppError(reason) ? reason : null;
    if (backendError) {
      if (backendError.code === "cancelled") return;
      reportUnexpected(sourceFromAppError(backendError, { context: "unhandledrejection", ...(reason instanceof Error && reason.stack ? { stack: reason.stack } : {}) }));
      return;
    }
    const { detail, stack } = describe(reason);
    reportUnexpected(sourceFromFrontend(detail, "unhandledrejection", stack));
  });
}
