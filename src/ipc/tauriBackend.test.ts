import { beforeEach, describe, expect, it, vi } from "vitest";
import namesSource from "../../src-tauri/src/commands/names.rs?raw";

type InvokeArgs = Record<string, unknown> | Uint8Array;
type InvokeOptions = { headers?: Record<string, string> };
const invoke = vi.fn<(cmd: string, args?: InvokeArgs, options?: InvokeOptions) => Promise<unknown>>();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: InvokeArgs, options?: InvokeOptions) =>
    options === undefined ? invoke(cmd, args) : invoke(cmd, args, options),
  Channel: class {
    onmessage: unknown = null;
  },
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const { TauriBackend, TERM_ID_HEADER } = await import("./tauriBackend");

const noop = () => undefined;
const size = { cols: 80, rows: 24, pixelWidth: 0, pixelHeight: 0 };

/** `"raw"`: the payload is the raw request body (a `Uint8Array`), arguments travel in headers. */
type Row = [command: string, run: (b: InstanceType<typeof TauriBackend>) => Promise<unknown>, keys: string[] | "raw"];

// Expected JS argument keys per command (§4.2: camelCase of the Rust parameter names; Channel params included).
const ROWS: Row[] = [
  ["app_frontend_ready", (b) => b.appFrontendReady(), []],
  ["app_quit_ack", (b) => b.appQuitAck(), []],
  ["app_quit_cancel", (b) => b.appQuitCancel(), []],
  ["app_quit", (b) => b.appQuit({}), ["lastDirs"]],
  ["app_open_logs_dir", (b) => b.appOpenLogsDir(), []],
  ["app_reveal_path", (b) => b.appRevealPath("sshConfig"), ["kind"]],
  ["app_open_external", (b) => b.appOpenExternal("https://x", "t", "b"), ["confirmBody", "confirmTitle", "url"]],
  ["env_info", (b) => b.envInfo(), []],
  ["agent_status", (b) => b.agentStatus(), []],
  ["keys_list", (b) => b.keysList(), []],
  ["keys_read_public", (b) => b.keysReadPublic("p"), ["path"]],
  ["app_save_text_file", (b) => b.appSaveTextFile("n", "c", "t"), ["contents", "dialogTitle", "suggestedName"]],
  ["app_error_report", (b) => b.appErrorReport({ includeLog: false }), ["input"]],
  ["window_minimize", (b) => b.windowMinimize(), []],
  ["window_toggle_maximize", (b) => b.windowToggleMaximize(), []],
  ["window_toggle_fullscreen", (b) => b.windowToggleFullscreen(), []],
  ["window_close", (b) => b.windowClose(), []],
  ["window_start_dragging", (b) => b.windowStartDragging(), []],
  ["menu_set_labels", (b) => b.menuSetLabels({}), ["labels"]],
  ["notifications_set_texts", (b) => b.notificationsSetTexts({} as never), ["texts"]],
  ["dialog_pick_identity_file", (b) => b.dialogPickIdentityFile("t", "f"), ["filterName", "title"]],
  ["ssh_config_reload", (b) => b.sshConfigReload(), []],
  ["bookmarks_get", (b) => b.bookmarksGet(), []],
  ["bookmark_save", (b) => b.bookmarkSave({} as never), ["input"]],
  ["bookmark_delete", (b) => b.bookmarkDelete("i"), ["id"]],
  ["bookmark_copy_from_ssh_config", (b) => b.bookmarkCopyFromSshConfig("a"), ["alias"]],
  ["bookmark_set_password", (b) => b.bookmarkSetPassword("i", null), ["id", "password"]],
  ["bookmark_move", (b) => b.bookmarkMove("i", null, 0), ["groupId", "id", "index"]],
  ["group_move", (b) => b.groupMove("i", 0), ["id", "index"]],
  ["group_save", (b) => b.groupSave({} as never), ["input"]],
  ["group_delete", (b) => b.groupDelete("i"), ["id"]],
  ["recents_get", (b) => b.recentsGet(), []],
  ["recents_clear", (b) => b.recentsClear(), []],
  ["bookmarks_export", (b) => b.bookmarksExport("json", "t"), ["dialogTitle", "format"]],
  ["bookmarks_import_open", (b) => b.bookmarksImportOpen("t"), ["dialogTitle"]],
  [
    "bookmarks_import_apply",
    (b) => b.bookmarksImportApply("k", "skip", false),
    ["keepStartupCommands", "strategy", "token"],
  ],
  ["bookmarks_import_cancel", (b) => b.bookmarksImportCancel("k"), ["token"]],
  ["settings_get", (b) => b.settingsGet(), []],
  ["settings_update", (b) => b.settingsUpdate({} as never), ["settings"]],
  ["settings_reset", (b) => b.settingsReset(), []],
  ["session_open", (b) => b.openSession({} as never, noop), ["events", "req"]],
  ["session_answer_prompt", (b) => b.sessionAnswerPrompt("s", "p", {} as never), ["answer", "promptId", "sessionId"]],
  ["session_reconnect_now", (b) => b.sessionReconnectNow("s"), ["sessionId"]],
  ["session_disconnect", (b) => b.sessionDisconnect("s"), ["sessionId"]],
  ["session_close", (b) => b.sessionClose("s", null), ["lastDir", "sessionId"]],
  ["session_probe_all", (b) => b.sessionProbeAll(), []],
  ["term_open", (b) => b.openTerminal("s", size, null, noop), ["cwd", "cwdFallback", "output", "sessionId", "size"]],
  ["term_write", (b) => b.termWrite("t", [1]), "raw"],
  ["term_resize", (b) => b.termResize("t", size), ["size", "termId"]],
  ["term_ack", (b) => b.termAck("t", 1), ["bytes", "termId"]],
  ["term_restart", (b) => b.termRestart("t"), ["termId"]],
  ["term_close", (b) => b.termClose("t"), ["termId"]],
  ["sftp_realpath", (b) => b.sftpRealpath("s", "/"), ["path", "sessionId"]],
  ["sftp_list", (b) => b.sftpList("s", "/"), ["path", "sessionId"]],
  ["sftp_stat", (b) => b.sftpStat("s", "/"), ["path", "sessionId"]],
  ["sftp_read_text", (b) => b.sftpReadText("s", "/", null), ["encoding", "path", "sessionId"]],
  ["sftp_write_text", (b) => b.sftpWriteText("s", {} as never), ["req", "sessionId"]],
  ["sftp_create_file", (b) => b.sftpCreateFile("s", "/"), ["path", "sessionId"]],
  ["sftp_mkdir", (b) => b.sftpMkdir("s", "/"), ["path", "sessionId"]],
  ["sftp_rename", (b) => b.sftpRename("s", "/a", "/b"), ["from", "sessionId", "to"]],
  ["sftp_delete", (b) => b.sftpDelete("s", "/"), ["path", "sessionId"]],
  ["transfer_download", (b) => b.transferDownload("s", "/", "t", noop), ["dialogTitle", "progress", "remotePath", "sessionId"]],
  [
    "transfer_upload_pick",
    (b) => b.transferUploadPick("s", "/", "t", false, noop),
    ["dialogTitle", "overwrite", "progress", "remoteDir", "sessionId"],
  ],
  [
    "transfer_upload_dropped",
    (b) => b.transferUploadDropped("s", "d", "/", false, noop),
    ["dropId", "overwrite", "progress", "remoteDir", "sessionId"],
  ],
  [
    "transfer_download_folder",
    (b) => b.transferDownloadFolder("s", "/", "t", noop),
    ["dialogTitle", "progress", "remotePath", "sessionId"],
  ],
  ["transfer_decide", (b) => b.transferDecide("t", "merge"), ["decision", "transferId"]],
  ["transfer_retry", (b) => b.transferRetry("t", false, noop), ["overwrite", "progress", "transferId"]],
  ["transfer_cancel", (b) => b.transferCancel("t"), ["transferId"]],
  ["update_check", (b) => b.updateCheck(), []],
  ["update_install", (b) => b.updateInstall("1.2.3", noop), ["progress", "version"]],
  ["update_cancel", (b) => b.updateCancel(), []],
  ["update_skip_version", (b) => b.updateSkipVersion("1.2.3"), ["version"]],
];

describe("TauriBackend command mapping", () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue(undefined);
  });

  const rustCommands = [...namesSource.matchAll(/^\s*"([a-z_]+)",$/gm)].map((m) => m[1] as string);

  it("covers exactly the commands listed in src-tauri/src/commands/names.rs", () => {
    expect(ROWS.map(([cmd]) => cmd).sort()).toEqual([...rustCommands].sort());
    expect(rustCommands.length).toBe(72);
  });

  for (const [command, run, keys] of ROWS) {
    it(`${command}: command name and argument keys`, async () => {
      await run(new TauriBackend());
      expect(invoke).toHaveBeenCalledTimes(1);
      const [cmd, args] = invoke.mock.calls[0] ?? [];
      expect(cmd).toBe(command);
      if (keys === "raw") expect(args).toBeInstanceOf(Uint8Array);
      else expect(Object.keys(args ?? {}).sort()).toEqual(keys);
    });
  }

  describe("term_write raw body", () => {
    it("sends a Uint8Array as the body as is, with the terminal id in the header", async () => {
      const data = new TextEncoder().encode("ls -la\r");
      await new TauriBackend().termWrite("term-1", data);
      const [cmd, body, options] = invoke.mock.calls[0] ?? [];
      expect(cmd).toBe("term_write");
      expect(body).toBe(data);
      expect(options).toEqual({ headers: { [TERM_ID_HEADER]: "term-1" } });
    });

    it("copies a number[] into a Uint8Array", async () => {
      await new TauriBackend().termWrite("t", [0, 104, 255]);
      const [, body, options] = invoke.mock.calls[0] ?? [];
      expect(body).toBeInstanceOf(Uint8Array);
      expect(Array.from(body as Uint8Array)).toEqual([0, 104, 255]);
      expect(options).toEqual({ headers: { "shellfin-term-id": "t" } });
    });

    it("keeps a subarray view to its own bytes", async () => {
      const whole = Uint8Array.from([1, 2, 3, 4, 5]);
      await new TauriBackend().termWrite("t", whole.subarray(1, 3));
      expect(Array.from(invoke.mock.calls[0]?.[1] as Uint8Array)).toEqual([2, 3]);
    });

    it("re-throws a rejection as IpcError", async () => {
      invoke.mockRejectedValue({ code: "terminalNotFound", params: { termId: "t" } });
      await expect(new TauriBackend().termWrite("t", [1])).rejects.toMatchObject({
        name: "IpcError",
        error: { code: "terminalNotFound" },
      });
    });
  });

  it("re-throws rejections as IpcError carrying the AppError", async () => {
    invoke.mockRejectedValue({ code: "notConnected", params: { sessionId: "s" } });
    await expect(new TauriBackend().termClose("t")).rejects.toMatchObject({
      name: "IpcError",
      error: { code: "notConnected" },
    });
  });
});
