import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import type { Extension } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { tags as t } from "@lezer/highlight";

/**
 * CodeMirror 6 theme for both app themes. Colors come from CSS variables (styles/tokens.css), so the same extension
 * serves dark and light; `dark: true` only selects CodeMirror's own defaults, and every one of those that shows a
 * color is overridden here with a token.
 */
const editorTheme = EditorView.theme(
  {
    "&": {
      color: "var(--text)",
      backgroundColor: "var(--bg)",
      fontFamily: "var(--editor-font-family, var(--font-mono))",
      fontSize: "var(--editor-font-size, 13px)",
      height: "100%",
    },
    ".cm-scroller": { fontFamily: "var(--editor-font-family, var(--font-mono))", lineHeight: "21px", overflow: "auto" },
    // `editor.fontLigatures`: the bundled font has ligatures, so "off" must be explicit (`none`); "on" = `normal`.
    ".cm-content": {
      caretColor: "var(--accent)",
      padding: "10px 0",
      fontVariantLigatures: "var(--editor-font-ligatures, none)",
    },
    ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--accent)" },
    "&.cm-focused": { outline: "none" },
    "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground, ::selection": {
      backgroundColor: "var(--selection-bg)",
    },
    ".cm-gutters": {
      backgroundColor: "var(--bg)",
      color: "var(--text-gutter)",
      border: "none",
      paddingRight: "4px",
    },
    ".cm-lineNumbers .cm-gutterElement": { padding: "0 12px 0 20px", minWidth: "40px" },
    ".cm-activeLine": { backgroundColor: "var(--active-line)" },
    ".cm-activeLineGutter": { backgroundColor: "var(--active-line)", color: "var(--text)" },
    ".cm-foldGutter span": { color: "var(--text-muted)", padding: "0 3px", cursor: "pointer" },
    ".cm-matchingBracket, .cm-nonmatchingBracket": { backgroundColor: "var(--accent-ring)", outline: "1px solid var(--accent)" },
    ".cm-selectionMatch": { backgroundColor: "var(--accent-ring)" },
    ".cm-searchMatch": { backgroundColor: "var(--warn-bg)", outline: "1px solid var(--warn)" },
    ".cm-searchMatch-selected": { backgroundColor: "var(--accent-ring)", outline: "1px solid var(--accent)" },
    ".cm-foldPlaceholder": {
      backgroundColor: "var(--hover)",
      color: "var(--text-muted)",
      border: "1px solid var(--border-input)",
    },
    ".cm-panels": { backgroundColor: "var(--panel)", color: "var(--text)", borderColor: "var(--border)" },
    ".cm-panels-bottom": { borderTop: "1px solid var(--border)" },
    // Everything CodeMirror styles differently for dark editors (`dark: true` below) is set from tokens here, so the
    // light theme never inherits a dark default: buttons and text fields (search panel, Go to line), special
    // characters, tooltips.
    ".cm-button": {
      backgroundColor: "var(--hover)",
      backgroundImage: "none",
      color: "var(--text)",
      border: "1px solid var(--border-input)",
      borderRadius: "var(--radius-sm)",
    },
    ".cm-button:active": { backgroundImage: "none", backgroundColor: "var(--selected)" },
    ".cm-textfield": {
      backgroundColor: "var(--bg)",
      color: "var(--text)",
      border: "1px solid var(--border-input)",
      borderRadius: "var(--radius-sm)",
    },
    ".cm-dialog": { fontFamily: "var(--font-ui)", fontSize: "calc(12px * var(--ui-scale, 1))" },
    ".cm-specialChar": { color: "var(--danger)" },
    ".cm-tooltip": { backgroundColor: "var(--panel)", color: "var(--text)", border: "1px solid var(--border)" },
    ".cm-tooltip .cm-tooltip-arrow:before": { borderTopColor: "var(--border)", borderBottomColor: "var(--border)" },
    ".cm-tooltip .cm-tooltip-arrow:after": { borderTopColor: "var(--panel)", borderBottomColor: "var(--panel)" },
    ".cm-panel.cm-search": {
      fontFamily: "var(--font-ui)",
      fontSize: "calc(12px * var(--ui-scale, 1))",
      padding: "6px 28px 6px 10px",
    },
    ".cm-panel.cm-search input, .cm-panel.cm-search button": { font: "inherit" },
    ".cm-panel.cm-search input[type=text]": {
      backgroundColor: "var(--bg)",
      color: "var(--text)",
      border: "1px solid var(--border-input)",
      borderRadius: "var(--radius-sm)",
      padding: "2px 6px",
    },
    ".cm-panel.cm-search button": {
      backgroundColor: "var(--hover)",
      backgroundImage: "none", // CodeMirror's default buttons carry a dark/light gradient of their own
      color: "var(--text)",
      border: "1px solid var(--border-input)",
      borderRadius: "var(--radius-sm)",
      padding: "2px 8px",
    },
    ".cm-panel.cm-search [name=close]": { color: "var(--text-muted)", background: "transparent", border: "none" },
  },
  { dark: true },
);

const highlightStyle = HighlightStyle.define([
  { tag: [t.keyword, t.propertyName, t.definition(t.propertyName), t.atom], color: "var(--syn-keyword)" },
  { tag: [t.string, t.special(t.string)], color: "var(--syn-string)" },
  { tag: [t.number, t.bool, t.null], color: "var(--syn-number)" },
  { tag: [t.comment, t.lineComment, t.blockComment], color: "var(--syn-comment)" },
  { tag: [t.typeName, t.className, t.meta, t.tagName], color: "var(--syn-purple)" },
]);

export const cmTheme: Extension = [editorTheme, syntaxHighlighting(highlightStyle)];
