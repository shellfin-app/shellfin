import type { ReactNode } from "react";
import { type Block, type Inline, type ParsedNotes, allowedReleaseLink, parseReleaseNotes } from "../../lib/releaseNotes";
import { displayServerText } from "../../lib/serverText";
import { openNotesLink } from "./updateActions";

/** Per text node bound; the whole input is already bounded by the dialog (NOTES_DISPLAY_LIMIT). */
const NODE_LIMIT = 10_000;

const text = (s: string) => displayServerText(s, NODE_LIMIT, true);

function renderInline(nodes: Inline[]): ReactNode[] {
  return nodes.map((n, i) => {
    switch (n.kind) {
      case "text":
        return text(n.text);
      case "code":
        return <code key={i}>{text(n.text)}</code>;
      case "bold":
        return <strong key={i}>{renderInline(n.children)}</strong>;
      case "link":
        // Allowlisted links open through the backend's confirmed opener; the webview never navigates.
        return renderLink(n, i);
    }
  });
}

function renderLink(n: Extract<Inline, { kind: "link" }>, key: number): ReactNode {
  const url = allowedReleaseLink(n.url);
  // What was checked is what is opened: the canonical `href` of the parsed URL, not the raw text.
  return url ? (
    <button key={key} type="button" className="update__link" onClick={() => openNotesLink(url.href)}>
      {text(n.text)}
    </button>
  ) : (
    text(n.text)
  );
}

function renderBlock(b: Block, i: number): ReactNode {
  switch (b.kind) {
    case "heading":
      return (
        <h4 key={i} className={`update__md-h update__md-h--${b.level}`}>
          {renderInline(b.inline)}
        </h4>
      );
    case "paragraph":
      return <p key={i}>{renderInline(b.inline)}</p>;
    case "list":
      return (
        <ul key={i}>
          {b.items.map((it, j) => (
            <li key={j}>
              {renderInline(it.inline)}
              {it.sub.length > 0 ? (
                <ul>
                  {it.sub.map((s, k) => (
                    <li key={k}>{renderInline(s)}</li>
                  ))}
                </ul>
              ) : null}
            </li>
          ))}
        </ul>
      );
  }
}

/** Release notes (untrusted): a small Markdown subset rendered as React elements, never as HTML (§9.5, §9.8). */
export function ReleaseNotes({ blocks }: { blocks: Block[] }) {
  return <>{blocks.map(renderBlock)}</>;
}

/**
 * Escape and bound the input, then parse (the caller shows the "shortened" hint when the result is truncated, which
 * includes a cut by the display limit: escaping can make notes within the backend's limit exceed it).
 */
export function parseNotesForDisplay(notes: string, limit: number): ParsedNotes {
  const escaped = displayServerText(notes, Number.POSITIVE_INFINITY, true);
  const shown = displayServerText(escaped, limit, true); // idempotent on already escaped text
  const parsed = parseReleaseNotes(shown);
  return shown === escaped ? parsed : { ...parsed, truncated: true };
}
