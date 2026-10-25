// TauriBackend: the real implementation (invoke / Channel / listen). Selected by ipc/index.ts when running
// inside Tauri. JS passes camelCase argument keys; the Rust command names are snake_case (§4.1).
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Backend, BackendEventName, BackendEvents, TermMessage, Unlisten } from "./backend";
import { toIpcError } from "./call";
import type {
  AgentStatus,
  AppBootstrap,
  Bookmark,
  BookmarksDto,
  BookmarksExportResult,
  BookmarksImportPreview,
  BookmarksImportResult,
  DirListing,
  EnvInfo,
  ErrorReport,
  FileEntry,
  Group,
  LocalKeyInfo,
  RecentEntry,
  SaveResult,
  SaveTextOutcome,
  SessionEvent,
  Settings,
  SshConfigSnapshot,
  TextDocument,
  TransferEvent,
  UpdateCheckOutcome,
  UpdateProgress,
} from "./types";

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    throw toIpcError(e);
  }
}

/** Header of a raw-body `term_write` (must match `TERM_ID_HEADER` in src-tauri/src/commands/terminal.rs). */
export const TERM_ID_HEADER = "shellfin-term-id";

/** A command whose payload is the raw request body (`InvokeBody::Raw` in Rust); arguments go in headers. */
async function callRaw<T>(cmd: string, body: Uint8Array, headers: Record<string, string>): Promise<T> {
  try {
    return await invoke<T>(cmd, body, { headers });
  } catch (e) {
    throw toIpcError(e);
  }
}

function channel<T>(onMessage: (m: T) => void): Channel<T> {
  const ch = new Channel<T>();
  ch.onmessage = onMessage;
  return ch;
}

export class TauriBackend implements Backend {
  // ---- app ----
  appFrontendReady = () => call<AppBootstrap>("app_frontend_ready");
  appQuitAck = () => call<void>("app_quit_ack");
  appQuitCancel = () => call<void>("app_quit_cancel");
  appQuit: Backend["appQuit"] = (lastDirs) => call<void>("app_quit", { lastDirs });
  appOpenLogsDir = () => call<void>("app_open_logs_dir");
  appRevealPath: Backend["appRevealPath"] = (kind) => call<void>("app_reveal_path", { kind });
  appOpenExternal: Backend["appOpenExternal"] = (url, confirmTitle, confirmBody) =>
    call<boolean>("app_open_external", { url, confirmTitle, confirmBody });
  envInfo = () => call<EnvInfo>("env_info");
  agentStatus = () => call<AgentStatus>("agent_status");
  keysList = () => call<LocalKeyInfo[]>("keys_list");
  keysReadPublic: Backend["keysReadPublic"] = (path) => call<string>("keys_read_public", { path });
  dialogPickIdentityFile: Backend["dialogPickIdentityFile"] = (title, filterName) =>
    call<string | null>("dialog_pick_identity_file", { title, filterName });
  appSaveTextFile: Backend["appSaveTextFile"] = (suggestedName, contents, dialogTitle) =>
    call<SaveTextOutcome>("app_save_text_file", { suggestedName, contents, dialogTitle });
  appErrorReport: Backend["appErrorReport"] = (input) => call<ErrorReport>("app_error_report", { input });

  // ---- window ----
  windowMinimize = () => call<void>("window_minimize");
  windowToggleMaximize = () => call<void>("window_toggle_maximize");
  windowToggleFullscreen = () => call<void>("window_toggle_fullscreen");
  windowClose = () => call<void>("window_close");
  windowStartDragging = () => call<void>("window_start_dragging");
  menuSetLabels: Backend["menuSetLabels"] = (labels) => call<void>("menu_set_labels", { labels });
  notificationsSetTexts: Backend["notificationsSetTexts"] = (texts) => call<void>("notifications_set_texts", { texts });

  // ---- hosts & storage ----
  sshConfigReload = () => call<SshConfigSnapshot>("ssh_config_reload");
  bookmarksGet = () => call<BookmarksDto>("bookmarks_get");
  bookmarkSave: Backend["bookmarkSave"] = (input) => call<Bookmark>("bookmark_save", { input });
  bookmarkDelete: Backend["bookmarkDelete"] = (id) => call<void>("bookmark_delete", { id });
  bookmarkCopyFromSshConfig: Backend["bookmarkCopyFromSshConfig"] = (alias) =>
    call<Bookmark>("bookmark_copy_from_ssh_config", { alias });
  bookmarkSetPassword: Backend["bookmarkSetPassword"] = (id, password) =>
    call<Bookmark>("bookmark_set_password", { id, password });
  bookmarkMove: Backend["bookmarkMove"] = (id, groupId, index) => call<BookmarksDto>("bookmark_move", { id, groupId, index });
  groupMove: Backend["groupMove"] = (id, index) => call<BookmarksDto>("group_move", { id, index });
  groupSave: Backend["groupSave"] = (input) => call<Group>("group_save", { input });
  groupDelete: Backend["groupDelete"] = (id) => call<void>("group_delete", { id });
  recentsGet = () => call<RecentEntry[]>("recents_get");
  recentsClear = () => call<RecentEntry[]>("recents_clear");
  bookmarksExport: Backend["bookmarksExport"] = (format, dialogTitle) =>
    call<BookmarksExportResult>("bookmarks_export", { format, dialogTitle });
  bookmarksImportOpen: Backend["bookmarksImportOpen"] = (dialogTitle) =>
    call<BookmarksImportPreview>("bookmarks_import_open", { dialogTitle });
  bookmarksImportApply: Backend["bookmarksImportApply"] = (token, strategy, keepStartupCommands) =>
    call<BookmarksImportResult>("bookmarks_import_apply", { token, strategy, keepStartupCommands });
  bookmarksImportCancel: Backend["bookmarksImportCancel"] = (token) => call<void>("bookmarks_import_cancel", { token });
  settingsGet = () => call<Settings>("settings_get");
  settingsUpdate: Backend["settingsUpdate"] = (settings) => call<Settings>("settings_update", { settings });
  settingsReset = () => call<Settings>("settings_reset");

  // ---- sessions ----
  openSession: Backend["openSession"] = (req, onEvent) =>
    call<string>("session_open", { req, events: channel<SessionEvent>(onEvent) });
  sessionAnswerPrompt: Backend["sessionAnswerPrompt"] = (sessionId, promptId, answer) =>
    call<void>("session_answer_prompt", { sessionId, promptId, answer });
  sessionReconnectNow: Backend["sessionReconnectNow"] = (sessionId) =>
    call<void>("session_reconnect_now", { sessionId });
  sessionDisconnect: Backend["sessionDisconnect"] = (sessionId) => call<void>("session_disconnect", { sessionId });
  sessionClose: Backend["sessionClose"] = (sessionId, lastDir) => call<void>("session_close", { sessionId, lastDir });
  sessionProbeAll = () => call<void>("session_probe_all");

  // ---- terminals ----
  openTerminal: Backend["openTerminal"] = (sessionId, size, cwd, onMessage, cwdFallback = false) =>
    call<string>("term_open", { sessionId, size, cwd, output: channel<TermMessage>(onMessage), cwdFallback });
  // The bytes are the raw request body (no JSON number[]); the terminal id travels in a header.
  termWrite: Backend["termWrite"] = (termId, data) =>
    // isView, not instanceof: a Uint8Array from another realm (e.g. TextEncoder in tests) is not copied either.
    callRaw<void>("term_write", ArrayBuffer.isView(data) ? data : Uint8Array.from(data), {
      [TERM_ID_HEADER]: termId,
    });
  termResize: Backend["termResize"] = (termId, size) => call<void>("term_resize", { termId, size });
  termAck: Backend["termAck"] = (termId, bytes) => call<void>("term_ack", { termId, bytes });
  termRestart: Backend["termRestart"] = (termId) => call<void>("term_restart", { termId });
  termClose: Backend["termClose"] = (termId) => call<void>("term_close", { termId });

  // ---- sftp ----
  sftpRealpath: Backend["sftpRealpath"] = (sessionId, path) => call<string>("sftp_realpath", { sessionId, path });
  sftpList: Backend["sftpList"] = (sessionId, path) => call<DirListing>("sftp_list", { sessionId, path });
  sftpStat: Backend["sftpStat"] = (sessionId, path) => call<FileEntry>("sftp_stat", { sessionId, path });
  sftpReadText: Backend["sftpReadText"] = (sessionId, path, encoding) =>
    call<TextDocument>("sftp_read_text", { sessionId, path, encoding });
  sftpWriteText: Backend["sftpWriteText"] = (sessionId, req) => call<SaveResult>("sftp_write_text", { sessionId, req });
  sftpCreateFile: Backend["sftpCreateFile"] = (sessionId, path) =>
    call<FileEntry>("sftp_create_file", { sessionId, path });
  sftpMkdir: Backend["sftpMkdir"] = (sessionId, path) => call<FileEntry>("sftp_mkdir", { sessionId, path });
  sftpRename: Backend["sftpRename"] = (sessionId, from, to) => call<FileEntry>("sftp_rename", { sessionId, from, to });
  sftpDelete: Backend["sftpDelete"] = (sessionId, path) => call<void>("sftp_delete", { sessionId, path });

  // ---- transfers ----
  transferDownload: Backend["transferDownload"] = (sessionId, remotePath, dialogTitle, onEvent) =>
    call<string | null>("transfer_download", {
      sessionId,
      remotePath,
      dialogTitle,
      progress: channel<TransferEvent>(onEvent),
    });
  transferUploadPick: Backend["transferUploadPick"] = (sessionId, remoteDir, dialogTitle, overwrite, onEvent) =>
    call<string[]>("transfer_upload_pick", {
      sessionId,
      remoteDir,
      dialogTitle,
      overwrite,
      progress: channel<TransferEvent>(onEvent),
    });
  transferUploadDropped: Backend["transferUploadDropped"] = (sessionId, dropId, remoteDir, overwrite, onEvent) =>
    call<string[]>("transfer_upload_dropped", {
      sessionId,
      dropId,
      remoteDir,
      overwrite,
      progress: channel<TransferEvent>(onEvent),
    });
  transferRetry: Backend["transferRetry"] = (transferId, overwrite, onEvent) =>
    call<string>("transfer_retry", { transferId, overwrite, progress: channel<TransferEvent>(onEvent) });
  transferCancel: Backend["transferCancel"] = (transferId) => call<void>("transfer_cancel", { transferId });

  // ---- updates ----
  updateCheck = () => call<UpdateCheckOutcome>("update_check");
  updateInstall: Backend["updateInstall"] = (version, onProgress) =>
    call<void>("update_install", { version, progress: channel<UpdateProgress>(onProgress) });
  updateCancel = () => call<void>("update_cancel");
  updateSkipVersion: Backend["updateSkipVersion"] = (version) => call<void>("update_skip_version", { version });

  // ---- events ----
  async listen<E extends BackendEventName>(event: E, callback: (payload: BackendEvents[E]) => void): Promise<Unlisten> {
    return listen<BackendEvents[E]>(event, (e) => callback(e.payload));
  }
}
