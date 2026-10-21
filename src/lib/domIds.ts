// Technical identifiers (selectors, menu item ids, status tokens). Not UI copy: kept out of the files that the
// "no literal string" lint rule covers.
export const SEL_MENU_ITEM = '[role="menuitem"]:not([aria-disabled="true"]), [role="menuitemcheckbox"]:not([aria-disabled="true"])';
export const SEL_TERMINAL_HOST = ".terminal-host[data-term-id]";
export const SEL_TERMINAL_TAB = "[data-terminal-tab]";
export const SEL_SESSION_TAB = "[data-session-tab]";
export const SEL_TEXT_FIELD = "input, textarea";
export const SEL_HOST_ROW = "[data-host-key]";
export const SEL_GROUP_HEADER = "[data-group-id]";

// Bookmark list drag and drop (pointerDrag.ts).
export const SEL_DROP_SECTION = "[data-drop-section]";
export const SEL_DROP_HEADER = "[data-drop-header]";
export const SEL_DROP_GROUP_SECTION = "[data-drop-section][data-drop-group]:not([data-drop-group=''])";

export const MENU_SELECT_ALL = "selectAll";
export const MENU_CLOSE_OTHERS = "closeOthers";


export const SEL_TREE_ITEM = "[data-tree-path]";
export const SEL_EDITOR_TAB = "[data-editor-tab]";
export const SEL_REMOTE_DIR = "[data-remote-dir]";
export const SEL_TREE_SESSION = "[data-tree-session]";
export const SEL_SELECTED_TAB = '[role="tab"][aria-selected="true"]';

export const MENU_COPY_PATH = "copyPath";
export const ID_NEW_FILE = "newFile";
export const ID_GO_INTO = "goInto";
export const ID_NEW_FOLDER = "newFolder";
export const ID_TERMINAL_HERE = "terminalHere";

// Selectors for focus movement, zoom and edit routing.
export const SEL_HOST_NAV = "[data-host-nav]";
export const SEL_SIDEBAR_TREE = ".sidebar .file-tree";
export const SEL_SIDEBAR_HOST_CURRENT = ".sidebar [data-host-nav][tabindex='0']";
export const SEL_SIDEBAR_HOST_ANY = ".sidebar [data-host-nav]";
export const SEL_TERMINAL_AREA = ".xterm, .terminal-panel";
export const SEL_EDITOR_AREA = ".cm-editor, .editor-area";
