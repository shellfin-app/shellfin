// Pure helpers for the file tree details (`files.showPermissions`): the `ls -l` style mode string and the owner/group
// text. Names are server text: callers render them through `displayServerText`.
import type { FileEntry } from "../ipc/types";

type ModeSource = Pick<FileEntry, "kind" | "permissions">;

const KIND_CHAR: Record<FileEntry["kind"], string> = { dir: "d", file: "-", symlink: "l", other: "?" };

/** `drwxr-xr-x` (with s/S, t/T for setuid, setgid, sticky); `null` when the server reported no mode. */
export function modeString(entry: ModeSource): string | null {
  const mode = entry.permissions;
  if (mode === null) return null;
  const bit = (mask: number, ch: string) => ((mode & mask) !== 0 ? ch : "-");
  const exec = (xMask: number, special: number, on: string, off: string) => {
    const x = (mode & xMask) !== 0;
    if ((mode & special) === 0) return x ? "x" : "-";
    return x ? on : off;
  };
  return (
    KIND_CHAR[entry.kind] +
    bit(0o400, "r") +
    bit(0o200, "w") +
    exec(0o100, 0o4000, "s", "S") +
    bit(0o040, "r") +
    bit(0o020, "w") +
    exec(0o010, 0o2000, "s", "S") +
    bit(0o004, "r") +
    bit(0o002, "w") +
    exec(0o001, 0o1000, "t", "T")
  );
}

/** The owner name, else the numeric uid, else `null` (nothing known). */
export function ownerText(entry: Pick<FileEntry, "owner" | "uid">): string | null {
  if (entry.owner !== null) return entry.owner;
  return entry.uid !== null ? String(entry.uid) : null;
}

/** The group name, else the numeric gid, else `null`. */
export function groupText(entry: Pick<FileEntry, "group" | "gid">): string | null {
  if (entry.group !== null) return entry.group;
  return entry.gid !== null ? String(entry.gid) : null;
}
