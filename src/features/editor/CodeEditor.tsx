import { redo, selectAll, undo } from "@codemirror/commands";
import { openSearchPanel } from "@codemirror/search";
import type { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { useEffect, useRef, type CSSProperties } from "react";
import { useTranslation } from "react-i18next";
import { terminalFontStack } from "../../lib/fontFamily";
import { useStore } from "../../state/store";
import {
  applySettingsToAll,
  bufferOptionsFromSettings,
  getBufferState,
  localeEffect,
  setBufferState,
  settingsEffects,
} from "./bufferRegistry";
import { registerCmCommands, registerCmView } from "./cmBridge";
import { refreshDirty, registerEditorFocus } from "./editorActions";

// The Edit menu and the context menu run these on the focused editor without importing CodeMirror themselves.
registerCmCommands({ undo, redo, selectAll, openSearchPanel });

/** After the last edit this long, the dirty flag is re-checked against the saved text (so undo back to clean works). */
const DIRTY_CHECK_MS = 150;

/** Saves a buffer's state back to the registry, but only while the buffer is still open (closed buffers must not be resurrected). */
function persistState(bufferId: string, state: EditorState): void {
  if (useStore.getState().buffers[bufferId]) setBufferState(bufferId, state);
}

function cursorOf(state: EditorState): { line: number; col: number } {
  const head = state.selection.main.head;
  const line = state.doc.lineAt(head);
  return { line: line.number, col: head - line.from + 1 };
}

/**
 * One CodeMirror view for the active buffer. Switching buffers swaps the EditorState (so undo history and
 * selection survive tab switches, §6.5); the state of the buffer being left is saved first.
 */
export function CodeEditor({ bufferId }: { bufferId: string }) {
  const { i18n } = useTranslation();
  const hostRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const currentRef = useRef<string | null>(null);
  const dirtyTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const rev = useStore((s) => s.buffers[bufferId]?.rev ?? 0);
  const editor = useStore((s) => s.settings?.editor);
  const {
    tabSize,
    wordWrap,
    lineNumbers,
    highlightActiveLine,
    indentWithTabs,
    detectIndentation,
    autoCloseBrackets,
    bracketMatching,
    showWhitespace,
    codeFolding,
  } = bufferOptionsFromSettings(editor);
  const fontSize = editor?.fontSize ?? 13;
  const fontStack = terminalFontStack(editor?.fontFamily ?? null);

  // Create the view once.
  useEffect(() => {
    const host = hostRef.current;
    if (!host) return undefined;
    const view = new EditorView({
      parent: host,
      dispatch: (tr, v) => {
        v.update([tr]);
        const id = currentRef.current;
        if (!id) return;
        const { patchBuffer, setCursor, buffers } = useStore.getState();
        if (tr.docChanged) {
          // Dirty at once; a debounced check can turn it clean again (undo back to the saved text).
          if (buffers[id]?.dirty === false) patchBuffer(id, { dirty: true });
          persistState(id, v.state); // the registry always holds the live state (save, quit-save-all)
          if (dirtyTimer.current) clearTimeout(dirtyTimer.current);
          dirtyTimer.current = setTimeout(() => {
            dirtyTimer.current = null;
            if (currentRef.current === id) refreshDirty(id);
          }, DIRTY_CHECK_MS);
        }
        if (tr.docChanged || tr.selection) setCursor(id, cursorOf(v.state));
      },
    });
    viewRef.current = view;
    registerCmView(view, true);
    registerEditorFocus(() => view.focus());
    return () => {
      registerEditorFocus(null);
      registerCmView(view, false);
      if (dirtyTimer.current) clearTimeout(dirtyTimer.current);
      const id = currentRef.current;
      if (id) persistState(id, view.state);
      view.destroy();
      viewRef.current = null;
      currentRef.current = null;
    };
  }, []);

  // Swap the state when the active buffer changes, or when it was replaced from outside (load / reload).
  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    const previous = currentRef.current;
    if (previous && previous !== bufferId) {
      persistState(previous, view.state);
      if (dirtyTimer.current) {
        clearTimeout(dirtyTimer.current);
        dirtyTimer.current = null;
        refreshDirty(previous);
      }
    }
    const next = getBufferState(bufferId);
    currentRef.current = bufferId;
    if (next && view.state !== next) {
      view.setState(next);
      useStore.getState().setCursor(bufferId, cursorOf(next));
    }
  }, [bufferId, rev]);

  // Editor settings changed (tab size, word wrap): live view now, stored states for the other buffers.
  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    const options = {
      tabSize,
      wordWrap,
      lineNumbers,
      highlightActiveLine,
      indentWithTabs,
      detectIndentation,
      autoCloseBrackets,
      bracketMatching,
      showWhitespace,
      codeFolding,
    };
    view.dispatch({ effects: settingsEffects(options, view.state.doc) });
    applySettingsToAll(options, currentRef.current);
  }, [tabSize, wordWrap, lineNumbers, highlightActiveLine, indentWithTabs, detectIndentation, autoCloseBrackets, bracketMatching, showWhitespace, codeFolding]);

  // UI language changed: the live view gets the new phrases and accessible name (stored states: editorActions).
  const path = useStore((s) => s.buffers[bufferId]?.path);
  const language = i18n.resolvedLanguage;
  useEffect(() => {
    const view = viewRef.current;
    if (!view || path === undefined) return;
    view.dispatch({ effects: localeEffect(path) });
  }, [language, path, rev]);

  // Font size and family are CSS variables on the host: every open editor follows live, no state change needed.
  return (
    <div
      ref={hostRef}
      className="code-editor"
      style={
        {
          "--editor-font-size": `${fontSize}px`,
          "--editor-font-family": fontStack,
          "--editor-font-ligatures": editor?.fontLigatures ? "normal" : "none",
        } as CSSProperties
      }
    />
  );
}
