import type { NoticeInfo } from "../state/slices/ui";

// Informational toasts (texts in `notices.info.<kind>`).
export const FOLDERS_UNSUPPORTED: NoticeInfo = { kind: "foldersUnsupported" };
export const DROP_NO_TARGET: NoticeInfo = { kind: "dropNoTarget" };
export const mixedEolNotice = (name: string): NoticeInfo => ({ kind: "mixedEol", name });
export const COPIED: NoticeInfo = { kind: "copied" };
export const OUTPUT_CANCELLED: NoticeInfo = { kind: "outputCancelled" };
export const OUTPUT_TOO_LARGE: NoticeInfo = { kind: "outputTooLarge" };
export const shortcutSwallowedNotice = (combo: string): NoticeInfo => ({ kind: "shortcutSwallowed", combo });
export const outputSavedNotice = (path: string): NoticeInfo => ({ kind: "outputSaved", path });
// Updater.
export const UP_TO_DATE: NoticeInfo = { kind: "upToDate" };
export const updateAvailableNotice = (version: string): NoticeInfo => ({ kind: "updateAvailable", version });
