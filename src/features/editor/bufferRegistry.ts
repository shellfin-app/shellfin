// CodeMirror runtime part of the per-buffer state (docs/ARCHITECTURE.md §6.5): extensions, compartments and the
// builders of `EditorState`. The store itself (`bufferStore.ts`) is light; this module is loaded on demand together with
// the editor (`bufferLoader.ts`) so the entry chunk does not carry CodeMirror.
import { closeBrackets, closeBracketsKeymap } from "@codemirror/autocomplete";
import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
import { bracketMatching, foldGutter, foldKeymap, indentUnit } from "@codemirror/language";
import { highlightSelectionMatches, searchKeymap } from "@codemirror/search";
import { Compartment, EditorState, type Extension, type Text } from "@codemirror/state";
import {
  EditorView,
  drawSelection,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightWhitespace,
  keymap,
  lineNumbers,
} from "@codemirror/view";
import i18next from "i18next";
import { cmPhrases } from "../../lib/cmPhrases";
import { detectIndent, type DetectedIndent } from "../../lib/detectIndent";
import { languageForPath } from "../../lib/languages";
import { basename } from "../../lib/remotePath";
import { displayName } from "../../lib/serverText";
import { cmTheme } from "../../styles/cmTheme";
import { updateStoredStates, type BufferStateOptions } from "./bufferStore";
import { languageExtension } from "./languageExtension";

// The light store API stays importable from here (tests and the editor use one import path).
export * from "./bufferStore";

const indentCompartment = new Compartment();
const wrapCompartment = new Compartment();
const gutterCompartment = new Compartment();
const activeLineCompartment = new Compartment();
const bracketMatchCompartment = new Compartment();
const closeBracketsCompartment = new Compartment();
const whitespaceCompartment = new Compartment();
const foldCompartment = new Compartment();
const localeCompartment = new Compartment();

/** How much of a document is looked at to detect its indentation (a 256 MiB file must not be scanned whole). */
const DETECT_SAMPLE = 200_000;

/** The document's own indentation when `detectIndentation` is on and the text is unambiguous, else `null`. */
function detectedIndent(options: BufferStateOptions, sample: string | null): DetectedIndent | null {
  return options.detectIndentation !== false && sample !== null ? detectIndent(sample) : null;
}

function indentExtension(options: BufferStateOptions, sample: string | null): Extension {
  const detected = detectedIndent(options, sample);
  const tabs = detected ? detected.tabs : (options.indentWithTabs ?? false);
  const width = detected && !detected.tabs ? detected.width : options.tabSize;
  return [EditorState.tabSize.of(width), indentUnit.of(tabs ? "\t" : " ".repeat(width))];
}

const bracketMatchExtension = (options: BufferStateOptions): Extension => ((options.bracketMatching ?? true) ? bracketMatching() : []);
const closeBracketsExtension = (options: BufferStateOptions): Extension =>
  (options.autoCloseBrackets ?? false) ? [closeBrackets(), keymap.of(closeBracketsKeymap)] : [];
const whitespaceExtension = (options: BufferStateOptions): Extension => ((options.showWhitespace ?? false) ? highlightWhitespace() : []);
const foldExtension = (options: BufferStateOptions): Extension => ((options.codeFolding ?? true) ? [foldGutter(), keymap.of(foldKeymap)] : []);

const gutterExtension = (options: BufferStateOptions): Extension => ((options.lineNumbers ?? true) ? lineNumbers() : []);
const activeLineExtension = (options: BufferStateOptions): Extension =>
  (options.highlightActiveLine ?? true) ? [highlightActiveLine(), highlightActiveLineGutter()] : [];

const sampleOf = (doc: Text | undefined): string | null => (doc ? doc.sliceString(0, Math.min(doc.length, DETECT_SAMPLE)) : null);

/**
 * Effects that apply new editor settings to an existing state (`EditorView.dispatch({ effects })`). `doc` is the
 * document of that state: indentation detection looks at it.
 */
export function settingsEffects(options: BufferStateOptions, doc?: Text) {
  return [
    indentCompartment.reconfigure(indentExtension(options, sampleOf(doc))),
    wrapCompartment.reconfigure(options.wordWrap ? EditorView.lineWrapping : []),
    gutterCompartment.reconfigure(gutterExtension(options)),
    activeLineCompartment.reconfigure(activeLineExtension(options)),
    bracketMatchCompartment.reconfigure(bracketMatchExtension(options)),
    closeBracketsCompartment.reconfigure(closeBracketsExtension(options)),
    whitespaceCompartment.reconfigure(whitespaceExtension(options)),
    foldCompartment.reconfigure(foldExtension(options)),
  ];
}

/** Applies changed editor settings to every stored state except `exceptId` (that one is live in the view). */
export function applySettingsToAll(options: BufferStateOptions, exceptId: string | null): void {
  updateStoredStates((_id, state) => state.update({ effects: settingsEffects(options, state.doc) }).state, exceptId);
}

/** Everything in an editor that is worded in the UI language: CodeMirror's own phrases and the accessible name. */
function localeExtension(path: string): Extension {
  return [
    EditorState.phrases.of(cmPhrases()),
    EditorView.contentAttributes.of({ "aria-label": i18next.t("editor.contentLabel", { name: displayName(basename(path)) }) }),
  ];
}

/** Effect that re-words one state after a language change (`path` = the buffer's remote path). */
export function localeEffect(path: string) {
  return localeCompartment.reconfigure(localeExtension(path));
}

/** After a language change: every stored state gets the new wording (`pathOf` = the buffer's path, null = unknown). */
export function applyLocaleToAll(pathOf: (bufferId: string) => string | null): void {
  updateStoredStates((id, state) => {
    const path = pathOf(id);
    return path === null ? state : state.update({ effects: localeEffect(path) }).state;
  });
}

/** Creates the initial EditorState for a document (language picked from the path). */
export function createBufferState(
  path: string,
  text: string,
  options: BufferStateOptions,
  extra: readonly Extension[] = [],
): EditorState {
  return EditorState.create({
    doc: text,
    extensions: [
      gutterCompartment.of(gutterExtension(options)),
      activeLineCompartment.of(activeLineExtension(options)),
      foldCompartment.of(foldExtension(options)),
      history(),
      drawSelection(),
      bracketMatchCompartment.of(bracketMatchExtension(options)),
      closeBracketsCompartment.of(closeBracketsExtension(options)),
      whitespaceCompartment.of(whitespaceExtension(options)),
      highlightSelectionMatches(),
      keymap.of([...defaultKeymap, ...historyKeymap, ...searchKeymap]),
      indentCompartment.of(indentExtension(options, text.length > DETECT_SAMPLE ? text.slice(0, DETECT_SAMPLE) : text)),
      wrapCompartment.of(options.wordWrap ? EditorView.lineWrapping : []),
      localeCompartment.of(localeExtension(path)),
      languageExtension(languageForPath(path)),
      cmTheme,
      ...extra,
    ],
  });
}
