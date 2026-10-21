import { displayServerText } from "../lib/serverText";

export interface ServerTextProps {
  /** Untrusted text received from the server. */
  text: string;
  /** Maximum number of characters (§9.5: banners 8192, labels 256). */
  limit?: number;
  keepNewlines?: boolean;
}

/** Renders server-originated text as sanitized plain text (never HTML), bidi-isolated (§9.5). */
export function ServerText({ text, limit = 256, keepNewlines = false }: ServerTextProps) {
  return <bdi className="server-text">{displayServerText(text, limit, keepNewlines)}</bdi>;
}
