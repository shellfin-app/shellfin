// Chooses the context menu for the element that was right-clicked. The native WebView menu is never
// shown: `contextmenu` is always cancelled, and these are the app's own menus.
import type { EditorView } from "@codemirror/view";
import { cmCommands, findCmView } from "../editor/cmBridge";
import i18n from "i18next";
import type { Platform } from "../../ipc/types";
import {
  MENU_CLOSE_OTHERS,
  MENU_COPY_PATH,
  MENU_SELECT_ALL,
  SEL_EDITOR_TAB,
  SEL_HOST_NAV,
  SEL_GROUP_HEADER,
  SEL_HOST_ROW,
  SEL_SESSION_TAB,
  SEL_TERMINAL_HOST,
  SEL_TERMINAL_TAB,
  SEL_TEXT_FIELD,
  SEL_TREE_ITEM,
} from "../../lib/domIds";
import { isMac } from "../../lib/keybindings";
import { guard } from "../../lib/reportError";
import { useStore } from "../../state/store";
import { copyPath } from "../files/fileActions";
import { treeMenuItems } from "../files/treeMenu";
import { requestCloseBuffer, saveBuffer } from "../editor/editorActions";
import { describeTarget } from "../../lib/targetInfo";
import { groupMenuItems, hostMenuItems } from "../hosts/hostMenu";
import {
  canDisconnect,
  canReconnect,
  canSaveAsBookmark,
  disconnectSession,
  duplicateSession,
  reconnectSession,
  saveSessionAsBookmark,
} from "../session/connectionActions";
import { requestCloseSession } from "../session/sessionActions";
import { saveOutputOf } from "../terminal/saveOutput";
import { requestCloseTerminalTabs } from "../terminal/terminalActions";
import { loadedTerminalRegistry } from "../terminal/terminalLoader";
import type { MenuItem } from "./menuStore";

type TextField = HTMLInputElement | HTMLTextAreaElement;

const t = (key: string): string => (i18n.t as (k: string) => string)(key);

function writeClipboard(text: string): void {
  if (text !== "") navigator.clipboard.writeText(text).catch(() => undefined);
}

export function isTextField(el: Element | null): el is TextField {
  return el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement;
}

/** Inputs that cannot hold free text (checkboxes, buttons ...) get no text menu. */
export function isEditableField(el: TextField): boolean {
  if (el instanceof HTMLTextAreaElement) return true;
  return ["text", "search", "url", "tel", "email", "password", "number", ""].includes(el.type);
}

function fieldSelectionText(el: TextField): string {
  const start = el.selectionStart ?? 0;
  const end = el.selectionEnd ?? 0;
  return el.value.slice(Math.min(start, end), Math.max(start, end));
}

function replaceFieldSelection(el: TextField, text: string): void {
  el.focus();
  el.setRangeText(text, el.selectionStart ?? el.value.length, el.selectionEnd ?? el.value.length, "end");
  el.dispatchEvent(new Event("input", { bubbles: true })); // React's onChange listens for this
}

export function textFieldItems(el: TextField): MenuItem[] {
  const secret = el instanceof HTMLInputElement && el.type === "password";
  const locked = el.readOnly || el.disabled;
  const hasSelection = fieldSelectionText(el) !== "";
  return [
    {
      id: "cut",
      label: t("menu.cut"),
      disabled: locked || secret || !hasSelection,
      onSelect: () => {
        writeClipboard(fieldSelectionText(el));
        replaceFieldSelection(el, "");
      },
    },
    { id: "copy", label: t("menu.copy"), disabled: secret || !hasSelection, onSelect: () => writeClipboard(fieldSelectionText(el)) },
    {
      id: "paste",
      label: t("menu.paste"),
      disabled: locked,
      onSelect: () => guard(navigator.clipboard.readText().then((text) => replaceFieldSelection(el, text))),
    },
    { id: MENU_SELECT_ALL, label: t("menu.selectAll"), separatorBefore: true, onSelect: () => (el.focus(), el.select()) },
  ];
}

export function codeMirrorItems(view: EditorView): MenuItem[] {
  const selected = () => view.state.sliceDoc(view.state.selection.main.from, view.state.selection.main.to);
  const hasSelection = !view.state.selection.main.empty;
  const readOnly = view.state.readOnly;
  return [
    {
      id: "cut",
      label: t("menu.cut"),
      disabled: readOnly || !hasSelection,
      onSelect: () => {
        writeClipboard(selected());
        view.dispatch(view.state.replaceSelection(""));
      },
    },
    { id: "copy", label: t("menu.copy"), disabled: !hasSelection, onSelect: () => writeClipboard(selected()) },
    {
      id: "paste",
      label: t("menu.paste"),
      disabled: readOnly,
      onSelect: () =>
        guard(navigator.clipboard.readText().then((text) => view.dispatch(view.state.replaceSelection(text), { userEvent: "input.paste" }))),
    },
    { id: MENU_SELECT_ALL, label: t("menu.selectAll"), separatorBefore: true, onSelect: () => (view.focus(), cmCommands()?.selectAll(view)) },
  ];
}

export function terminalItems(termId: string, platform: Platform): MenuItem[] {
  const registry = loadedTerminalRegistry();
  if (!registry) return [];
  const mac = isMac(platform);
  return [
    {
      id: "copy",
      label: t("menu.copy"),
      shortcut: t(mac ? "shortcut.terminalCopyMac" : "shortcut.terminalCopy"),
      disabled: !registry.terminalHasSelection(termId),
      onSelect: () => registry.copyTerminalSelection(termId),
    },
    {
      id: "paste",
      label: t("menu.paste"),
      shortcut: t(mac ? "shortcut.terminalPasteMac" : "shortcut.terminalPaste"),
      onSelect: () => guard(registry.pasteClipboardInto(termId)),
    },
    { id: MENU_SELECT_ALL, label: t("menu.selectAll"), onSelect: () => registry.selectAllInTerminal(termId) },
    {
      id: "find",
      label: t("menu.find"),
      shortcut: t(mac ? "shortcut.terminalFindMac" : "shortcut.terminalFind"),
      separatorBefore: true,
      onSelect: () => useStore.getState().setTerminalSearch(termId),
    },
    { id: "clear", label: t("menu.clear"), onSelect: () => registry.clearTerminal(termId) },
    { id: "saveoutput", label: t("action.saveTerminalOutput"), separatorBefore: true, onSelect: () => guard(saveOutputOf(termId)) },
  ];
}

function terminalTabItems(sessionId: string, termId: string): MenuItem[] {
  const others = (useStore.getState().workspaces[sessionId]?.terminalTabs ?? []).filter((id) => id !== termId);
  return [
    { id: "rename", label: t("menu.rename"), onSelect: () => useStore.getState().setRenamingTerminal(termId) },
    { id: "saveoutput", label: t("action.saveTerminalOutput"), onSelect: () => guard(saveOutputOf(termId)) },
    { id: "close", label: t("menu.close"), separatorBefore: true, onSelect: () => requestCloseTerminalTabs(sessionId, [termId]) },
    {
      id: MENU_CLOSE_OTHERS,
      label: t("menu.closeOthers"),
      disabled: others.length === 0,
      onSelect: () => requestCloseTerminalTabs(sessionId, others),
    },
  ];
}

function editorTabItems(bufferId: string): MenuItem[] {
  const meta = useStore.getState().buffers[bufferId];
  if (!meta) return [];
  return [
    { id: "save", label: t("menu.save"), disabled: !meta.dirty || meta.load !== "ready", onSelect: () => guard(saveBuffer(bufferId)) },
    { id: "close", label: t("menu.close"), onSelect: () => requestCloseBuffer(bufferId) },
    { id: MENU_COPY_PATH, label: t("files.copyPath"), separatorBefore: true, onSelect: () => copyPath(meta.path) },
  ];
}

/** `user@host` of a session: the connected hop, else what its target says. */
function sessionUserAtHost(sessionId: string): string | null {
  const state = useStore.getState();
  const entry = state.sessionsById[sessionId];
  if (!entry) return null;
  const hop = entry.info?.hops[entry.info.hops.length - 1];
  if (hop) return `${hop.user}@${hop.host}`;
  const info = describeTarget(entry.target, state.bookmarks?.bookmarks ?? [], state.sshConfig?.hosts ?? []);
  return info.host ? info.host : null;
}

function sessionTabItems(sessionId: string): MenuItem[] {
  const state = useStore.getState();
  const others = state.sessionOrder.filter((id) => id !== sessionId);
  const userAtHost = sessionUserAtHost(sessionId);
  return [
    { id: "reconnect", label: t("action.reconnect"), disabled: !canReconnect(state, sessionId), onSelect: () => reconnectSession(sessionId) },
    { id: "disconnect", label: t("action.disconnect"), disabled: !canDisconnect(state, sessionId), onSelect: () => disconnectSession(sessionId) },
    { id: "duplicate", label: t("action.duplicateConnection"), onSelect: () => duplicateSession(sessionId) },
    {
      id: "saveasbookmark",
      label: t("action.saveAsBookmark"),
      disabled: !canSaveAsBookmark(state, sessionId),
      onSelect: () => saveSessionAsBookmark(sessionId),
    },
    {
      id: "copyuserathost",
      label: t("menu.copyUserAtHost"),
      disabled: userAtHost === null,
      onSelect: () => userAtHost && writeClipboard(userAtHost),
    },
    { id: "rename", label: t("menu.rename"), separatorBefore: true, onSelect: () => state.setRenamingSession(sessionId) },
    { id: "close", label: t("menu.close"), onSelect: () => requestCloseSession(sessionId) },
    {
      id: MENU_CLOSE_OTHERS,
      label: t("menu.closeOthers"),
      disabled: others.length === 0,
      onSelect: () => {
        for (const id of others) requestCloseSession(id);
      },
    },
  ];
}

export interface BuiltMenu {
  items: MenuItem[];
  /** Where focus should go when the menu closes without an action that moves it. */
  restoreFocus(): void;
}

/** The menu for a right-click on `target` (empty `items` = nothing to offer; the click is still swallowed). */
export function buildMenu(target: Element, platform: Platform): BuiltMenu {
  const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const restoreTo = (el: HTMLElement | null) => () => {
    if (el?.isConnected) el.focus();
  };

  const hostRow = target.closest<HTMLElement>(SEL_HOST_ROW);
  if (hostRow?.dataset["hostKey"]) {
    // A right-click selects the row (focus moves to it) like it does in the file tree.
    hostRow.querySelector<HTMLElement>(SEL_HOST_NAV)?.focus();
    return { items: hostMenuItems(hostRow.dataset["hostKey"]), restoreFocus: restoreTo(previous) };
  }

  const groupHeader = target.closest<HTMLElement>(SEL_GROUP_HEADER);
  if (groupHeader?.dataset["groupId"]) {
    groupHeader.focus();
    return { items: groupMenuItems(groupHeader.dataset["groupId"]), restoreFocus: restoreTo(previous) };
  }

  const terminalHost = target.closest<HTMLElement>(SEL_TERMINAL_HOST);
  if (terminalHost?.dataset["termId"]) {
    const termId = terminalHost.dataset["termId"];
    return {
      items: terminalItems(termId, platform),
      restoreFocus: () => loadedTerminalRegistry()?.focusTerminal(termId),
    };
  }

  const terminalTab = target.closest<HTMLElement>(SEL_TERMINAL_TAB);
  if (terminalTab?.dataset["terminalTab"] && terminalTab.dataset["sessionId"]) {
    return {
      items: terminalTabItems(terminalTab.dataset["sessionId"], terminalTab.dataset["terminalTab"]),
      restoreFocus: restoreTo(previous),
    };
  }

  const sessionTab = target.closest<HTMLElement>(SEL_SESSION_TAB);
  if (sessionTab?.dataset["sessionTab"]) {
    return { items: sessionTabItems(sessionTab.dataset["sessionTab"]), restoreFocus: restoreTo(previous) };
  }

  const treeItem = target.closest<HTMLElement>(SEL_TREE_ITEM);
  const treeSession = treeItem?.dataset["treeSession"];
  const treePath = treeItem?.dataset["treePath"];
  if (treeSession && treePath) {
    useStore.getState().selectTreeEntry(treeSession, treePath);
    return {
      items: treeMenuItems(treeSession, treePath),
      restoreFocus: () =>
        [...document.querySelectorAll<HTMLElement>(".file-tree")].find((el) => el.dataset["treeSession"] === treeSession)?.focus(),
    };
  }

  const editorTab = target.closest<HTMLElement>(SEL_EDITOR_TAB);
  const tabBuffer = editorTab?.dataset["editorTab"];
  if (tabBuffer) return { items: editorTabItems(tabBuffer), restoreFocus: restoreTo(previous) };

  const cm = target.closest<HTMLElement>(".cm-editor");
  if (cm) {
    const view = findCmView(cm);
    if (view) return { items: codeMirrorItems(view), restoreFocus: () => view.focus() };
  }

  const field = target.closest(SEL_TEXT_FIELD);
  if (isTextField(field) && isEditableField(field)) {
    return { items: textFieldItems(field), restoreFocus: restoreTo(field) };
  }

  // Selectable text elsewhere (fingerprints, hints, error text): offer Copy for a non-empty selection.
  const selection = window.getSelection()?.toString() ?? "";
  if (selection !== "") {
    return {
      items: [{ id: "copy", label: t("menu.copy"), onSelect: () => writeClipboard(selection) }],
      restoreFocus: restoreTo(previous),
    };
  }
  return { items: [], restoreFocus: restoreTo(previous) };
}
