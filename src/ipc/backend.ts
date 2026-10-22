// The Backend interface (docs/ARCHITECTURE.md §8.8): mirrors every command in §4.2 plus streams and events.
// Two implementations: TauriBackend (invoke / Channel / listen) and MockBackend (browser-only dev and tests).
import type {
  AgentStatus,
  AppBootstrap,
  Bookmark,
  BookmarkInput,
  BookmarkExportFormat,
  BookmarksDto,
  BookmarksExportResult,
  BookmarksImportPreview,
  BookmarksImportResult,
  ImportStrategy,
  DirListing,
  DropFilesPayload,
  DropHoverPayload,
  EmptyPayload,
  EnvInfo,
  MenuActionPayload,
  NotificationTexts,
  WindowState,
  FileEntry,
  Group,
  GroupInput,
  LocalKeyInfo,
  OpenSessionRequest,
  PromptAnswer,
  RecentEntry,
  RevealKind,
  SaveRequest,
  SaveResult,
  SaveTextOutcome,
  SessionEvent,
  Settings,
  SshConfigSnapshot,
  TermEvent,
  TermSize,
  TextDocument,
  TransferEvent,
  AppError,
  UpdateCheckOutcome,
  UpdateInfo,
  UpdateProgress,
} from "./types";

/**
 * Terminal channel message: raw bytes or a control event (§4.4/§4.5). This is the TS-side union of the Tauri
 * channel payload kinds, not a Rust DTO, so ts-rs does not generate it.
 */
export type TermMessage = ArrayBuffer | TermEvent;

/** Realm-safe check (an `ArrayBuffer` from another realm, e.g. in jsdom tests, fails `instanceof`). */
export function isRawBytes(message: TermMessage): message is ArrayBuffer {
  return message instanceof ArrayBuffer || Object.prototype.toString.call(message) === "[object ArrayBuffer]";
}

/** Global events (`app.emit` -> `listen`), §4.2. */
export interface BackendEvents {
  "ssh-config:changed": SshConfigSnapshot;
  "recents:changed": RecentEntry[];
  "app:quit-requested": EmptyPayload;
  "drop:hover": DropHoverPayload;
  "drop:leave": EmptyPayload;
  "drop:files": DropFilesPayload;
  /** macOS native menu: the id of the chosen item. */
  "menu:action": MenuActionPayload;
  /** Emitted when `maximized`, `fullscreen` or `focused` of the window changes. */
  "window:state": WindowState;
  /** Updater: an automatic check found an update (never a skipped version). */
  "update:available": UpdateInfo;
  /** Updater: the staged update could not be installed at the end of the quit flow; the app stays open. */
  "update:install-failed": AppError;
}
export type BackendEventName = keyof BackendEvents;
export type Unlisten = () => void;

export interface Backend {
  // ---- app ----
  appFrontendReady(): Promise<AppBootstrap>;
  appQuitAck(): Promise<void>;
  appQuitCancel(): Promise<void>;
  /** `lastDirs`: sessionId -> tree root. */
  appQuit(lastDirs: Record<string, string | null>): Promise<void>;
  appOpenLogsDir(): Promise<void>;
  appRevealPath(kind: RevealKind): Promise<void>;
  /** Resolves to true when the URL was opened after the native confirmation. */
  appOpenExternal(url: string, confirmTitle: string, confirmBody: string): Promise<boolean>;
  envInfo(): Promise<EnvInfo>;
  agentStatus(): Promise<AgentStatus>;
  keysList(): Promise<LocalKeyInfo[]>;
  /** One-line OpenSSH public key of a key listed by `keysList` (`path` must be exactly a listed path). */
  keysReadPublic(path: string): Promise<string>;
  dialogPickIdentityFile(title: string, filterName: string): Promise<string | null>;
  /** Native Save dialog + atomic UTF-8 write (no path ever comes from the page). `saved: false` = cancelled. */
  appSaveTextFile(suggestedName: string, contents: string, dialogTitle: string): Promise<SaveTextOutcome>;

  // ---- window (custom title bar): Windows/Linux draw their own controls ----
  windowMinimize(): Promise<void>;
  windowToggleMaximize(): Promise<void>;
  windowToggleFullscreen(): Promise<void>;
  /** The old close button: starts the quit flow and emits `app:quit-requested`. */
  windowClose(): Promise<void>;
  /** Starts a native window drag (`data-tauri-drag-region` is not available: no `core:window` permission). */
  windowStartDragging(): Promise<void>;
  /** Translated macOS menu labels (keys = `menu:action` ids and `menu.<name>`). Send the full set. */
  menuSetLabels(labels: Record<string, string>): Promise<void>;
  /** Localized OS notification texts (ARCHITECTURE §8.11); send the full set after startup and on language change. */
  notificationsSetTexts(texts: NotificationTexts): Promise<void>;

  // ---- hosts & storage ----
  sshConfigReload(): Promise<SshConfigSnapshot>;
  bookmarksGet(): Promise<BookmarksDto>;
  bookmarkSave(input: BookmarkInput): Promise<Bookmark>;
  bookmarkDelete(id: string): Promise<void>;
  bookmarkCopyFromSshConfig(alias: string): Promise<Bookmark>;
  /** `index` = final position in the target list (clamped); `groupId` null = ungrouped. */
  bookmarkMove(id: string, groupId: string | null, index: number): Promise<BookmarksDto>;
  bookmarkSetPassword(id: string, password: string | null): Promise<Bookmark>;
  groupSave(input: GroupInput): Promise<Group>;
  /** Never refuses; the group's bookmarks become ungrouped (appended). Reload with `bookmarksGet`. */
  groupDelete(id: string): Promise<void>;
  groupMove(id: string, index: number): Promise<BookmarksDto>;
  recentsGet(): Promise<RecentEntry[]>;
  /** Empties the recent connections (also emits `recents:changed`). Returns the (empty) list. */
  recentsClear(): Promise<RecentEntry[]>;
  /** Native Save dialog; the webview never supplies a path. Cancelled: `saved: false`. Never exports secrets. */
  bookmarksExport(format: BookmarkExportFormat, dialogTitle: string): Promise<BookmarksExportResult>;
  /**
   * Import step 1: native Open dialog (JSON), parse and validate; writes nothing. `picked: false` = cancelled.
   * The preview lists the file's valid startup commands; the parsed file waits under `token` (a new open replaces it).
   */
  bookmarksImportOpen(dialogTitle: string): Promise<BookmarksImportPreview>;
  /**
   * Import step 2: apply the pending import of `token` (consumed by every attempt; unknown → `invalidArgument{token}`).
   * Startup commands from the file are kept only with `keepStartupCommands` (explicit opt-in, default false).
   * Refresh the bookmark slice from `bookmarks` when set.
   */
  bookmarksImportApply(
    token: string,
    strategy: ImportStrategy,
    keepStartupCommands: boolean,
  ): Promise<BookmarksImportResult>;
  /** Drop the pending import of `token` (preview closed). Another or an unknown token is a no-op. */
  bookmarksImportCancel(token: string): Promise<void>;
  settingsGet(): Promise<Settings>;
  settingsUpdate(settings: Settings): Promise<Settings>;
  /** Writes the default settings and returns them. */
  settingsReset(): Promise<Settings>;

  // ---- sessions ----
  /** Resolves with the sessionId immediately; progress arrives through `onEvent`. */
  openSession(req: OpenSessionRequest, onEvent: (event: SessionEvent) => void): Promise<string>;
  sessionAnswerPrompt(sessionId: string, promptId: string, answer: PromptAnswer): Promise<void>;
  sessionReconnectNow(sessionId: string): Promise<void>;
  /** Drops the connection but keeps the session: state becomes `disconnected{userRequested, canRetry}`. */
  sessionDisconnect(sessionId: string): Promise<void>;
  sessionClose(sessionId: string, lastDir: string | null): Promise<void>;
  sessionProbeAll(): Promise<void>;

  // ---- terminals ----
  /** Output is raw bytes (ArrayBuffer) or a TermEvent. Resolves with the termId. */
  openTerminal(
    sessionId: string,
    size: TermSize,
    cwd: string | null,
    onMessage: (message: TermMessage) => void,
    /** `newTerminalCwd = "currentFolder"`: an unusable `cwd` falls back to the home folder instead of failing. */
    cwdFallback?: boolean,
  ): Promise<string>;
  /** Input bytes; sent as the raw IPC request body, so prefer a `Uint8Array` (a `number[]` is copied). */
  termWrite(termId: string, data: Uint8Array | readonly number[]): Promise<void>;
  termResize(termId: string, size: TermSize): Promise<void>;
  termAck(termId: string, bytes: number): Promise<void>;
  termRestart(termId: string): Promise<void>;
  termClose(termId: string): Promise<void>;

  // ---- sftp ----
  sftpRealpath(sessionId: string, path: string): Promise<string>;
  sftpList(sessionId: string, path: string): Promise<DirListing>;
  sftpStat(sessionId: string, path: string): Promise<FileEntry>;
  sftpReadText(sessionId: string, path: string, encoding: string | null): Promise<TextDocument>;
  sftpWriteText(sessionId: string, req: SaveRequest): Promise<SaveResult>;
  sftpCreateFile(sessionId: string, path: string): Promise<FileEntry>;
  sftpMkdir(sessionId: string, path: string): Promise<FileEntry>;
  sftpRename(sessionId: string, from: string, to: string): Promise<FileEntry>;
  sftpDelete(sessionId: string, path: string): Promise<void>;

  // ---- transfers ----
  transferDownload(
    sessionId: string,
    remotePath: string,
    dialogTitle: string,
    onEvent: (event: TransferEvent) => void,
  ): Promise<string | null>;
  transferUploadPick(
    sessionId: string,
    remoteDir: string,
    dialogTitle: string,
    overwrite: boolean,
    onEvent: (event: TransferEvent) => void,
  ): Promise<string[]>;
  transferUploadDropped(
    sessionId: string,
    dropId: string,
    remoteDir: string,
    overwrite: boolean,
    onEvent: (event: TransferEvent) => void,
  ): Promise<string[]>;
  transferRetry(transferId: string, overwrite: boolean, onEvent: (event: TransferEvent) => void): Promise<string>;
  transferCancel(transferId: string): Promise<void>;

  // ---- updates ----
  /** Manual "Check for updates…"; reports a skipped version too. */
  updateCheck(): Promise<UpdateCheckOutcome>;
  /**
   * "Update and restart": download + verify `version` (must be the update last reported), then the backend starts
   * the normal quit flow (`app:quit-requested`); the install happens at its end. Rejects with `cancelled` after
   * `updateCancel`.
   */
  updateInstall(version: string, onProgress: (event: UpdateProgress) => void): Promise<void>;
  /** Stops a running download and drops a staged update. */
  updateCancel(): Promise<void>;
  /** Automatic checks stop announcing exactly this version. */
  updateSkipVersion(version: string): Promise<void>;

  // ---- events ----
  listen<E extends BackendEventName>(event: E, callback: (payload: BackendEvents[E]) => void): Promise<Unlisten>;
}
