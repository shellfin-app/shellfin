import type { DirListing, FileEntry } from "../ipc/types";

function sameEntry(a: FileEntry, b: FileEntry): boolean {
  return (
    a.name === b.name &&
    a.path === b.path &&
    a.kind === b.kind &&
    a.linkTargetKind === b.linkTargetKind &&
    a.size === b.size &&
    a.mtime === b.mtime &&
    a.permissions === b.permissions &&
    a.uid === b.uid &&
    a.gid === b.gid &&
    a.owner === b.owner &&
    a.group === b.group &&
    a.lossyName === b.lossyName
  );
}

/**
 * Does a fresh listing show exactly what the cached one shows (names, kinds, sizes, mtimes, permissions, owners)?
 * Auto-refresh skips replacing a listing that did not change, which keeps the sorted-children cache and the rendered
 * rows.
 */
export function sameListing(a: DirListing, b: DirListing): boolean {
  if (a === b) return true;
  if (a.path !== b.path || a.truncated !== b.truncated || a.entries.length !== b.entries.length) return false;
  for (let i = 0; i < a.entries.length; i += 1) {
    const x = a.entries[i];
    const y = b.entries[i];
    if (x === undefined || y === undefined || !sameEntry(x, y)) return false;
  }
  return true;
}
