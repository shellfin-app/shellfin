/** Parsed `[user@]host[:port]` text (a pasted ssh target). `null` fields were not present in the text. */
export interface HostSpecParts {
  user: string | null;
  host: string;
  port: number | null;
}

/**
 * Splits `user@host:port` (also `[::1]:2222`, `host:22`, `user@host`). Returns `null` when the text has no
 * user and no port, so a plain host name is left alone, and when the text is not a sane single-line target.
 * A bare IPv6 address (several colons, no brackets) is a host, not `host:port`.
 */
export function parseHostSpec(text: string): HostSpecParts | null {
  const input = text.trim();
  if (input === "" || /\s/.test(input) || input.length > 512) return null;
  let rest = input;
  let user: string | null = null;
  const at = rest.lastIndexOf("@");
  if (at >= 0) {
    user = rest.slice(0, at);
    rest = rest.slice(at + 1);
    if (user === "") user = null;
  }
  let host = rest;
  let port: number | null = null;
  const bracket = /^\[([^\]]+)\](?::(\d{1,5}))?$/.exec(rest);
  if (bracket) {
    host = bracket[1] ?? "";
    port = bracket[2] !== undefined ? Number(bracket[2]) : null;
  } else {
    const colons = rest.split(":").length - 1;
    if (colons === 1) {
      const m = /^([^:]+):(\d{1,5})$/.exec(rest);
      if (m) {
        host = m[1] ?? "";
        port = Number(m[2]);
      }
    }
  }
  if (host === "") return null;
  if (port !== null && (port < 1 || port > 65535)) return null;
  // A bare `[::1]` is a host in brackets: strip them, keeping the form usable.
  if (user === null && port === null) return bracket ? { user, host, port } : null;
  return { user, host, port };
}
