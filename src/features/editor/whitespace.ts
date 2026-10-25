// `editor.showWhitespace` in the editor (docs/ARCHITECTURE.md §8.11): `all` is CodeMirror's `highlightWhitespace`,
// `activeLine` marks only the lines that hold a cursor, with the same classes (`cm-highlightSpace`,
// `cm-highlightTab`), so both modes look alike; the faint marker colour is in `styles/cmTheme.ts`.
import { RangeSetBuilder, type EditorState, type Extension } from "@codemirror/state";
import { Decoration, ViewPlugin, highlightWhitespace, type DecorationSet, type EditorView, type ViewUpdate } from "@codemirror/view";
import type { ShowWhitespace } from "../../ipc/types";
import { CM_WHITESPACE_CLASS, SHOW_WHITESPACE } from "../../lib/identifiers";

const spaceMark = Decoration.mark({ class: CM_WHITESPACE_CLASS.space });
const tabMark = Decoration.mark({ class: CM_WHITESPACE_CLASS.tab });

/** Numbers of the lines with a cursor (the head of every selection range), ascending and distinct. */
function cursorLines(state: EditorState): number[] {
  const numbers = state.selection.ranges.map((range) => state.doc.lineAt(range.head).number);
  return numbers.sort((a, b) => a - b).filter((n, i, all) => i === 0 || all[i - 1] !== n);
}

/** Marks for the spaces and tabs of the cursor lines, limited to the rendered part (a minified line can be huge). */
function cursorLineMarks(view: EditorView): DecorationSet {
  const builder = new RangeSetBuilder<Decoration>();
  const { doc } = view.state;
  for (const number of cursorLines(view.state)) {
    const line = doc.line(number);
    for (const { from, to } of view.visibleRanges) {
      const start = Math.max(from, line.from);
      const end = Math.min(to, line.to);
      if (start >= end) continue;
      const text = doc.sliceString(start, end);
      for (let i = 0; i < text.length; i++) {
        const ch = text.charCodeAt(i);
        if (ch === 0x20) builder.add(start + i, start + i + 1, spaceMark);
        else if (ch === 0x09) builder.add(start + i, start + i + 1, tabMark);
      }
    }
  }
  return builder.finish();
}

const activeLineWhitespace = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = cursorLineMarks(view);
    }
    update(update: ViewUpdate) {
      if (update.docChanged || update.selectionSet || update.viewportChanged) this.decorations = cursorLineMarks(update.view);
    }
  },
  { decorations: (plugin) => plugin.decorations },
);

/** The whitespace markers for a mode (`none` = no extension at all). */
export function whitespaceMarkers(mode: ShowWhitespace): Extension {
  if (mode === SHOW_WHITESPACE.all) return highlightWhitespace();
  if (mode === SHOW_WHITESPACE.activeLine) return activeLineWhitespace;
  return [];
}
