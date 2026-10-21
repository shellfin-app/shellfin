import type { AppError } from "../ipc/types";

export type OpenFailureKind = "binary" | "tooLarge" | "decode" | "other";

/** Which page the editor shows for a file that could not be opened as text (§6.3). */
export function failureKind(failure: AppError | null): OpenFailureKind {
  switch (failure?.code) {
    case "fileIsBinary":
      return "binary";
    case "fileTooLarge":
      return "tooLarge";
    case "decodeFailed":
    case "unsupportedEncoding":
      return "decode";
    default:
      return "other";
  }
}
