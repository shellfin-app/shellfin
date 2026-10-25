// Settings form <-> contract, with the ranges of docs/ARCHITECTURE.md §4.4 (the backend re-validates).
// One flat form model for every section of the Settings dialog; `look` (appearance and the terminal background) is the
// one nested part and lives in `settingsFormAppearance.ts`.
import type {
  ConfirmQuit,
  ConflictPolicy,
  CursorStyle,
  DateFormat,
  DoubleClickAction,
  LogLevel,
  NewFileEol,
  NewTerminalCwd,
  RightClickAction,
  Settings,
  ShowWhitespace,
  SizeUnits,
  StartupAction,
  TermType,
  TerminalBell,
  TerminalExitAction,
} from "../ipc/types";
import { fieldFromKiBps, kibpsFromField, type BandwidthField } from "./bandwidth";
import { validateFontFamily } from "./fontFamily";
import { applyAppearance, appearanceFromSettings, appearanceInvalidFields, type AppearanceForm } from "./settingsFormAppearance";
import { showWhitespaceMode } from "./showWhitespace";

/** Field path of the terminal font family in `invalidFields` / backend `invalidArgument{field}`. */
export const FONT_FAMILY_FIELD = "terminal.fontFamily";
/** Same for the editor font family. */
export const EDITOR_FONT_FAMILY_FIELD = "editor.fontFamily";
/** Terminal line height: a decimal, so it is kept apart from the integer fields. */
export const LINE_HEIGHT_FIELD = "terminal.lineHeight";
export const LINE_HEIGHT_RANGE = { min: 1.0, max: 2.0 } as const;

/** Parses the line-height text (decimal point or comma); `null` when it is not a number in range. */
export function parseLineHeight(text: string): number | null {
  const normalized = text.trim().replace(",", ".");
  if (!/^\d+(\.\d+)?$/.test(normalized)) return null;
  const n = Number(normalized);
  return n >= LINE_HEIGHT_RANGE.min && n <= LINE_HEIGHT_RANGE.max ? n : null;
}

export interface NumberRule {
  min: number;
  max: number;
}

/** Field paths as constants (UI code refers to fields by these names). */
export const FIELD = {
  TERMINAL_FONT_SIZE: "terminal.fontSize",
  TERMINAL_SCROLLBACK: "terminal.scrollback",
  LETTER_SPACING: "terminal.letterSpacing",
  SCROLL_SENSITIVITY: "terminal.scrollSensitivity",
  MIN_CONTRAST: "terminal.minimumContrastRatio",
  WORD_SEPARATORS: "terminal.wordSeparators",
  EDITOR_FONT_SIZE: "editor.fontSize",
  EDITOR_TAB_SIZE: "editor.tabSize",
  EDITOR_MAX_FILE: "editor.maxFileSizeMiB",
  KEEPALIVE_INTERVAL: "connection.keepaliveIntervalSecs",
  KEEPALIVE_COUNT: "connection.keepaliveCountMax",
  CONNECT_TIMEOUT: "connection.connectTimeoutSecs",
  PROMPT_TIMEOUT: "connection.promptTimeoutSecs",
  RECONNECT_ATTEMPTS: "connection.reconnect.maxAttempts",
  DEFAULT_USER: "connection.defaultUser",
  MAX_CONCURRENT: "transfers.maxConcurrent",
} as const;

/** Integer fields kept as text while editing: field path (as in the backend's `invalidArgument{field}`) -> range. */
export const SETTINGS_RANGES: Record<string, NumberRule> = {
  "terminal.fontSize": { min: 8, max: 32 },
  "terminal.scrollback": { min: 10, max: 1000000 },
  "terminal.letterSpacing": { min: 0, max: 5 },
  "terminal.scrollSensitivity": { min: 1, max: 10 },
  "editor.fontSize": { min: 8, max: 32 },
  "editor.tabSize": { min: 1, max: 8 },
  "editor.maxFileSizeMiB": { min: 1, max: 256 },
  "connection.keepaliveIntervalSecs": { min: 5, max: 300 },
  "connection.keepaliveCountMax": { min: 1, max: 10 },
  "connection.connectTimeoutSecs": { min: 3, max: 120 },
  "connection.promptTimeoutSecs": { min: 30, max: 110 },
  "connection.reconnect.maxAttempts": { min: 0, max: 10 },
  "transfers.maxConcurrent": { min: 1, max: 64 },
};

export const CONTRAST_RANGE = { min: 1, max: 21 } as const;
export const WORD_SEPARATORS_MAX = 64;
export const DEFAULT_USER_MAX = 255;
export const AUTO_REFRESH_CHOICES = [0, 5, 10, 30, 60] as const;

/** Terminal scrollback above this shows the memory hint (xterm keeps ~12 bytes per cell). */
export const SCROLLBACK_MEMORY_HINT_FROM = 100_000;
/** Editor size above this shows the warning. */
export const MAX_FILE_WARN_FROM_MIB = 50;
/** Bytes xterm needs per cell of scrollback (rough). */
const BYTES_PER_CELL = 12;
const ASSUMED_COLUMNS = 200;

/** Approximate memory of a full scrollback in MiB (lines x columns x 12 bytes), for the hint. */
export function scrollbackMemoryMiB(lines: number, columns = ASSUMED_COLUMNS): number {
  return Math.round((lines * columns * BYTES_PER_CELL) / (1024 * 1024));
}

export interface SettingsFormState {
  // General
  locale: string;
  confirmCloseWithTerminals: boolean;
  confirmQuit: ConfirmQuit;
  onStartup: StartupAction;
  notifyOnDisconnect: boolean;
  /** Automatic update checks. */
  autoUpdate: boolean;
  // Integer fields as text (`SETTINGS_RANGES`)
  numbers: Record<string, string>;
  // Terminal
  terminalCursorBlink: boolean;
  terminalCopyOnSelect: boolean;
  /** `null` = the bundled font; else the family text as typed/picked (checked by `validateFontFamily` on save). */
  terminalFontFamily: string | null;
  terminalCursorStyle: CursorStyle;
  /** Raw text of the line height (checked by `parseLineHeight`). */
  terminalLineHeight: string;
  terminalBell: TerminalBell;
  terminalRightClick: RightClickAction;
  /** Raw text of the minimum contrast ratio (decimal). */
  minimumContrastRatio: string;
  wordSeparators: string;
  mouseReporting: boolean;
  hideMousePointerWhileTyping: boolean;
  multilinePasteWarning: boolean;
  onExit: TerminalExitAction;
  newTerminalCwd: NewTerminalCwd;
  openTerminalOnConnect: boolean;
  termType: TermType;
  altIsMeta: boolean;
  screenReaderMode: boolean;
  // Editor
  editorWordWrap: boolean;
  editorFontFamily: string | null;
  editorLineNumbers: boolean;
  editorHighlightActiveLine: boolean;
  editorIndentWithTabs: boolean;
  editorTrimTrailingWhitespace: boolean;
  editorInsertFinalNewline: boolean;
  detectIndentation: boolean;
  newFileEol: NewFileEol;
  autoCloseBrackets: boolean;
  bracketMatching: boolean;
  showWhitespace: ShowWhitespace;
  codeFolding: boolean;
  fontLigatures: boolean;
  // Files
  showHidden: boolean;
  foldersFirst: boolean;
  confirmDelete: boolean;
  doubleClick: DoubleClickAction;
  sizeUnits: SizeUnits;
  dateFormat: DateFormat;
  showPermissions: boolean;
  /** Seconds as text of a <select> value (`"0"` = off). */
  autoRefresh: string;
  // Connection
  reconnectEnabled: boolean;
  defaultUser: string;
  readSshConfig: boolean;
  rememberPasswords: boolean;
  // Transfers
  onConflict: ConflictPolicy;
  /** Bandwidth limits (0 = unlimited). */
  uploadLimit: BandwidthField;
  downloadLimit: BandwidthField;
  preserveTimestamps: boolean;
  notifyOnComplete: boolean;
  // Advanced
  logLevel: LogLevel;
  /** `appearance.*` and `terminal.background` (`settingsFormAppearance.ts`). */
  look: AppearanceForm;
}

export const BANDWIDTH_FIELDS = { UPLOAD: "transfers.uploadLimitKiBps", DOWNLOAD: "transfers.downloadLimitKiBps" } as const;

function get(settings: Settings, path: string): number {
  switch (path) {
    case "terminal.fontSize":
      return settings.terminal.fontSize;
    case "terminal.scrollback":
      return settings.terminal.scrollback;
    case "terminal.letterSpacing":
      return settings.terminal.letterSpacing;
    case "terminal.scrollSensitivity":
      return settings.terminal.scrollSensitivity;
    case "editor.fontSize":
      return settings.editor.fontSize;
    case "editor.tabSize":
      return settings.editor.tabSize;
    case "editor.maxFileSizeMiB":
      return settings.editor.maxFileSizeMiB;
    case "connection.keepaliveIntervalSecs":
      return settings.connection.keepaliveIntervalSecs;
    case "connection.keepaliveCountMax":
      return settings.connection.keepaliveCountMax;
    case "connection.connectTimeoutSecs":
      return settings.connection.connectTimeoutSecs;
    case "connection.promptTimeoutSecs":
      return settings.connection.promptTimeoutSecs;
    case "transfers.maxConcurrent":
      return settings.transfers.maxConcurrent;
    default:
      return settings.connection.reconnect.maxAttempts;
  }
}

export function formFromSettings(settings: Settings): SettingsFormState {
  const numbers: Record<string, string> = {};
  for (const path of Object.keys(SETTINGS_RANGES)) numbers[path] = String(get(settings, path));
  return {
    locale: settings.locale,
    confirmCloseWithTerminals: settings.general.confirmCloseWithTerminals,
    confirmQuit: settings.general.confirmQuit,
    onStartup: settings.general.onStartup,
    notifyOnDisconnect: settings.general.notifyOnDisconnect,
    autoUpdate: settings.general.autoUpdate,
    numbers,
    terminalCursorBlink: settings.terminal.cursorBlink,
    terminalCopyOnSelect: settings.terminal.copyOnSelect,
    terminalFontFamily: settings.terminal.fontFamily,
    terminalCursorStyle: settings.terminal.cursorStyle,
    terminalLineHeight: String(settings.terminal.lineHeight),
    terminalBell: settings.terminal.bell,
    terminalRightClick: settings.terminal.rightClick,
    minimumContrastRatio: String(settings.terminal.minimumContrastRatio),
    wordSeparators: settings.terminal.wordSeparators,
    mouseReporting: settings.terminal.mouseReporting,
    hideMousePointerWhileTyping: settings.terminal.hideMousePointerWhileTyping,
    multilinePasteWarning: settings.terminal.multilinePasteWarning,
    onExit: settings.terminal.onExit,
    newTerminalCwd: settings.terminal.newTerminalCwd,
    openTerminalOnConnect: settings.terminal.openTerminalOnConnect,
    termType: settings.terminal.termType,
    altIsMeta: settings.terminal.altIsMeta,
    screenReaderMode: settings.terminal.screenReaderMode,
    editorWordWrap: settings.editor.wordWrap,
    editorFontFamily: settings.editor.fontFamily,
    editorLineNumbers: settings.editor.lineNumbers,
    editorHighlightActiveLine: settings.editor.highlightActiveLine,
    editorIndentWithTabs: settings.editor.indentWithTabs,
    editorTrimTrailingWhitespace: settings.editor.trimTrailingWhitespace,
    editorInsertFinalNewline: settings.editor.insertFinalNewline,
    detectIndentation: settings.editor.detectIndentation,
    newFileEol: settings.editor.newFileEol,
    autoCloseBrackets: settings.editor.autoCloseBrackets,
    bracketMatching: settings.editor.bracketMatching,
    showWhitespace: showWhitespaceMode(settings.editor.showWhitespace),
    codeFolding: settings.editor.codeFolding,
    fontLigatures: settings.editor.fontLigatures,
    showHidden: settings.files.showHidden,
    foldersFirst: settings.files.foldersFirst,
    confirmDelete: settings.files.confirmDelete,
    doubleClick: settings.files.doubleClick,
    sizeUnits: settings.files.sizeUnits,
    dateFormat: settings.files.dateFormat,
    showPermissions: settings.files.showPermissions,
    autoRefresh: String(settings.files.autoRefresh),
    reconnectEnabled: settings.connection.reconnect.enabled,
    defaultUser: settings.connection.defaultUser,
    readSshConfig: settings.connection.readSshConfig,
    rememberPasswords: settings.connection.rememberPasswords,
    onConflict: settings.transfers.onConflict,
    uploadLimit: fieldFromKiBps(settings.transfers.uploadLimitKiBps),
    downloadLimit: fieldFromKiBps(settings.transfers.downloadLimitKiBps),
    preserveTimestamps: settings.transfers.preserveTimestamps,
    notifyOnComplete: settings.transfers.notifyOnComplete,
    logLevel: settings.advanced.logLevel,
    look: appearanceFromSettings(settings),
  };
}

/** The contrast text (decimal point or comma) as a number in 1-21, else `null`. */
export function parseContrast(text: string): number | null {
  const normalized = text.trim().replace(",", ".");
  if (!/^\d+(\.\d+)?$/.test(normalized)) return null;
  const n = Number(normalized);
  return n >= CONTRAST_RANGE.min && n <= CONTRAST_RANGE.max ? n : null;
}

/** 0-64 printable ASCII characters (0x20-0x7E), the backend rule. Compared numerically. */
export function validWordSeparators(text: string): boolean {
  if (text.length > WORD_SEPARATORS_MAX) return false;
  for (let i = 0; i < text.length; i += 1) {
    const code = text.charCodeAt(i);
    if (code < 0x20 || code > 0x7e) return false;
  }
  return true;
}

/** Same rule as a bookmark user, empty allowed: 1-255 characters, no whitespace or control characters. */
export function validDefaultUser(text: string): boolean {
  if (text === "") return true;
  if (text.length > DEFAULT_USER_MAX) return false;
  for (const ch of text) {
    const code = ch.codePointAt(0) ?? 0;
    if (code <= 0x20 || (code >= 0x7f && code <= 0x9f) || /\s/u.test(ch)) return false;
  }
  return true;
}

/** Field paths whose value is not usable (an integer outside its range, a bad font, colour, separators, ...). */
export function invalidFields(form: SettingsFormState): string[] {
  const bandwidth = [
    ...(kibpsFromField(form.uploadLimit) === null ? [BANDWIDTH_FIELDS.UPLOAD] : []),
    ...(kibpsFromField(form.downloadLimit) === null ? [BANDWIDTH_FIELDS.DOWNLOAD] : []),
  ];
  const numbers = Object.entries(SETTINGS_RANGES)
    .filter(([path, rule]) => {
      const text = (form.numbers[path] ?? "").trim();
      if (!/^\d+$/.test(text)) return true;
      const n = Number(text);
      return n < rule.min || n > rule.max;
    })
    .map(([path]) => path);
  const font = form.terminalFontFamily !== null && validateFontFamily(form.terminalFontFamily) !== null ? [FONT_FAMILY_FIELD] : [];
  const editorFont = form.editorFontFamily !== null && validateFontFamily(form.editorFontFamily) !== null ? [EDITOR_FONT_FAMILY_FIELD] : [];
  const lineHeight = parseLineHeight(form.terminalLineHeight) === null ? [LINE_HEIGHT_FIELD] : [];
  const text = [
    ...(parseContrast(form.minimumContrastRatio) === null ? [FIELD.MIN_CONTRAST] : []),
    ...(validWordSeparators(form.wordSeparators) ? [] : [FIELD.WORD_SEPARATORS]),
    ...(validDefaultUser(form.defaultUser.trim()) ? [] : [FIELD.DEFAULT_USER]),
  ];
  return [...numbers, ...font, ...editorFont, ...lineHeight, ...bandwidth, ...text, ...appearanceInvalidFields(form.look)];
}

export function settingsFromForm(base: Settings, form: SettingsFormState): Settings {
  const n = (path: string) => Number(form.numbers[path]);
  return applyAppearance(
    {
      ...base,
      locale: form.locale,
      general: {
        ...base.general,
        confirmCloseWithTerminals: form.confirmCloseWithTerminals,
        confirmQuit: form.confirmQuit,
        onStartup: form.onStartup,
        notifyOnDisconnect: form.notifyOnDisconnect,
        autoUpdate: form.autoUpdate,
      },
      terminal: {
        ...base.terminal,
        fontSize: n("terminal.fontSize"),
        scrollback: n("terminal.scrollback"),
        cursorBlink: form.terminalCursorBlink,
        copyOnSelect: form.terminalCopyOnSelect,
        fontFamily: form.terminalFontFamily === null ? null : form.terminalFontFamily.trim(),
        cursorStyle: form.terminalCursorStyle,
        lineHeight: parseLineHeight(form.terminalLineHeight) ?? base.terminal.lineHeight,
        bell: form.terminalBell,
        rightClick: form.terminalRightClick,
        letterSpacing: n("terminal.letterSpacing"),
        scrollSensitivity: n("terminal.scrollSensitivity"),
        minimumContrastRatio: parseContrast(form.minimumContrastRatio) ?? base.terminal.minimumContrastRatio,
        wordSeparators: form.wordSeparators,
        mouseReporting: form.mouseReporting,
        hideMousePointerWhileTyping: form.hideMousePointerWhileTyping,
        multilinePasteWarning: form.multilinePasteWarning,
        onExit: form.onExit,
        newTerminalCwd: form.newTerminalCwd,
        openTerminalOnConnect: form.openTerminalOnConnect,
        termType: form.termType,
        altIsMeta: form.altIsMeta,
        screenReaderMode: form.screenReaderMode,
      },
      editor: {
        ...base.editor,
        fontSize: n("editor.fontSize"),
        tabSize: n("editor.tabSize"),
        wordWrap: form.editorWordWrap,
        maxFileSizeMiB: n("editor.maxFileSizeMiB"),
        fontFamily: form.editorFontFamily === null ? null : form.editorFontFamily.trim(),
        lineNumbers: form.editorLineNumbers,
        highlightActiveLine: form.editorHighlightActiveLine,
        indentWithTabs: form.editorIndentWithTabs,
        trimTrailingWhitespace: form.editorTrimTrailingWhitespace,
        insertFinalNewline: form.editorInsertFinalNewline,
        detectIndentation: form.detectIndentation,
        newFileEol: form.newFileEol,
        autoCloseBrackets: form.autoCloseBrackets,
        bracketMatching: form.bracketMatching,
        showWhitespace: form.showWhitespace,
        codeFolding: form.codeFolding,
        fontLigatures: form.fontLigatures,
      },
      advanced: { ...base.advanced, logLevel: form.logLevel },
      connection: {
        ...base.connection,
        keepaliveIntervalSecs: n("connection.keepaliveIntervalSecs"),
        keepaliveCountMax: n("connection.keepaliveCountMax"),
        connectTimeoutSecs: n("connection.connectTimeoutSecs"),
        promptTimeoutSecs: n("connection.promptTimeoutSecs"),
        reconnect: { enabled: form.reconnectEnabled, maxAttempts: n("connection.reconnect.maxAttempts") },
        defaultUser: form.defaultUser.trim(),
        readSshConfig: form.readSshConfig,
        rememberPasswords: form.rememberPasswords,
      },
      files: {
        ...base.files,
        showHidden: form.showHidden,
        foldersFirst: form.foldersFirst,
        confirmDelete: form.confirmDelete,
        doubleClick: form.doubleClick,
        sizeUnits: form.sizeUnits,
        dateFormat: form.dateFormat,
        showPermissions: form.showPermissions,
        autoRefresh: Number(form.autoRefresh),
      },
      transfers: {
        ...base.transfers,
        maxConcurrent: n("transfers.maxConcurrent"),
        onConflict: form.onConflict,
        uploadLimitKiBps: kibpsFromField(form.uploadLimit) ?? 0,
        downloadLimitKiBps: kibpsFromField(form.downloadLimit) ?? 0,
        preserveTimestamps: form.preserveTimestamps,
        notifyOnComplete: form.notifyOnComplete,
      },
    },
    form.look,
  );
}
