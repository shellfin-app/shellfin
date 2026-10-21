export type PalettePrefix = "commands" | "hosts" | "path" | "";

/**
 * `>` limits the palette to commands, `@` to hosts; the prefix is not part of the search text. A query starting with
 * `/` is a remote path to go to: its text keeps the case and the leading slash.
 */
export function parsePaletteQuery(query: string): { prefix: PalettePrefix; text: string } {
  if (query.startsWith(">")) return { prefix: "commands", text: query.slice(1).trim().toLowerCase() };
  if (query.startsWith("@")) return { prefix: "hosts", text: query.slice(1).trim().toLowerCase() };
  if (query.trimStart().startsWith("/")) return { prefix: "path", text: query.trim() };
  return { prefix: "", text: query.trim().toLowerCase() };
}
