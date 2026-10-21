import type { Bookmark, JumpRef, RecentTarget, SshConfigHost } from "../ipc/types";
import { formatEndpoint } from "./endpoint";
import { parseHostSpec } from "./hostSpec";

export interface TargetInfo {
  /** `user@host:port` (user omitted when unknown). `null` when the target can no longer be resolved. */
  endpoint: string | null;
  host: string | null;
  port: number | null;
  /** Jump host label (alias / bookmark name / spec), if the target goes through one. */
  via: string | null;
}

function jumpLabel(ref: JumpRef | null, bookmarks: readonly Bookmark[]): string | null {
  if (!ref) return null;
  switch (ref.kind) {
    case "sshConfig":
      return ref.alias;
    case "bookmark":
      return bookmarks.find((b) => b.id === ref.id)?.name ?? null;
    case "spec":
      return ref.spec;
  }
}

function endpointOf(user: string | null, host: string, port: number): string {
  return user ? formatEndpoint(user, host, port) : `${host.includes(":") ? `[${host}]` : host}:${port}`;
}

/** What the connecting card / failure panel say about a session's target (display data only). */
export function describeTarget(
  target: RecentTarget,
  bookmarks: readonly Bookmark[],
  sshHosts: readonly SshConfigHost[],
): TargetInfo {
  const none: TargetInfo = { endpoint: null, host: null, port: null, via: null };
  switch (target.kind) {
    case "bookmark": {
      const b = bookmarks.find((x) => x.id === target.id);
      if (!b) return none;
      const port = b.port ?? 22;
      return { endpoint: endpointOf(b.user, b.host, port), host: b.host, port, via: jumpLabel(b.proxyJump, bookmarks) };
    }
    case "sshConfig": {
      const h = sshHosts.find((x) => x.alias === target.alias);
      if (!h) return none;
      return { endpoint: endpointOf(h.user, h.hostname, h.port), host: h.hostname, port: h.port, via: h.proxyJump };
    }
    case "quick": {
      const parts = parseHostSpec(target.spec);
      if (parts) {
        const port = parts.port ?? 22;
        return { endpoint: endpointOf(parts.user, parts.host, port), host: parts.host, port, via: null };
      }
      return { endpoint: target.spec, host: target.spec, port: null, via: null };
    }
  }
}
