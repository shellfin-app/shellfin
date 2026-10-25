import { compareNames } from "../../i18n/collate";
import type { Bookmark, BookmarkColor, RecentTarget, SshConfigHost } from "../../ipc/types";
import { bookmarkTarget, sshConfigTarget } from "../../lib/targets";

export interface HostRow {
  key: string;
  label: string;
  /** Right-aligned secondary text: address (jump host shown via `via`). */
  address: string;
  /** Jump host alias; rendered through the `hosts.via` key. */
  via: string | null;
  /** Bookmark color (ssh-config hosts have none). */
  color: BookmarkColor | null;
  target: RecentTarget;
}

export function sshConfigRows(hosts: readonly SshConfigHost[]): HostRow[] {
  return hosts.map((h) => ({
    key: `ssh:${h.alias}`,
    label: h.alias,
    address: h.hostname,
    via: h.proxyJump,
    color: null,
    target: sshConfigTarget(h.alias),
  }));
}

/** Bookmarks of ONE list (a group or the ungrouped list) in their persistent order. */
export function bookmarkRows(bookmarks: readonly Bookmark[]): HostRow[] {
  return [...bookmarks]
    .sort((a, b) => a.order - b.order || compareNames(a.name, b.name))
    .map((b) => ({
      key: `bm:${b.id}`,
      label: b.name,
      address: b.port !== null && b.port !== 22 ? `${b.host}:${b.port}` : b.host,
      via: null,
      color: b.color,
      target: bookmarkTarget(b.id),
    }));
}
