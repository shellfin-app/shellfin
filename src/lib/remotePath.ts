// Remote (POSIX) path helpers — the only place remote paths are manipulated (docs/ARCHITECTURE.md §6.2).

export function basename(path: string): string {
  const trimmed = path.length > 1 ? path.replace(/\/+$/, "") : path;
  const i = trimmed.lastIndexOf("/");
  return i < 0 ? trimmed : trimmed.slice(i + 1) || "/";
}

export function dirname(path: string): string {
  const trimmed = path.length > 1 ? path.replace(/\/+$/, "") : path;
  const i = trimmed.lastIndexOf("/");
  if (i < 0) return ".";
  return i === 0 ? "/" : trimmed.slice(0, i);
}

export function joinPath(dir: string, name: string): string {
  return dir.endsWith("/") ? `${dir}${name}` : `${dir}/${name}`;
}

/** Path segments from the root: "/srv/app" -> ["srv", "app"]. */
export function segments(path: string): string[] {
  return path.split("/").filter(Boolean);
}

const NUL = String.fromCharCode(0);

/** Remote paths and names cannot contain NUL. */
export function hasNul(s: string): boolean {
  return s.includes(NUL);
}
