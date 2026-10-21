// "Copy ssh command": the equivalent `ssh` invocation for a saved host, to paste into any terminal (a hint for the
// user; the app never runs it). Names and hosts are quoted when they hold anything but safe characters, and the
// destination follows `--` so a host or alias that starts with "-" can never be read as an ssh option.
import type { Bookmark, JumpRef, RecentTarget, SshConfigHost } from "../ipc/types";
import { parseHostSpec } from "./hostSpec";

const SAFE = /^[A-Za-z0-9_@%+=:,./][A-Za-z0-9_@%+=:,./-]*$/;

function quote(text: string): string {
  // A single quote inside single quotes: close, an escaped quote, reopen.
  return SAFE.test(text) ? text : `'${text.replaceAll("'", String.raw`'\''`)}'`;
}

function jumpArg(ref: JumpRef, bookmarks: readonly Bookmark[]): string | null {
  switch (ref.kind) {
    case "sshConfig":
      return ref.alias;
    case "spec":
      return ref.spec;
    case "bookmark": {
      const b = bookmarks.find((x) => x.id === ref.id);
      if (!b) return null;
      const host = `${b.user ? `${b.user}@` : ""}${b.host}`;
      return b.port !== null && b.port !== 22 ? `${host}:${b.port}` : host;
    }
  }
}

export function sshCommand(
  target: RecentTarget,
  bookmarks: readonly Bookmark[],
  sshHosts: readonly SshConfigHost[],
): string | null {
  switch (target.kind) {
    case "sshConfig":
      return sshHosts.some((h) => h.alias === target.alias) ? `ssh -- ${quote(target.alias)}` : null;
    case "quick": {
      // `user@host:port` is the app's own spec syntax; ssh wants the port as an option.
      const parts = parseHostSpec(target.spec);
      if (!parts) return `ssh -- ${quote(target.spec)}`;
      const port = parts.port !== null && parts.port !== 22 ? ["-p", String(parts.port)] : [];
      return ["ssh", ...port, "--", quote(`${parts.user ? `${parts.user}@` : ""}${parts.host}`)].join(" ");
    }
    case "bookmark": {
      const b = bookmarks.find((x) => x.id === target.id);
      if (!b) return null;
      const parts = ["ssh"];
      if (b.port !== null && b.port !== 22) parts.push("-p", String(b.port));
      const jump = b.proxyJump ? jumpArg(b.proxyJump, bookmarks) : null;
      // A jump host that looks like an option cannot be quoted into safety: refuse to build the command.
      if (jump?.startsWith("-")) return null;
      if (jump) parts.push("-J", quote(jump));
      parts.push("--", quote(`${b.user ? `${b.user}@` : ""}${b.host}`));
      return parts.join(" ");
    }
  }
}
