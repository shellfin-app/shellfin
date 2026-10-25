// Protocol identifiers used by the settings code under src/features (ESLint keeps literal strings out of features;
// these are data values of the contract, not UI copy).
import type { ConfirmQuit, NewTerminalCwd, ShowWhitespace, TerminalExitAction } from "../ipc/types";

export const CONFIRM_QUIT = {
  always: "always",
  withConnections: "withConnections",
  never: "never",
} as const satisfies Record<ConfirmQuit, ConfirmQuit>;

export const TERMINAL_EXIT = {
  keep: "keep",
  closeOnSuccess: "closeOnSuccess",
  close: "close",
} as const satisfies Record<TerminalExitAction, TerminalExitAction>;

export const NEW_TERMINAL_CWD = {
  home: "home",
  currentFolder: "currentFolder",
} as const satisfies Record<NewTerminalCwd, NewTerminalCwd>;

export const SHOW_WHITESPACE = {
  all: "all",
  activeLine: "activeLine",
  none: "none",
} as const satisfies Record<ShowWhitespace, ShowWhitespace>;

/** CodeMirror's whitespace marker classes (`highlightWhitespace`); the active-line-only mode reuses them. */
export const CM_WHITESPACE_CLASS = { space: "cm-highlightSpace", tab: "cm-highlightTab" } as const;

/** The placeholders the backend fills into notification texts (ARCHITECTURE §8.11). They must reach it unchanged. */
export const NOTIFICATION_PLACEHOLDERS = { name: "{{name}}", count: "{{count}}" } as const;

/** CSI prefix of the DEC private modes (`CSI ? Pm h`). */
export const CSI_PRIVATE_PREFIX = "?";
/** Final byte of "set mode" (DECSET); "l" resets and always passes. */
export const CSI_SET_FINAL = "h";

/** Separates the user from the host in a `user@host` target. */
export const USER_HOST_SEPARATOR = "@";
