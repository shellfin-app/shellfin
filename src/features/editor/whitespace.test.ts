// `editor.showWhitespace` modes in a real EditorView (ARCHITECTURE §8.11) and the editor theme rules behind #40/#41:
// faint whitespace markers and a selection that the active line cannot hide.
import { EditorSelection, EditorState, type Extension } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it } from "vitest";
import type { ShowWhitespace } from "../../ipc/types";
import { cmTheme } from "../../styles/cmTheme";
import { createBufferState, settingsEffects, type BufferStateOptions } from "./bufferRegistry";

const TEXT = "a b\n\tc\nno_space_here\n  d  e\n";

let views: EditorView[] = [];
afterEach(() => {
  for (const v of views) v.destroy();
  views = [];
});

const options = (showWhitespace: ShowWhitespace, over: Partial<BufferStateOptions> = {}): BufferStateOptions => ({
  tabSize: 4,
  wordWrap: false,
  showWhitespace,
  ...over,
});

function mount(showWhitespace: ShowWhitespace, over: Partial<BufferStateOptions> = {}, extra: Extension[] = []): EditorView {
  const state = createBufferState("/srv/notes.txt", TEXT, options(showWhitespace, over), extra);
  const view = new EditorView({ state, parent: document.body });
  views.push(view);
  return view;
}

/** Document line numbers (1-based) that carry a marker of `kind`, ascending and distinct. */
function markedLines(view: EditorView, kind: "Space" | "Tab"): number[] {
  const lines = [...view.contentDOM.querySelectorAll(`.cm-highlight${kind}`)].map((el) => view.state.doc.lineAt(view.posAtDOM(el)).number);
  return [...new Set(lines)].sort((a, b) => a - b);
}

const cursorOnLine = (view: EditorView, line: number) => view.dispatch({ selection: { anchor: view.state.doc.line(line).from } });

describe("whitespace markers per mode", () => {
  it("none: no markers at all", () => {
    const view = mount("none");
    expect(view.contentDOM.querySelector(".cm-highlightSpace, .cm-highlightTab")).toBeNull();
  });

  it("all: spaces and tabs on every line", () => {
    const view = mount("all");
    expect(markedLines(view, "Space")).toEqual([1, 4]);
    expect(markedLines(view, "Tab")).toEqual([2]);
  });

  it("active line only: the cursor line, following the cursor", () => {
    const view = mount("activeLine");
    expect(markedLines(view, "Space")).toEqual([1]);
    expect(markedLines(view, "Tab")).toEqual([]);
    cursorOnLine(view, 2);
    expect(markedLines(view, "Space")).toEqual([]);
    expect(markedLines(view, "Tab")).toEqual([2]);
    cursorOnLine(view, 4);
    expect(markedLines(view, "Space")).toEqual([4]);
    expect(view.contentDOM.querySelectorAll(".cm-highlightSpace").length).toBeGreaterThanOrEqual(1);
    cursorOnLine(view, 3); // a line without whitespace
    expect(view.contentDOM.querySelector(".cm-highlightSpace, .cm-highlightTab")).toBeNull();
  });

  it("active line only: every cursor of a multiple selection counts, a range counts by its head", () => {
    // The app editor has one selection range today; the plugin is ready for more (like highlightActiveLine).
    const view = mount("activeLine", {}, [EditorState.allowMultipleSelections.of(true)]);
    const doc = view.state.doc;
    view.dispatch({ selection: EditorSelection.create([EditorSelection.cursor(doc.line(1).from), EditorSelection.cursor(doc.line(4).from)]) });
    expect(markedLines(view, "Space")).toEqual([1, 4]);
    view.dispatch({ selection: EditorSelection.range(doc.line(1).from, doc.line(2).to) });
    expect(markedLines(view, "Space")).toEqual([]);
    expect(markedLines(view, "Tab")).toEqual([2]);
  });

  it("active line only: edits keep the marks on the cursor line", () => {
    const view = mount("activeLine");
    view.dispatch({ changes: { from: 0, insert: "x y " }, selection: { anchor: 4 } });
    expect(markedLines(view, "Space")).toEqual([1]);
    expect(view.contentDOM.querySelectorAll(".cm-line")[0]?.querySelectorAll(".cm-highlightSpace").length).toBeGreaterThanOrEqual(2);
  });

  it("active line only works with word wrap (the whole logical line)", () => {
    const view = mount("activeLine", { wordWrap: true });
    cursorOnLine(view, 4);
    expect(markedLines(view, "Space")).toEqual([4]);
  });

  it("switches live between the modes", () => {
    const view = mount("none");
    const apply = (mode: ShowWhitespace) => view.dispatch({ effects: settingsEffects(options(mode), view.state.doc) });
    apply("all");
    expect(markedLines(view, "Space")).toEqual([1, 4]);
    apply("activeLine");
    expect(markedLines(view, "Space")).toEqual([1]);
    apply("none");
    expect(view.contentDOM.querySelector(".cm-highlightSpace, .cm-highlightTab")).toBeNull();
    apply("activeLine");
    expect(markedLines(view, "Space")).toEqual([1]);
  });
});

describe("editor theme (#40, #41)", () => {
  const rules = EditorState.create({ extensions: cmTheme })
    .facet(EditorView.styleModule)
    .map((m) => m.getRules())
    .join("\n");
  /** The declarations of the first rule with a selector (of its comma-separated list) that ends with `selector`. */
  const ruleFor = (selector: string): string => {
    const found = rules.split("\n").find((line) => (line.split("{")[0] ?? "").split(",").some((s) => s.trim().endsWith(selector)));
    if (!found) throw new Error(`no rule for ${selector}`);
    return found.slice(found.indexOf("{"));
  };

  it("whitespace markers are drawn in the faint marker token, not in the text colour", () => {
    expect(ruleFor(".cm-highlightSpace")).toContain("var(--whitespace-marker)");
    const tab = ruleFor(".cm-highlightTab::before");
    expect(tab).toContain("background-color: var(--whitespace-marker)");
    expect(tab).toMatch(/(^|[;{ ])mask: url\('data:image\/svg\+xml,/);
    expect(tab).toContain("-webkit-mask: url(");
    expect(ruleFor(".cm-highlightTab")).toContain("background-image: none");
  });

  it("the selection uses the accent tint focused and unfocused, and the active line its translucent token", () => {
    expect(ruleFor(".cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground")).toContain("var(--selection-bg)");
    // The unfocused selection: `<theme scope> .cm-selectionBackground` alone.
    const unfocused = rules
      .split("\n")
      .find((line) => (line.split("{")[0] ?? "").split(",").some((s) => /^\S+ \.cm-selectionBackground$/.test(s.trim())));
    expect(unfocused).toContain("var(--selection-bg)");
    expect(ruleFor(".cm-activeLine")).toContain("var(--active-line)");
  });
});
