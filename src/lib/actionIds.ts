// The one list of app actions. Menus, the command palette, keyboard shortcuts, context menus and the
// native macOS menu (`menu:action` events) all refer to these ids. The labels live in translation.json under
// `action.<id>`; the menu names under `menubar.<id>`. Technical identifiers only: this file is not UI copy.
// WARNING: imported directly by Node (`scripts/menu-mnemonics-check.mjs`, type stripping): erasable TypeScript syntax
// only, and value imports need explicit `.ts` extensions.
import type { Platform } from "../ipc/types";

export const ACTION_IDS = [
  // File
  "newConnection",
  "importBookmarks",
  "exportBookmarks",
  "save",
  "saveAll",
  "closeTab",
  "closeConnection",
  "settings",
  "exit",
  // Edit
  "undo",
  "redo",
  "cut",
  "copy",
  "paste",
  "find",
  "replace",
  "selectAll",
  // View
  "toggleSidebar",
  "toggleTerminalPanel",
  "toggleMaximizePanel",
  "toggleBookmarks",
  "toggleStatusBar",
  "commandPalette",
  "zoomIn",
  "zoomOut",
  "zoomReset",
  "fullScreen",
  // Connection
  "reconnect",
  "disconnect",
  "duplicateConnection",
  "saveAsBookmark",
  "goToFolder",
  "uploadFiles",
  "showTransfers",
  "editConnection",
  "copySshCommand",
  "manageKeys",
  // Terminal
  "newTerminal",
  "closeTerminal",
  "renameTerminal",
  "restartTerminal",
  "clearTerminal",
  "findInTerminal",
  "saveTerminalOutput",
  // Help
  "keyboardShortcuts",
  "documentation",
  "reportProblem",
  "securityPolicy",
  "openLogs",
  "checkForUpdates",
  "about",
  // Focus (shortcuts only, also in the palette)
  "focusFiles",
  "focusEditor",
  "focusTerminal",
  "focusNextRegion",
  "focusPrevRegion",
] as const;

export type ActionId = (typeof ACTION_IDS)[number];

export const MENU_IDS = ["file", "edit", "view", "connection", "terminal", "help"] as const;
export type MenuId = (typeof MENU_IDS)[number];

export type MenuEntry = ActionId | "-";

/** Menu bar content. A "-" is a separator. The mnemonic of a menu is the first letter of its id. */
export const MENUS: Record<MenuId, readonly MenuEntry[]> = {
  file: ["newConnection", "-", "importBookmarks", "exportBookmarks", "-", "save", "saveAll", "-", "closeTab", "closeConnection", "-", "settings", "-", "exit"],
  edit: ["undo", "redo", "-", "cut", "copy", "paste", "-", "find", "replace", "-", "selectAll"],
  view: [
    "commandPalette",
    "-",
    "toggleSidebar",
    "toggleBookmarks",
    "toggleTerminalPanel",
    "toggleMaximizePanel",
    "toggleStatusBar",
    "-",
    "zoomIn",
    "zoomOut",
    "zoomReset",
    "-",
    "fullScreen",
  ],
  connection: [
    "reconnect",
    "disconnect",
    "duplicateConnection",
    "-",
    "editConnection",
    "saveAsBookmark",
    "copySshCommand",
    "-",
    "goToFolder",
    "uploadFiles",
    "showTransfers",
    "-",
    "manageKeys",
  ],
  terminal: ["newTerminal", "closeTerminal", "renameTerminal", "restartTerminal", "-", "clearTerminal", "findInTerminal", "saveTerminalOutput"],
  help: ["keyboardShortcuts", "documentation", "reportProblem", "securityPolicy", "-", "openLogs", "-", "checkForUpdates", "about"],
};

/** Cheat-sheet sections (Help > Keyboard shortcuts): the menus, plus the focus shortcuts. */
export const CHEAT_SHEET: readonly { id: string; actions: readonly ActionId[] }[] = [
  ...MENU_IDS.map((id) => ({ id: `menu:${id}`, actions: MENUS[id].filter((e): e is ActionId => e !== "-") })),
  { id: "focus", actions: ["focusFiles", "focusEditor", "focusTerminal", "focusNextRegion", "focusPrevRegion"] },
];

/** Display strings, [Windows/Linux, macOS]. Matching lives in `keybindings.ts`; this is only what the UI shows. */
const SHORTCUT_LABELS: Partial<Record<ActionId, readonly [string, string]>> = {
  newConnection: ["Ctrl+N", "⌘N"],
  save: ["Ctrl+S", "⌘S"],
  saveAll: ["Ctrl+Alt+S", "⌥⌘S"],
  closeTab: ["Ctrl+W", "⌘W"],
  settings: ["Ctrl+,", "⌘,"],
  find: ["Ctrl+F", "⌘F"],
  undo: ["Ctrl+Z", "⌘Z"],
  redo: ["Ctrl+Y", "⇧⌘Z"],
  cut: ["Ctrl+X", "⌘X"],
  copy: ["Ctrl+C", "⌘C"],
  paste: ["Ctrl+V", "⌘V"],
  selectAll: ["Ctrl+A", "⌘A"],
  toggleSidebar: ["Ctrl+B", "⌘B"],
  toggleTerminalPanel: ["Ctrl+`", "⌃`"],
  commandPalette: ["Ctrl+K", "⌘K"],
  zoomIn: ["Ctrl+=", "⌘="],
  zoomOut: ["Ctrl+-", "⌘-"],
  zoomReset: ["Ctrl+0", "⌘0"],
  fullScreen: ["F11", "⌃⌘F"],
  goToFolder: ["Ctrl+L", "⌘L"],
  newTerminal: ["Ctrl+Shift+`", "⌃⇧`"],
  findInTerminal: ["Ctrl+Shift+F", "⌘F"],
  focusFiles: ["Ctrl+Shift+E", "⇧⌘E"],
  focusEditor: ["Ctrl+Shift+Y", "⇧⌘Y"],
  focusTerminal: ["Ctrl+Shift+J", "⇧⌘J"],
  focusNextRegion: ["F6", "F6"],
  focusPrevRegion: ["Shift+F6", "⇧F6"],
};

export function shortcutLabel(id: ActionId, platform: Platform): string | undefined {
  const pair = SHORTCUT_LABELS[id];
  return pair ? pair[platform === "macos" ? 1 : 0] : undefined;
}

/** `A.save` instead of the string "save": action ids written in components stay checked by the compiler. */
export const A = Object.fromEntries(ACTION_IDS.map((id) => [id, id])) as { readonly [K in ActionId]: K };
