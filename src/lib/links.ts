// Plain-text URL detection for terminal output (docs/ARCHITECTURE.md §8.6). Only http(s) is ever offered; the
// backend repeats the check and shows a native confirmation with the real URL.

export const MAX_LINK_LENGTH = 2048;

export interface FoundUrl {
  url: string;
  /** UTF-16 index of the first character in the searched text. */
  start: number;
  /** Exclusive end index. */
  end: number;
}

// Candidate: scheme + everything up to whitespace, quotes, angle brackets or a control character.
const CANDIDATE = new RegExp(
  `https?://[^\\s<>"'\`${String.fromCharCode(0)}-${String.fromCharCode(0x1f)}${String.fromCharCode(0x7f)}-${String.fromCharCode(0x9f)}]+`,
  "gi",
);
const TRAILING = new Set([".", ",", ";", ":", "!", "?", "'", '"']);
const PAIRS: Record<string, string> = { ")": "(", "]": "[", "}": "{" };

function count(s: string, ch: string): number {
  let n = 0;
  for (const c of s) if (c === ch) n += 1;
  return n;
}

function trimTrailing(url: string): string {
  let out = url;
  for (;;) {
    const last = out[out.length - 1];
    if (last === undefined) return out;
    const open = PAIRS[last];
    if (TRAILING.has(last) || (open !== undefined && count(out, last) > count(out, open))) {
      out = out.slice(0, -1);
    } else {
      return out;
    }
  }
}

/** True for absolute http/https URLs without credentials, within the length bound. */
export function isOpenableUrl(uri: string): boolean {
  if (uri.length > MAX_LINK_LENGTH) return false;
  try {
    const u = new URL(uri);
    return (u.protocol === "http:" || u.protocol === "https:") && u.username === "" && u.password === "";
  } catch {
    return false;
  }
}

/** All http(s) URLs in one line of terminal text. */
export function findUrls(text: string): FoundUrl[] {
  const found: FoundUrl[] = [];
  for (const m of text.matchAll(CANDIDATE)) {
    const url = trimTrailing(m[0]);
    if (isOpenableUrl(url)) found.push({ url, start: m.index, end: m.index + url.length });
  }
  return found;
}
