// Constructors for the few backend errors the frontend raises itself (e.g. a strict "save all" that could not
// save, or a request that must not be sent). Identifiers only, no UI copy: the text comes from `errors.<code>`.
import type { AppError } from "../ipc/types";

export const remoteChangedError = (path: string, size: number, mtime: number, target?: string): AppError => ({
  code: "remoteChanged",
  params: target === undefined ? { path, size, mtime } : { path, size, mtime, target },
});
export const notConnectedError = (sessionId: string): AppError => ({ code: "notConnected", params: { sessionId } });
export const invalidArgumentError = (field: string): AppError => ({ code: "invalidArgument", params: { field } });
