/** `user@host:port` — technical data shown verbatim (never translated). IPv6 hosts are bracketed. */
export function formatEndpoint(user: string, host: string, port: number): string {
  const h = host.includes(":") ? `[${host}]` : host;
  return `${user}@${h}:${port}`;
}
