// Edit menu: routes Undo / Redo / Cut / Copy / Paste / Find / Replace / Select all to whatever has keyboard focus (a
// terminal, the code editor or a text field). The menu bar keeps the previous focus while it is used, so
// `document.activeElement` is still the real target when an item is chosen.
import type { EditorView } from "@codemirror/view";
import { cmCommands, findCmView } from "../editor/cmBridge";
import { MENU_SELECT_ALL, SEL_TERMINAL_HOST, SEL_TEXT_FIELD } from "../../lib/domIds";
import { useStore } from "../../state/store";
import { codeMirrorItems, isEditableField, isTextField, terminalItems, textFieldItems } from "../menu/buildMenu";
import { loadedTerminalRegistry } from "../terminal/terminalLoader";

export type EditCommand = "undo" | "redo" | "cut" | "copy" | "paste" | "find" | "replace" | "selectAll";

const ITEM_ID: Partial<Record<EditCommand, string>> = {
  cut: "cut",
  copy: "copy",
  paste: "paste",
  selectAll: MENU_SELECT_ALL,
};

type Target =
  | { kind: "terminal"; termId: string }
  | { kind: "editor"; view: EditorView }
  | { kind: "field"; el: HTMLInputElement | HTMLTextAreaElement };

export function editTarget(from: Element | null = document.activeElement): Target | null {
  if (!from) return null;
  const host = from.closest<HTMLElement>(SEL_TERMINAL_HOST);
  if (host?.dataset["termId"]) return { kind: "terminal", termId: host.dataset["termId"] };
  const cm = from.closest<HTMLElement>(".cm-editor");
  if (cm) {
    const view = findCmView(cm);
    if (view) return { kind: "editor", view };
  }
  const field = from.closest(SEL_TEXT_FIELD);
  if (isTextField(field) && isEditableField(field)) return { kind: "field", el: field };
  return null;
}

/** Runs an Edit command on the focused target. Returns false when nothing editable has the focus. */
export function runEditCommand(command: EditCommand): boolean {
  const target = editTarget();
  if (!target) return false;
  const platform = useStore.getState().platform;

  if (command === "undo" || command === "redo") {
    if (target.kind === "editor") cmCommands()?.[command](target.view);
    else if (target.kind === "field") document.execCommand(command);
    return target.kind !== "terminal";
  }
  if (command === "find" || command === "replace") {
    if (target.kind === "editor") cmCommands()?.openSearchPanel(target.view);
    else if (target.kind === "terminal") useStore.getState().setTerminalSearch(target.termId);
    return target.kind !== "field";
  }

  const id = ITEM_ID[command];
  const items =
    target.kind === "terminal"
      ? loadedTerminalRegistry()
        ? terminalItems(target.termId, platform)
        : []
      : target.kind === "editor"
        ? codeMirrorItems(target.view)
        : textFieldItems(target.el);
  const item = items.find((i) => i.id === id);
  if (!item || item.disabled) return false;
  item.onSelect();
  return true;
}
