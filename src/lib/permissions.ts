import type { FileEntry, RemoteIdentity, SessionInfo } from "../ipc/types";

/** `no` only when the mode bits say so for sure. Anything we cannot judge is `unknown`: no hint is shown for it. */
export type { RemoteIdentity };

export type Writability = "yes" | "no" | "unknown";

type ModeInfo = Pick<FileEntry, "kind" | "permissions" | "uid" | "gid">;

/**
 * Hint from the classic Unix mode bits (owner / group / other) whether `identity` can write the entry. For a directory,
 * "write" means creating, renaming and deleting inside it, which also needs the search (`x`) bit. ACLs, read-only mounts,
 * attributes and symlink targets are invisible here, so this is a hint: it warns, it never blocks an action.
 */
export function writability(entry: ModeInfo | null | undefined, identity: RemoteIdentity | null | undefined): Writability {
  if (!entry || !identity) return "unknown";
  if (entry.permissions === null || entry.uid === null || entry.gid === null) return "unknown";
  if (entry.kind === "symlink" || entry.kind === "other") return "unknown";
  if (identity.uid === 0) return "yes";
  const mode = entry.permissions & 0o777;
  const need = entry.kind === "dir" ? 0b011 : 0b010; // w (+ x for directories)
  let bits: number;
  if (entry.uid === identity.uid) bits = (mode >> 6) & 0b111;
  else if (identity.gids.includes(entry.gid)) bits = (mode >> 3) & 0b111;
  else bits = mode & 0b111;
  return (bits & need) === need ? "yes" : "no";
}

/**
 * Hint whether `identity` can open (list) a directory: needs read and search (`x`) on it. Same caveats as `writability`.
 * Only plain directories are judged; everything else is `unknown`.
 */
export function enterable(entry: ModeInfo | null | undefined, identity: RemoteIdentity | null | undefined): Writability {
  if (!entry || !identity || entry.kind !== "dir") return "unknown";
  if (entry.permissions === null || entry.uid === null || entry.gid === null) return "unknown";
  if (identity.uid === 0) return "yes";
  const mode = entry.permissions & 0o777;
  const need = 0b101; // r + x
  let bits: number;
  if (entry.uid === identity.uid) bits = (mode >> 6) & 0b111;
  else if (identity.gids.includes(entry.gid)) bits = (mode >> 3) & 0b111;
  else bits = mode & 0b111;
  return (bits & need) === need ? "yes" : "no";
}

/** The session user's uid and groups (`SessionInfo.remoteIdentity`); `null` = unknown, so every hint stays off. */
export function identityOf(info: Pick<SessionInfo, "remoteIdentity"> | null | undefined): RemoteIdentity | null {
  return info?.remoteIdentity ?? null;
}

const STICKY = 0o1000;

/**
 * Hint whether `entry` can be renamed, moved or deleted: that needs write + search on the PARENT directory, and in a
 * sticky parent (`/tmp`) also owning the entry or the parent. Same caveats as `writability`.
 */
export function canRemove(
  parent: ModeInfo | null | undefined,
  entry: Pick<FileEntry, "uid"> | null | undefined,
  identity: RemoteIdentity | null | undefined,
): Writability {
  const base = writability(parent, identity);
  if (base !== "yes" || !parent || !identity) return base;
  if (identity.uid === 0 || parent.permissions === null || (parent.permissions & STICKY) === 0) return base;
  if (parent.uid === identity.uid) return "yes";
  if (entry?.uid == null) return "unknown";
  return entry.uid === identity.uid ? "yes" : "no";
}
