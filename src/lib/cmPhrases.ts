// CodeMirror ships English UI strings (search panel, fold gutter, announcements). `EditorState.phrases` replaces them.
// Left side: the exact English phrase CodeMirror asks for; right side: the `editor.cm.<id>` key in translation.json.
// This is data, not UI copy, so it lives outside the no-literal-string lint scope (docs/ARCHITECTURE.md §8.2.2).
import i18next from "i18next";

const PHRASE_KEYS: Readonly<Record<string, string>> = {
  Find: "editor.cm.find",
  Replace: "editor.cm.replace",
  next: "editor.cm.next",
  previous: "editor.cm.previous",
  all: "editor.cm.all",
  "match case": "editor.cm.matchCase",
  regexp: "editor.cm.regexp",
  "by word": "editor.cm.byWord",
  replace: "editor.cm.replaceButton",
  "replace all": "editor.cm.replaceAll",
  close: "editor.cm.close",
  "Go to line": "editor.cm.goToLine",
  go: "editor.cm.go",
  "current match": "editor.cm.currentMatch",
  "on line": "editor.cm.onLine",
  "replaced match on line $": "editor.cm.replacedMatchOnLine",
  "replaced $ matches": "editor.cm.replacedMatches",
  "Control character": "editor.cm.controlCharacter",
  "folded code": "editor.cm.foldedCode",
  unfold: "editor.cm.unfold",
  "Folded lines": "editor.cm.foldedLines",
  "Unfolded lines": "editor.cm.unfoldedLines",
  to: "editor.cm.to",
  "Fold line": "editor.cm.foldLine",
  "Unfold line": "editor.cm.unfoldLine",
  "Selection deleted": "editor.cm.selectionDeleted",
};

/** The phrase table in the current UI language. */
export function cmPhrases(): Record<string, string> {
  const t = i18next.t.bind(i18next) as (key: string) => string;
  const out: Record<string, string> = {};
  for (const [english, key] of Object.entries(PHRASE_KEYS)) out[english] = t(key);
  return out;
}

/** i18n keys used above (for the completeness test). */
export const CM_PHRASE_KEYS: readonly string[] = Object.values(PHRASE_KEYS);
