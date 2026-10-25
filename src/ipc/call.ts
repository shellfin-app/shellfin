import type { AppError } from "./types";

/** Rejection type of every backend command (docs/ARCHITECTURE.md §4.1). */
export class IpcError extends Error {
  readonly error: AppError;
  /** True when the rejection did not come from the backend (a JS exception or string wrapped by `toIpcError`). */
  readonly wrapped: boolean;
  constructor(error: AppError, wrapped = false) {
    super(error.detail ?? error.code);
    this.name = "IpcError";
    this.error = error;
    this.wrapped = wrapped;
  }
}

export function isAppError(e: unknown): e is AppError {
  return typeof e === "object" && e !== null && typeof (e as { code?: unknown }).code === "string";
}

/** Normalizes anything a command promise rejects with into an `IpcError`. */
export function toIpcError(e: unknown): IpcError {
  if (e instanceof IpcError) return e;
  if (isAppError(e)) return new IpcError(e);
  const detail = e instanceof Error ? e.message : typeof e === "string" ? e : "unknown error";
  return new IpcError({ code: "internal", detail }, true);
}
