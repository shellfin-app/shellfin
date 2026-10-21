import type { Platform } from "../ipc/types";

/** A local path for display: one separator style per platform (the backend mixes `~\.ssh/id_x` on Windows). */
export function displayLocalPath(path: string, platform: Platform): string {
  return platform === "windows" ? path.replaceAll("/", "\\") : path;
}
