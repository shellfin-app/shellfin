// Per-buffer CodeMirror EditorState store (docs/ARCHITECTURE.md §6.5: "per-buffer EditorState in the buffer registry,
// outside Zustand; tab switch swaps states, undo history per buffer"). Text and undo history live here, metadata
// lives in the Zustand `buffers` slice.
//
// This module is deliberately light: it imports CodeMirror as types only. Everything that needs the CodeMirror
// runtime (building states, extensions, compartments) is in `bufferRegistry.ts`, which is loaded on demand with the
// editor (`bufferLoader.ts`), so the entry chunk does not carry CodeMirror.
import type { EditorState, Text } from "@codemirror/state";
import type { EditorSettings } from "../../ipc/types";

interface Entry {
  state: EditorState;
  /** The document as last loaded or saved: dirty = the live document differs (`Text.eq`, no hashing needed). */
  saved: Text;
}

const entries = new Map<string, Entry>();

export interface BufferStateOptions {
  tabSize: number;
  wordWrap: boolean;
  /** Show the line number gutter (default true). */
  lineNumbers?: boolean;
  /** Highlight the line with the cursor (default true). */
  highlightActiveLine?: boolean;
  /** Indent with a tab character instead of `tabSize` spaces (default false). */
  indentWithTabs?: boolean;
  /** Take tabs/spaces and the width from the file when it is unambiguous (default true). */
  detectIndentation?: boolean;
  /** Insert the closing bracket or quote too (default false). */
  autoCloseBrackets?: boolean;
  /** Highlight the bracket that matches the one at the cursor (default true). */
  bracketMatching?: boolean;
  /** Mark spaces and tabs (default false). */
  showWhitespace?: boolean;
  /** Fold gutter and fold commands (default true). */
  codeFolding?: boolean;
}

type OptionKeys =
  | "tabSize"
  | "wordWrap"
  | "lineNumbers"
  | "highlightActiveLine"
  | "indentWithTabs"
  | "detectIndentation"
  | "autoCloseBrackets"
  | "bracketMatching"
  | "showWhitespace"
  | "codeFolding";

/** The view-related editor settings as buffer options (defaults while the settings are not loaded). */
export function bufferOptionsFromSettings(editor: Partial<Pick<EditorSettings, OptionKeys>> | undefined): BufferStateOptions {
  return {
    tabSize: editor?.tabSize ?? 4,
    wordWrap: editor?.wordWrap ?? false,
    lineNumbers: editor?.lineNumbers ?? true,
    highlightActiveLine: editor?.highlightActiveLine ?? true,
    indentWithTabs: editor?.indentWithTabs ?? false,
    detectIndentation: editor?.detectIndentation ?? true,
    autoCloseBrackets: editor?.autoCloseBrackets ?? false,
    bracketMatching: editor?.bracketMatching ?? true,
    showWhitespace: editor?.showWhitespace ?? false,
    codeFolding: editor?.codeFolding ?? true,
  };
}

export function getBufferState(bufferId: string): EditorState | undefined {
  return entries.get(bufferId)?.state;
}

/** Stores the live state of a buffer (keeps the saved baseline). */
export function setBufferState(bufferId: string, state: EditorState): void {
  const entry = entries.get(bufferId);
  if (entry) entry.state = state;
  else entries.set(bufferId, { state, saved: state.doc });
}

/** A freshly loaded buffer: state and saved baseline are the same document. */
export function initBuffer(bufferId: string, state: EditorState): void {
  entries.set(bufferId, { state, saved: state.doc });
}

/** The document `doc` is now what the server has. */
export function markSaved(bufferId: string, doc: Text): void {
  const entry = entries.get(bufferId);
  if (entry) entry.saved = doc;
}

/** Does the live document differ from the last saved/loaded one? */
export function isDocDirty(bufferId: string): boolean {
  const entry = entries.get(bufferId);
  return entry ? !entry.state.doc.eq(entry.saved) : false;
}

const dropListeners: Array<(bufferId: string) => void> = [];

/** Registers a callback run whenever a buffer is dropped (modules with per-buffer maps clean up here). */
export function onBufferDropped(listener: (bufferId: string) => void): void {
  dropListeners.push(listener);
}

export function dropBuffer(bufferId: string): void {
  entries.delete(bufferId);
  for (const listener of dropListeners) listener(bufferId);
}

/** The remote file was renamed: keep state, undo history and baseline under the new id. */
export function moveBuffer(oldId: string, newId: string): void {
  const entry = entries.get(oldId);
  if (!entry || oldId === newId) return;
  entries.delete(oldId);
  entries.set(newId, entry);
}

/**
 * Replaces the stored state of every buffer except `exceptId` with `update(id, state)` (settings and language changes
 * reach the buffers that are not shown; the one in the view gets them through its own dispatch).
 */
export function updateStoredStates(update: (bufferId: string, state: EditorState) => EditorState, exceptId: string | null = null): void {
  for (const [id, entry] of entries) {
    if (id === exceptId) continue;
    entry.state = update(id, entry.state);
  }
}
