// Save-time text transforms of the editor (settings `editor.trimTrailingWhitespace`, `editor.insertFinalNewline`).
// They are computed as CodeMirror changes (not as a rewritten string) so the buffer can apply them as one undoable
// transaction: the saved text and the editor text stay identical, and undo/dirty tracking keep working.
import type { ChangeSpec, Text } from "@codemirror/state";

export interface SaveTransformOptions {
  trimTrailingWhitespace: boolean;
  insertFinalNewline: boolean;
}

/** Changes that turn `doc` into the text to save. Empty = nothing to do. Positions refer to the original document. */
export function saveTransformChanges(doc: Text, options: SaveTransformOptions): ChangeSpec[] {
  const changes: ChangeSpec[] = [];
  if (options.trimTrailingWhitespace) {
    for (let n = 1; n <= doc.lines; n += 1) {
      const line = doc.line(n);
      const trimmedLength = line.text.replace(/[ \t]+$/, "").length;
      if (trimmedLength < line.text.length) changes.push({ from: line.from + trimmedLength, to: line.to });
    }
  }
  // An empty document stays empty; otherwise the text must end with a line break.
  if (options.insertFinalNewline && doc.length > 0) {
    const last = doc.line(doc.lines).text;
    const effective = options.trimTrailingWhitespace ? last.replace(/[ \t]+$/, "") : last; // a blank last line becomes ""
    if (effective !== "") changes.push({ from: doc.length, insert: "\n" });
  }
  return changes;
}
