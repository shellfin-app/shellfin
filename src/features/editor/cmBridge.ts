// What the menus and the Edit commands need from CodeMirror without importing it: the mounted editor
// registers its view and the few commands here, so the entry chunk can stay free of CodeMirror. A focused `.cm-editor`
// implies the editor chunk is loaded, so the commands are always there when a view is.
import type { EditorView } from "@codemirror/view";

export interface CmCommands {
  undo(view: EditorView): boolean;
  redo(view: EditorView): boolean;
  selectAll(view: EditorView): boolean;
  openSearchPanel(view: EditorView): boolean;
}

let commands: CmCommands | null = null;
const views = new WeakMap<Element, EditorView>();

/** Called once by the editor module when it loads. */
export function registerCmCommands(c: CmCommands): void {
  commands = c;
}

export function cmCommands(): CmCommands | null {
  return commands;
}

/** The editor registers its view under its DOM root (`view.dom`, the `.cm-editor` element) and unregisters on destroy. */
export function registerCmView(view: EditorView, registered: boolean): void {
  if (registered) views.set(view.dom, view);
  else views.delete(view.dom);
}

/** The EditorView whose root is `dom` (replaces `EditorView.findFromDOM`). */
export function findCmView(dom: Element): EditorView | undefined {
  return views.get(dom);
}
