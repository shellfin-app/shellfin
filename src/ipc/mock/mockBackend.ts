// MockBackend: browser-only dev and tests (docs/ARCHITECTURE.md §8.8). Everything is in memory; behaviour is
// driven by a scenario (`?scenario=`), see ./scenarios.ts.
import type { Backend, BackendEventName, BackendEvents, TermMessage, Unlisten } from "../backend";
import { IpcError } from "../call";
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
  ImportStartupCommand,
  ImportStrategy,
  EnvInfo,
  Group,
  GroupInput,
  AppError,
  DropEntry,
  HopInfo,
  LocalKeyInfo,
  OpenSessionRequest,
  PromptAnswer,
  RecentEntry,
  RecentTarget,
  SaveRequest,
  SaveResult,
  SaveTextOutcome,
  SessionEvent,
  SessionInfo,
  SessionState,
  Settings,
  SshConfigSnapshot,
  TermSize,
  TextDocument,
  TransferDirection,
  TransferEvent,
  UpdateCheckOutcome,
  UpdateInfo,
  UpdateProgress,
  WindowState,
} from "../types";
import { validStartupCommand } from "../../lib/bookmarkForm";
import { dirname, joinPath } from "../../lib/remotePath";
import { FakeFs } from "./fakeFs";
import { FakeShell } from "./fakeShell";
import {
  makeAgent,
  makeBookmarks,
  makeBootstrap,
  makeEnv,
  makeMockUpdate,
  makeRecents,
  makeSettings,
  makeSshConfig,
} from "./fixtures";
import { TerminalOutput, floodChunks } from "./termFlow";
import { type MockImportFile, type ScenarioHooks, type ScenarioName, type SessionScript, resolveScenario } from "./scenarios";

export interface MockBackendOptions {
  scenario?: ScenarioName;
  /** Artificial latency per command in ms (0 in tests). */
  latencyMs?: number;
  /** Prompt timeout (the real one is `promptTimeoutSecs`, 110 s): `promptClosed{timeout}` + `promptTimeout`. */
  promptTimeoutMs?: number;
  /** Delay before `dropAfter10s` / `reconnectHostKeyMismatch` lose the connection. */
  dropAfterMs?: number;
  /** Size of the `terminalFlood` burst and of the shell's `flood` command default (bytes). */
  floodBytes?: number;
  /** Pause between the progress steps of an upload/download in ms (0 = as fast as the event loop allows). */
  transferStepMs?: number;
  /** Clock override for deterministic tests. */
  now?: () => number;
}

interface HostSpec {
  label: string;
  user: string;
  host: string;
  port: number;
}

interface MockSession {
  id: string;
  closed: boolean;
  host: HostSpec;
  /** Which store entry a remembered password belongs to (`bm:<id>` / `ssh:<alias>`), null for drafts and quick specs. */
  keychainKey: string | null;
  onEvent: (event: SessionEvent) => void;
  hop: HopInfo;
  info: SessionInfo;
  probeOnly: boolean;
  /** The prompt the script is waiting on. */
  pending: { id: string; resolve: (answer: PromptAnswer | null) => void } | null;
  /** Ends the current interruptible wait ("Reconnect now"). */
  waitRelease: (() => void) | null;
  lastState: SessionState | null;
  /** Bookmark the session was opened from (live edits). */
  bookmarkId: string | null;
  /** What the connection was made with; `needsReconnect` compares the saved bookmark against it. */
  connectedWith: string;
  /** `session_disconnect` was called: the next "reconnect now" restores the same session. */
  userDisconnected?: boolean;
  running: boolean;
  promptSeq: number;
  /** The single re-dial after a connection loss during authentication (CONTRACT #6) was used. */
  redialed: boolean;
  /** Answers given so far that the backend replays on the re-dial (keyboard-interactive answers are never cached). */
  cache: Partial<Record<"hostKey" | "passphrase" | "password", PromptAnswer>>;
}

/** How long a dropped-files token stays valid (§6.6). */
const DROP_TTL_MS = 60_000;

/** Largest `term_write` payload per call (`terminal::MAX_WRITE` in Rust). */
const MAX_TERM_WRITE = 1024 * 1024;

/** What counts as a different connection (ARCHITECTURE §7.6): host, port, user, useSshConfig, proxyJump, auth. */
function connectionKey(b: {
  host: string;
  port: number | null;
  user: string | null;
  useSshConfig: boolean;
  proxyJump: unknown;
  auth: { method: string; identityFile?: string | null };
}): string {
  return JSON.stringify([b.host, b.port, b.user, b.useSshConfig, b.proxyJump, b.auth.method, b.auth.method === "key" ? b.auth.identityFile : null]);
}

/** Dense 0..n-1 order inside each group (and among groups), keeping the current relative order. */
function normalizeOrder(dto: BookmarksDto): BookmarksDto {
  const groups = [...dto.groups].sort((a, b) => a.order - b.order).map((g, i) => ({ ...g, order: i }));
  const counters = new Map<string | null, number>();
  const bookmarks = [...dto.bookmarks]
    .sort((a, b) => a.order - b.order)
    .map((b) => {
      const n = counters.get(b.groupId) ?? 0;
      counters.set(b.groupId, n + 1);
      return { ...b, order: n };
    });
  return { ...dto, groups, bookmarks };
}

const MOCK_FINGERPRINT = "SHA256:nThbg6kXUpJWGl7E1IGOCspRomTxdCARLviKw6E5SY8";

interface MockTerminal {
  id: string;
  sessionId: string;
  shell: FakeShell;
  /** Backlog / ack window / overflow, like the real reader task (§4.5). */
  out: TerminalOutput;
  closed: boolean;
}

interface MockTransfer {
  id: string;
  sessionId: string;
  name: string;
  direction: TransferDirection;
  size: number;
  remoteDir: string;
  remotePath: string | null;
  localIsDir?: boolean;
  onEvent: (event: TransferEvent) => void;
  cancelled: boolean;
  /** Failed but retryable: kept for `transfer_retry` (the backend keeps these for 10 minutes). */
  failed: boolean;
}


/**
 * Marker string: present in a bundle if and only if MockBackend is bundled. `scripts/assert-no-mock.mjs`
 * fails the release build when it finds it in `dist/`.
 */
export const MOCK_BACKEND_MARKER = "__shellfin_mock_backend__";

export class MockBackend implements Backend {
  readonly marker = MOCK_BACKEND_MARKER;
  readonly scenarioName: ScenarioName;
  private readonly hooks: ScenarioHooks;
  private readonly latencyMs: number;
  private readonly now: () => number;
  private readonly dropAfterMs: number;
  private readonly floodBytes: number;
  private readonly promptTimeoutMs: number;
  private readonly transferStepMs: number;
  private readonly conflicted = new Set<string>();
  private readonly transfers = new Map<string, MockTransfer>();
  private readonly drops = new Map<string, { entries: DropEntry[]; at: number }>();
  private readonly savedPasswords = new Set<string>();
  private seq = 0;
  private disposed = false;
  private readonly timers = new Set<ReturnType<typeof setTimeout>>();

  private settings: Settings = makeSettings();
  private sshConfig: SshConfigSnapshot;
  private bookmarks: BookmarksDto;
  private recents: RecentEntry[];
  private readonly fs: FakeFs;
  private readonly sessions = new Map<string, MockSession>();
  private readonly terminals = new Map<string, MockTerminal>();
  private readonly listeners = new Map<BackendEventName, Set<(payload: never) => void>>();

  constructor(options: MockBackendOptions = {}) {
    this.scenarioName = options.scenario ?? "happy";
    this.hooks = resolveScenario(this.scenarioName);
    this.latencyMs = options.latencyMs ?? 0;
    this.now = options.now ?? Date.now;
    this.dropAfterMs = options.dropAfterMs ?? 10_000;
    this.floodBytes = options.floodBytes ?? 24 * 1024 * 1024;
    this.promptTimeoutMs = options.promptTimeoutMs ?? 0;
    this.transferStepMs = options.transferStepMs ?? 0;
    const now = this.now();
    this.sshConfig = makeSshConfig(now);
    this.bookmarks = makeBookmarks(now);
    this.recents = makeRecents(now);
    this.fs = new FakeFs(now);
    this.hooks.extraFiles?.(this.fs);
    this.importFile = this.hooks.importFile ?? { bookmarks: [{ name: "imported-box", host: "imported.example.com", user: "root" }] };
    if (this.hooks.update) this.publishedUpdate = makeMockUpdate(this.hooks.update);
  }

  // ---- helpers ----
  private id(prefix: string): string {
    this.seq += 1;
    return `${prefix}-${this.seq.toString(16).padStart(4, "0")}`;
  }

  /** A cancellable timer: after `dispose()` the callback never runs. */
  private later(fn: () => void, ms: number): void {
    const handle = setTimeout(() => {
      this.timers.delete(handle);
      if (!this.disposed) fn();
    }, ms);
    this.timers.add(handle);
  }

  /** `sleep` that never resolves once the backend is disposed (the awaiting script just stops). */
  private readonly wait = (ms: number): Promise<void> =>
    new Promise<void>((resolve) => {
      if (this.disposed) return;
      this.later(resolve, ms);
    });

  /**
   * Tests/dev: stops every pending timer and script and silences all further events (session, terminal, transfer,
   * global). Called by `setBackend` when the backend is replaced, so nothing fires into a torn-down app.
   */
  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    for (const handle of this.timers) clearTimeout(handle);
    this.timers.clear();
    for (const s of this.sessions.values()) {
      s.closed = true;
      s.pending = null;
      s.waitRelease?.();
    }
    for (const t of this.transfers.values()) t.cancelled = true;
    for (const t of this.terminals.values()) t.closed = true;
    this.listeners.clear();
  }

  private async delay(): Promise<void> {
    if (this.latencyMs > 0) await this.wait(this.latencyMs);
  }

  private emitGlobal<E extends BackendEventName>(event: E, payload: BackendEvents[E]): void {
    if (this.disposed) return;
    for (const cb of this.listeners.get(event) ?? []) (cb as (p: BackendEvents[E]) => void)(payload);
  }

  /** Test/dev helper: simulates a backend-originated global event (e.g. `app:quit-requested`). */
  emit<E extends BackendEventName>(event: E, payload: BackendEvents[E]): void {
    this.emitGlobal(event, payload);
  }

  private hostFor(target: OpenSessionRequest["target"]): HostSpec {
    switch (target.kind) {
      case "sshConfig": {
        const h = this.sshConfig.hosts.find((x) => x.alias === target.alias);
        if (!h) throw new IpcError({ code: "notFound", params: { what: "sshConfigHost", id: target.alias } });
        return { label: h.alias, user: h.user ?? "deploy", host: h.hostname, port: h.port };
      }
      case "bookmark": {
        const b = this.bookmarks.bookmarks.find((x) => x.id === target.id);
        if (!b) throw new IpcError({ code: "notFound", params: { what: "bookmark", id: target.id } });
        return { label: b.name, user: b.user ?? "user", host: b.host, port: b.port ?? 22 };
      }
      case "quick": {
        const m = /^(?:([^@]+)@)?(.+?)(?::(\d+))?$/.exec(target.spec);
        if (!m?.[2]) throw new IpcError({ code: "invalidHostSpec", params: { spec: target.spec } });
        const user = m[1] ?? "user";
        return { label: m[2], user, host: m[2], port: m[3] ? Number(m[3]) : 22 };
      }
      case "draft": {
        const b = target.bookmark;
        return { label: b.name || b.host, user: b.user ?? "user", host: b.host, port: b.port ?? 22 };
      }
    }
  }

  // ---- app ----
  async appFrontendReady(): Promise<AppBootstrap> {
    await this.delay();
    // Update scenarios: the "automatic check" a moment after startup (the backend waits 15 s).
    if (this.hooks.update) this.later(() => this.announceUpdate(), 1500);
    return {
      ...makeBootstrap(this.now()),
      settings: this.settings,
      sshConfig: this.settings.connection.readSshConfig ? this.sshConfig : this.emptySshConfig(),
      bookmarks: this.bookmarks,
      recents: this.recents,
      // Like the backend: only offered with `onStartup = "restore"`, and only ids that still exist as bookmarks.
      restoreBookmarkIds:
        this.settings.general.onStartup === "restore"
          ? this.openAtQuit.filter((id) => this.bookmarks.bookmarks.some((b) => b.id === id))
          : [],
    };
  }
  /** Bookmark ids recorded "at the last quit" (tests / dev scenarios set them). */
  openAtQuit: string[] = [];
  private emptySshConfig(): SshConfigSnapshot {
    return { ...this.sshConfig, files: [], hosts: [], warnings: [] };
  }
  async appQuitAck(): Promise<void> {}
  async appQuitCancel(): Promise<void> {
    // Like the backend: cancelling the quit drops an update staged by "Update and restart".
    this.stagedUpdate = null;
  }
  async appQuit(_lastDirs: Record<string, string | null>): Promise<void> {
    this.quitCalls += 1;
    const staged = this.stagedUpdate;
    this.stagedUpdate = null;
    if (!staged) return;
    if (this.updateInstallFailure) {
      this.emitGlobal("update:install-failed", this.updateInstallFailure);
      return;
    }
    this.installedUpdate = staged;
  }
  /** Number of `app_quit` calls (tests). */
  quitCalls = 0;
  async appOpenLogsDir(): Promise<void> {}
  async appRevealPath(): Promise<void> {}
  async appOpenExternal(): Promise<boolean> {
    return false;
  }
  async envInfo(): Promise<EnvInfo> {
    await this.delay();
    return makeEnv(this.now());
  }
  async agentStatus(): Promise<AgentStatus> {
    await this.delay();
    if (this.hooks.agentUntrusted) {
      return { available: false, kind: null, endpoint: null, identities: [], warning: { code: "agentUntrusted" } };
    }
    return makeAgent();
  }
  async keysList(): Promise<LocalKeyInfo[]> {
    return [
      { path: "~/.ssh/id_ed25519", keyType: "ssh-ed25519", fingerprint: "SHA256:0f4Q1mW8xk3dQ0v2yq9tJt7Zl2x0Yb5cRr1uXn8PzAs", encrypted: true, inAgent: true },
      { path: "~/.ssh/work_rsa", keyType: "ssh-rsa", fingerprint: "SHA256:9aB3kLq7Vd2sPz1Hn6TgE4uYw0MxCj5rFo8iQe2NhKc", encrypted: false, inAgent: true },
    ];
  }
  async keysReadPublic(path: string): Promise<string> {
    const known = (await this.keysList()).find((k) => k.path === path);
    if (!known) throw new IpcError({ code: "invalidArgument", params: { field: "path" } });
    return `${known.keyType ?? "ssh-ed25519"} AAAAC3NzaC1lZDI1NTE5AAAAIMockPublicKey${path.length} mock@shellfin`;
  }
  async dialogPickIdentityFile(): Promise<string | null> {
    return null;
  }
  /** Files "saved" through the mock native dialog, in order (tests assert on them). */
  readonly savedTextFiles: { suggestedName: string; contents: string }[] = [];
  async appSaveTextFile(suggestedName: string, contents: string): Promise<SaveTextOutcome> {
    this.savedTextFiles.push({ suggestedName, contents });
    return { saved: true, fileName: suggestedName === "" ? "output.txt" : suggestedName };
  }

  // ---- window (the mock keeps a window state so the title bar can be exercised in the browser) ----
  private windowFlags: WindowState = { maximized: false, fullscreen: false, focused: true };
  /** Calls made on the window API, in order (tests assert on them). */
  readonly windowCalls: string[] = [];
  async windowMinimize(): Promise<void> {
    this.windowCalls.push("minimize");
  }
  async windowToggleMaximize(): Promise<void> {
    this.windowCalls.push("toggleMaximize");
    this.windowFlags = { ...this.windowFlags, maximized: !this.windowFlags.maximized };
    this.emitGlobal("window:state", this.windowFlags);
  }
  async windowToggleFullscreen(): Promise<void> {
    this.windowCalls.push("toggleFullscreen");
    this.windowFlags = { ...this.windowFlags, fullscreen: !this.windowFlags.fullscreen };
    this.emitGlobal("window:state", this.windowFlags);
  }
  async windowClose(): Promise<void> {
    this.windowCalls.push("close");
  }
  async windowStartDragging(): Promise<void> {
    this.windowCalls.push("startDragging");
  }
  /** The last label set sent with `menuSetLabels` (the mock validates nothing). */
  menuLabels: Record<string, string> = {};
  async menuSetLabels(labels: Record<string, string>): Promise<void> {
    this.menuLabels = { ...labels };
  }
  /** The last texts sent with `notificationsSetTexts` (the mock shows no notification). */
  notificationTexts: Parameters<Backend["notificationsSetTexts"]>[0] | null = null;
  async notificationsSetTexts(texts: Parameters<Backend["notificationsSetTexts"]>[0]): Promise<void> {
    this.notificationTexts = { ...texts };
  }

  // ---- hosts & storage ----
  async sshConfigReload(): Promise<SshConfigSnapshot> {
    await this.delay();
    this.sshConfig = { ...this.sshConfig, loadedAtMs: this.now() };
    return this.settings.connection.readSshConfig ? this.sshConfig : this.emptySshConfig();
  }
  async bookmarksGet(): Promise<BookmarksDto> {
    return this.bookmarks;
  }
  async bookmarkSave(input: BookmarkInput): Promise<Bookmark> {
    const now = this.now();
    const existing = input.id ? this.bookmarks.bookmarks.find((b) => b.id === input.id) : undefined;
    const id = existing?.id ?? this.id("bookmark");
    // A saved password is bound to host/port/user/jump (§9.2): editing any of them forgets it.
    if (
      existing &&
      (existing.host !== input.host ||
        existing.port !== input.port ||
        existing.user !== input.user ||
        JSON.stringify(existing.proxyJump) !== JSON.stringify(input.proxyJump) ||
        input.auth.method !== "password")
    ) {
      this.savedPasswords.delete(id);
    }
    const auth: Bookmark["auth"] =
      input.auth.method === "password" ? { method: "password", passwordSaved: this.savedPasswords.has(id) } : input.auth;
    const saved: Bookmark = {
      ...input,
      auth,
      id,
      uploadLimitKiBps: input.uploadLimitKiBps ?? null,
      downloadLimitKiBps: input.downloadLimitKiBps ?? null,
      color: input.color ?? null,
      confirmConnect: input.confirmConnect ?? false,
      startupCommand: input.startupCommand?.trim() ? input.startupCommand : null,
      notes: input.notes?.trim() ? input.notes : null,
      // A new bookmark, or one that changes group, goes to the end of its group.
      order:
        existing && existing.groupId === input.groupId
          ? existing.order
          : this.bookmarks.bookmarks.filter((b) => b.groupId === input.groupId && b.id !== id).length,
      createdAtMs: existing?.createdAtMs ?? now,
      updatedAtMs: now,
    };
    const others = this.bookmarks.bookmarks.filter((b) => b.id !== saved.id);
    this.bookmarks = normalizeOrder({ ...this.bookmarks, bookmarks: [...others, saved] });
    this.announceBookmarkUpdate(saved);
    return this.bookmarks.bookmarks.find((b) => b.id === saved.id) ?? saved;
  }
  /** Live edit (ARCHITECTURE §7.6): every session opened from this bookmark learns the new name, color and limits. */
  private announceBookmarkUpdate(saved: Bookmark): void {
    for (const session of this.sessions.values()) {
      if (session.bookmarkId !== saved.id || session.closed) continue;
      const limitOverrides = { uploadKibps: saved.uploadLimitKiBps, downloadKibps: saved.downloadLimitKiBps };
      session.info = { ...session.info, limitOverrides, color: saved.color };
      session.onEvent({
        type: "bookmarkUpdated",
        label: saved.name,
        color: saved.color,
        limitOverrides,
        needsReconnect: connectionKey(saved) !== session.connectedWith,
      });
    }
  }
  async bookmarkMove(id: string, groupId: string | null, index: number): Promise<BookmarksDto> {
    const moving = this.bookmarks.bookmarks.find((b) => b.id === id);
    if (!moving) throw new IpcError({ code: "notFound", params: { what: "bookmark", id } });
    if (groupId !== null && !this.bookmarks.groups.some((g) => g.id === groupId)) {
      throw new IpcError({ code: "notFound", params: { what: "group", id: groupId } });
    }
    const target = this.bookmarks.bookmarks.filter((b) => b.groupId === groupId && b.id !== id).sort((a, b) => a.order - b.order);
    target.splice(Math.min(index, target.length), 0, { ...moving, groupId, updatedAtMs: moving.groupId === groupId ? moving.updatedAtMs : this.now() });
    const rest = this.bookmarks.bookmarks.filter((b) => b.groupId !== groupId && b.id !== id);
    this.bookmarks = normalizeOrder({ ...this.bookmarks, bookmarks: [...rest, ...target.map((b, i) => ({ ...b, order: i }))] });
    return this.bookmarks;
  }
  async groupMove(id: string, index: number): Promise<BookmarksDto> {
    const moving = this.bookmarks.groups.find((g) => g.id === id);
    if (!moving) throw new IpcError({ code: "notFound", params: { what: "group", id } });
    const list = this.bookmarks.groups.filter((g) => g.id !== id).sort((a, b) => a.order - b.order);
    list.splice(Math.min(index, list.length), 0, moving);
    this.bookmarks = { ...this.bookmarks, groups: list.map((g, i) => ({ ...g, order: i })) };
    return this.bookmarks;
  }
  async bookmarkDelete(id: string): Promise<void> {
    this.bookmarks = { ...this.bookmarks, bookmarks: this.bookmarks.bookmarks.filter((b) => b.id !== id) };
  }
  async bookmarkCopyFromSshConfig(alias: string): Promise<Bookmark> {
    const h = this.sshConfig.hosts.find((x) => x.alias === alias);
    if (!h) throw new IpcError({ code: "notFound", params: { what: "sshConfigHost", id: alias } });
    return this.bookmarkSave({
      id: null,
      name: h.alias,
      host: h.hostname,
      port: h.port === 22 ? null : h.port,
      user: h.user,
      auth: { method: "auto" },
      proxyJump: null,
      groupId: null,
      tags: [],
      startDir: null,
      useSshConfig: true,
      color: null,
    });
  }
  async bookmarkSetPassword(id: string, password: string | null): Promise<Bookmark> {
    const b = this.bookmarks.bookmarks.find((x) => x.id === id);
    if (!b) throw new IpcError({ code: "notFound", params: { what: "bookmark", id } });
    // `connection.rememberPasswords = false`: forgetting still works, storing does not (ARCHITECTURE §4.2).
    if (password !== null && !this.settings.connection.rememberPasswords) throw new IpcError({ code: "secretStorageDisabled" });
    if (password === null) this.savedPasswords.delete(id);
    else this.savedPasswords.add(id);
    const updated: Bookmark =
      b.auth.method === "password" ? { ...b, auth: { method: "password", passwordSaved: password !== null } } : b;
    this.bookmarks = { ...this.bookmarks, bookmarks: this.bookmarks.bookmarks.map((x) => (x.id === id ? updated : x)) };
    return updated;
  }
  async groupSave(input: GroupInput): Promise<Group> {
    // GroupInput.order is ignored: new groups are appended, updates keep their position.
    const existing = this.bookmarks.groups.find((g) => g.id === input.id);
    const group: Group = { id: existing?.id ?? this.id("group"), name: input.name, order: existing?.order ?? this.bookmarks.groups.length };
    this.bookmarks = { ...this.bookmarks, groups: [...this.bookmarks.groups.filter((g) => g.id !== group.id), group] };
    return group;
  }
  async groupDelete(id: string): Promise<void> {
    if (!this.bookmarks.groups.some((g) => g.id === id)) throw new IpcError({ code: "notFound", params: { what: "group", id } });
    // The bookmarks become ungrouped, appended after the existing ungrouped ones.
    const size = this.bookmarks.bookmarks.filter((b) => b.groupId === null).length;
    const bookmarks = this.bookmarks.bookmarks.map((b) =>
      b.groupId === id ? { ...b, groupId: null, order: size + b.order, updatedAtMs: this.now() } : b,
    );
    this.bookmarks = normalizeOrder({ ...this.bookmarks, groups: this.bookmarks.groups.filter((g) => g.id !== id), bookmarks });
  }
  async recentsGet(): Promise<RecentEntry[]> {
    return this.recents;
  }
  async recentsClear(): Promise<RecentEntry[]> {
    this.recents = [];
    this.emitGlobal("recents:changed", this.recents);
    return this.recents;
  }

  /** Bookmark export calls (tests assert on them). The mock writes nothing. */
  readonly exportCalls: { format: BookmarkExportFormat; dialogTitle: string }[] = [];
  /** What the next export reports: `null` = the dialog is cancelled. */
  exportOutcome: Omit<BookmarksExportResult, "saved"> | null = { fileName: "shellfin-bookmarks.json", count: 0, skipped: [] };
  async bookmarksExport(format: BookmarkExportFormat, dialogTitle: string): Promise<BookmarksExportResult> {
    this.exportCalls.push({ format, dialogTitle });
    if (this.exportOutcome === null) return { saved: false, fileName: null, count: 0, skipped: [] };
    const count = this.exportOutcome.count || this.bookmarks.bookmarks.length;
    return { saved: true, ...this.exportOutcome, count };
  }

  /** Bookmark import calls (tests assert on them). */
  readonly importOpenCalls: string[] = [];
  readonly importApplyCalls: { token: string; strategy: ImportStrategy; keepStartupCommands: boolean }[] = [];
  readonly importCancelCalls: string[] = [];
  /** The "file" the next import open reads: `null` = cancelled; an `AppError` = whole-file error (nothing pending). */
  importFile: MockImportFile | AppError | null;
  /** The single pending import (like `AppState.pending_import`): replaced by every open, consumed by every apply. */
  private pendingImport: { token: string; file: MockImportFile } | null = null;
  async bookmarksImportOpen(dialogTitle: string): Promise<BookmarksImportPreview> {
    this.importOpenCalls.push(dialogTitle);
    this.pendingImport = null;
    if (this.bookmarks.readOnly) throw new IpcError({ code: "storageReadOnly" }); // before the dialog
    const file = this.importFile;
    if (file === null) return { picked: false, token: null, bookmarks: 0, conflicts: 0, invalid: [], startupCommands: [] };
    if ("code" in file) throw new IpcError(file);
    const token = this.id("import");
    this.pendingImport = { token, file };
    const exists = (name: string) => this.bookmarks.bookmarks.some((b) => b.name.toLowerCase() === name.toLowerCase());
    const startupCommands: ImportStartupCommand[] = [];
    for (const entry of file.bookmarks) {
      const command = entry.startupCommand;
      if (command !== undefined && command.trim() !== "" && validStartupCommand(command)) {
        startupCommands.push({ name: entry.name, command, exists: exists(entry.name) });
      }
    }
    return {
      picked: true,
      token,
      bookmarks: file.bookmarks.length,
      conflicts: file.bookmarks.filter((e) => exists(e.name)).length,
      invalid: file.invalid ?? [],
      startupCommands,
    };
  }
  async bookmarksImportCancel(token: string): Promise<void> {
    this.importCancelCalls.push(token);
    if (this.pendingImport?.token === token) this.pendingImport = null;
  }
  async bookmarksImportApply(token: string, strategy: ImportStrategy, keepStartupCommands: boolean): Promise<BookmarksImportResult> {
    this.importApplyCalls.push({ token, strategy, keepStartupCommands });
    const pending = this.pendingImport;
    this.pendingImport = null; // every attempt consumes the pending import
    if (!pending || pending.token !== token) throw new IpcError({ code: "invalidArgument", params: { field: "token" } });
    if (this.bookmarks.readOnly) throw new IpcError({ code: "storageReadOnly" });
    const file = pending.file;
    let added = 0;
    let replaced = 0;
    let skipped = 0;
    const stripped: string[] = [];
    // Lower-cased names this import added: like the backend, `replace` never overwrites one of them (plain skip).
    const addedNow: string[] = [];
    for (const entry of file.bookmarks) {
      const existing = this.bookmarks.bookmarks.find((b) => b.name.toLowerCase() === entry.name.toLowerCase());
      if (existing && (strategy === "skip" || (strategy === "replace" && addedNow.includes(existing.name.toLowerCase())))) {
        skipped += 1;
        continue;
      }
      const name = existing && strategy === "duplicate" ? `${entry.name} (2)` : entry.name;
      // A replaced bookmark keeps its own command only while it still points at the same server and account.
      const fileCommand = entry.startupCommand?.trim() ? entry.startupCommand : null;
      const kept = keepStartupCommands && fileCommand !== null && validStartupCommand(fileCommand) ? fileCommand : null;
      const keepsOwn =
        kept === null &&
        existing !== undefined &&
        strategy === "replace" &&
        existing.host === entry.host &&
        existing.port === null &&
        existing.user === (entry.user ?? null) &&
        !existing.useSshConfig &&
        existing.proxyJump === null;
      const droppedOwn = kept === null && existing !== undefined && strategy === "replace" && !keepsOwn && existing.startupCommand !== null;
      await this.bookmarkSave({
        id: existing && strategy === "replace" ? existing.id : null,
        name,
        host: entry.host,
        port: null,
        user: entry.user ?? null,
        auth: { method: "auto" },
        proxyJump: null,
        groupId: existing && strategy === "replace" ? existing.groupId : null,
        tags: [],
        startDir: null,
        useSshConfig: false,
        // Like the backend: a file's command is imported only with the opt-in (and only a valid one); otherwise a
        // replaced bookmark keeps its own (same target only).
        startupCommand: kept ?? (keepsOwn ? existing.startupCommand : null),
      });
      if (existing && strategy === "replace") replaced += 1;
      else {
        added += 1;
        addedNow.push(name.toLowerCase());
      }
      // The final name of an added / replaced bookmark whose file command was not imported (no opt-in, or invalid), or
      // whose own command was dropped because the file points it elsewhere.
      if ((fileCommand !== null && kept === null) || droppedOwn) stripped.push(name);
    }
    const changed = added + replaced > 0;
    return {
      picked: true,
      added,
      replaced,
      skipped,
      invalid: file.invalid ?? [],
      bookmarks: changed ? this.bookmarks : null,
      startupCommandsStripped: stripped,
    };
  }
  async settingsGet(): Promise<Settings> {
    return this.settings;
  }
  async settingsReset(): Promise<Settings> {
    this.settings = makeSettings();
    return this.settings;
  }
  async settingsUpdate(settings: Settings): Promise<Settings> {
    const readBefore = this.settings.connection.readSshConfig;
    this.settings = settings;
    // Like the backend: switching `readSshConfig` re-publishes the (empty or full) snapshot.
    if (readBefore !== settings.connection.readSshConfig) {
      this.emitGlobal("ssh-config:changed", settings.connection.readSshConfig ? this.sshConfig : this.emptySshConfig());
    }
    return settings;
  }

  // ---- sessions ----
  async openSession(req: OpenSessionRequest, onEvent: (event: SessionEvent) => void): Promise<string> {
    await this.delay();
    const host = this.hostFor(req.target);
    const t = req.target;
    const bm = t.kind === "bookmark" ? this.bookmarks.bookmarks.find((b) => b.id === t.id) : t.kind === "draft" ? t.bookmark : undefined;
    const id = this.id("session");
    const sftp = this.hooks.sftpAvailable !== false;
    const hop: HopInfo = {
      index: 0,
      label: host.label,
      user: host.user,
      host: host.host,
      port: host.port,
      hostKey: { keyType: "ssh-ed25519", fingerprint: MOCK_FINGERPRINT, verification: "knownHosts" },
    };
    const info: SessionInfo = {
      sessionId: id,
      label: host.label,
      hops: [hop],
      auth: { method: "publickey", keyType: "ed25519", keySource: "~/.ssh/id_ed25519" },
      sftpAvailable: sftp,
      homeDir: sftp ? this.fs.homeDir() : null,
      startDir: sftp ? "/srv/app" : null,
      remotePathStyle: "posix",
      remoteIdentity: { uid: 1000, gids: [1000, 27] },
      limitOverrides: { uploadKibps: bm?.uploadLimitKiBps ?? null, downloadKibps: bm?.downloadLimitKiBps ?? null },
      color: t.kind === "bookmark" ? (this.bookmarks.bookmarks.find((b) => b.id === t.id)?.color ?? null) : null,
    };
    const session: MockSession = {
      id,
      closed: false,
      host,
      keychainKey: this.keychainKeyFor(req.target),
      onEvent,
      hop,
      info,
      probeOnly: req.probeOnly,
      pending: null,
      waitRelease: null,
      lastState: null,
      bookmarkId: t.kind === "bookmark" ? t.id : null,
      connectedWith: bm ? connectionKey(bm) : "",
      running: false,
      promptSeq: 0,
      redialed: false,
      cache: {},
    };
    this.sessions.set(id, session);
    // Run the scenario script detached: session_open returns immediately (§4.2).
    this.runScript(session);
    return id;
  }

  private keychainKeyFor(target: OpenSessionRequest["target"]): string | null {
    if (target.kind === "sshConfig") return `ssh:${target.alias}`;
    if (target.kind === "bookmark") return `bm:${target.id}`;
    return null;
  }

  /** Test hook: the backend ends the open prompt on its own (timeout / lost connection). */
  dropPendingPrompt(sessionId: string, reason: "timeout" | "connectionLost"): void {
    const session = this.sessions.get(sessionId);
    const pending = session?.pending;
    if (!session || !pending) return;
    session.pending = null;
    session.onEvent({ type: "promptClosed", promptId: pending.id, reason });
    pending.resolve(null);
    if (reason === "connectionLost" && !session.redialed) {
      // The backend re-dials ONCE with the answers already given (connecting -> maybe a new prompt -> connected).
      session.redialed = true;
      this.later(() => this.runScript(session), 0);
      return;
    }
    const error: AppError =
      reason === "timeout"
        ? { code: "promptTimeout" }
        : { code: "disconnected", params: { reason: "connectionReset", message: null } };
    session.lastState = { phase: "disconnected", error, canRetry: true };
    session.onEvent({ type: "state", state: session.lastState });
  }

  /** Starts (or restarts after "retry") the scenario script of a session. */
  private runScript(session: MockSession): void {
    const run = this.hooks.runSession;
    if (!run || session.running || session.closed) return;
    session.running = true;
    const emit = (e: SessionEvent) => {
      if (session.closed) return;
      if (e.type === "state") session.lastState = e.state;
      session.onEvent(e);
      // probeOnly: connected => closed immediately (§5.8).
      if (session.probeOnly && e.type === "state" && e.state.phase === "connected") {
        session.closed = true;
        session.onEvent({ type: "state", state: { phase: "closed" } });
      }
    };
    const scaled = (ms: number) => this.wait(this.latencyMs > 0 ? ms : Math.min(ms, 20));
    const script: SessionScript = {
      sessionId: session.id,
      info: session.info,
      hop: session.hop,
      options: { dropAfterMs: this.dropAfterMs },
      emit,
      // Without artificial latency (tests) scripted pauses are shortened.
      sleep: scaled,
      sleepInterruptible: async (ms) => {
        await Promise.race([
          scaled(ms),
          new Promise<void>((resolve) => {
            session.waitRelease = resolve;
          }),
        ]);
        session.waitRelease = null;
      },
      isClosed: () => session.closed,
      newPromptId: () => {
        session.promptSeq += 1;
        return `prompt-${session.promptSeq}`;
      },
      ask: (prompt) => {
        const cacheKey = prompt.kind === "hostKeyUnknown" ? "hostKey" : prompt.kind === "passphrase" || prompt.kind === "password" ? prompt.kind : null;
        const cached = session.redialed && cacheKey ? session.cache[cacheKey] : undefined;
        if (cacheKey && cached) {
          delete session.cache[cacheKey]; // replayed once; a wrong answer re-prompts for real
          return Promise.resolve(cached);
        }
        return new Promise<PromptAnswer | null>((resolve) => {
          session.pending = { id: prompt.id, resolve };
          emit({ type: "prompt", prompt });
          if (this.promptTimeoutMs > 0) {
            this.later(() => {
              if (session.pending?.id === prompt.id) this.dropPendingPrompt(session.id, "timeout");
            }, this.promptTimeoutMs);
          }
        });
      },
      credentials: this.credentialsFor(session),
      rememberPassword: () => {
        if (!session.keychainKey) return;
        this.savedPasswords.add(session.keychainKey);
        const id = session.keychainKey.startsWith("bm:") ? session.keychainKey.slice(3) : null;
        if (id) {
          this.bookmarks = {
            ...this.bookmarks,
            bookmarks: this.bookmarks.bookmarks.map((b) =>
              b.id === id && b.auth.method === "password" ? { ...b, auth: { method: "password", passwordSaved: true } } : b,
            ),
          };
        }
      },
      connected: (verification) => {
        const hostKey = { ...session.hop.hostKey, verification: verification ?? session.hop.hostKey.verification };
        emit({
          type: "state",
          state: { phase: "connected", info: { ...session.info, hops: [{ ...session.hop, hostKey }] } },
        });
        emit({ type: "stats", txBytesPerSec: 1229, rxBytesPerSec: 18432 });
      },
      fail: (error, canRetry = false) => emit({ type: "state", state: { phase: "disconnected", error, canRetry } }),
      notifyReconnected: () => this.respawnShells(session.id),
    };
    void run(script).finally(() => {
      session.running = false;
    });
  }

  /** The backend respawns the shells of running terminals after a reconnect (§5.8): `reconnected`, then a new shell. */
  private respawnShells(sessionId: string): void {
    for (const t of this.terminals.values()) {
      if (t.sessionId !== sessionId || t.closed) continue;
      t.out.event({ type: "reconnected" });
      t.shell.restart();
    }
  }

  private credentialsFor(session: MockSession) {
    const canRemember = session.keychainKey !== null && this.settings.connection.rememberPasswords;
    return {
      canRemember,
      savedPasswordAvailable:
        canRemember && (this.savedPasswords.has(session.keychainKey ?? "") || this.hooks.keychainEntries === true),
    };
  }

  async sessionAnswerPrompt(sessionId: string, promptId: string, answer: PromptAnswer): Promise<void> {
    // Like the real backend: an unknown prompt OR session is `promptNotFound`.
    const session = this.sessions.get(sessionId);
    if (!session) throw new IpcError({ code: "promptNotFound" });
    const pending = session.pending;
    if (!pending || pending.id !== promptId) throw new IpcError({ code: "promptNotFound" });
    session.pending = null;
    if (answer.kind === "hostKey" || answer.kind === "passphrase" || answer.kind === "password") session.cache[answer.kind] = answer;
    pending.resolve(answer);
    session.onEvent({ type: "promptClosed", promptId, reason: "answered" });
  }

  async sessionReconnectNow(sessionId: string): Promise<void> {
    const session = this.sessions.get(sessionId);
    if (!session) throw new IpcError({ code: "sessionNotFound", params: { sessionId } });
    if (session.waitRelease) {
      session.waitRelease();
      return;
    }
    const state = session.lastState;
    if (session.userDisconnected) {
      session.userDisconnected = false;
      this.restoreConnection(sessionId);
      return;
    }
    if (state?.phase === "disconnected" && state.canRetry) {
      session.lastState = null;
      session.redialed = false;
      session.cache = {};
      this.runScript(session);
    }
  }

  async sessionDisconnect(sessionId: string): Promise<void> {
    const session = this.sessions.get(sessionId);
    if (!session) throw new IpcError({ code: "sessionNotFound", params: { sessionId } });
    if (session.lastState?.phase === "disconnected") return;
    for (const t of this.transfers.values()) {
      if (t.sessionId === sessionId) t.cancelled = true;
    }
    session.userDisconnected = true;
    session.lastState = {
      phase: "disconnected",
      error: { code: "disconnected", params: { reason: "userRequested", message: null } },
      canRetry: true,
    };
    session.onEvent({ type: "state", state: session.lastState });
  }

  async sessionClose(sessionId: string, lastDir: string | null): Promise<void> {
    const s = this.sessions.get(sessionId);
    if (!s) return;
    s.closed = true;
    if (s.pending) {
      s.onEvent({ type: "promptClosed", promptId: s.pending.id, reason: "cancelled" });
      s.pending.resolve(null);
      s.pending = null;
    }
    s.waitRelease?.();
    for (const t of this.transfers.values()) {
      if (t.sessionId === sessionId) t.cancelled = true;
    }
    s.onEvent({ type: "state", state: { phase: "closed" } });
    this.sessions.delete(sessionId);
    for (const t of this.terminals.values()) if (t.sessionId === sessionId) t.closed = true;
    if (s.probeOnly) return;
    const entry: RecentEntry = {
      target: { kind: "quick", spec: `${s.host.user}@${s.host.host}:${s.host.port}` } satisfies RecentTarget,
      label: s.host.label,
      userAtHost: `${s.host.user}@${s.host.host}`,
      via: null,
      lastConnectedAtMs: this.now(),
      lastDir,
      terminalCount: 0,
    };
    this.recents = [entry, ...this.recents.filter((r) => r.label !== entry.label)];
    this.emitGlobal("recents:changed", this.recents);
  }
  async sessionProbeAll(): Promise<void> {
    this.probeCount += 1;
  }

  // ---- terminals ----
  /** Every `term_open` (tests assert on `cwd`, `cwdFallback` and the TERM the PTY request would carry). */
  readonly terminalOpens: { sessionId: string; cwd: string | null; cwdFallback: boolean; termType: string }[] = [];
  async openTerminal(
    sessionId: string,
    size: TermSize,
    cwd: string | null,
    onMessage: (message: TermMessage) => void,
    cwdFallback = false,
  ): Promise<string> {
    await this.delay();
    const session = this.sessions.get(sessionId);
    if (!session) throw new IpcError({ code: "notConnected", params: { sessionId } });
    this.terminalOpens.push({ sessionId, cwd, cwdFallback, termType: this.settings.terminal.termType });
    const id = this.id("term");
    const term: MockTerminal = {
      id,
      sessionId,
      closed: false,
      // Hands out standalone ArrayBuffers, like a Tauri raw channel message.
      out: new TerminalOutput((message) => {
        if (!term.closed) onMessage(message);
      }),
      shell: new FakeShell(session.host.user, session.host.label, cwd ?? "/srv/app", {
        emit: (bytes) => term.out.push(bytes),
        exit: (code) => term.out.event({ type: "exit", code, signal: null }),
        flood: (bytes) => {
          for (const chunk of floodChunks(bytes)) term.out.push(chunk);
        },
      }),
    };
    term.shell.resize(size.cols, size.rows);
    this.terminals.set(id, term);
    // Prompt after the caller has stored the termId.
    this.later(() => {
      if (term.closed) return;
      term.shell.start();
      this.hooks.onTerminalOpened?.({
        termId: id,
        emitBytes: (b) => term.out.push(b),
        emitEvent: (e) => term.out.event(e),
        flood: (bytes = this.floodBytes) => {
          for (const chunk of floodChunks(bytes)) term.out.push(chunk);
        },
        sleep: this.wait,
        isClosed: () => term.closed,
      });
    }, 0);
    return id;
  }
  async termWrite(termId: string, data: Uint8Array | readonly number[]): Promise<void> {
    const t = this.terminals.get(termId);
    if (!t) throw new IpcError({ code: "terminalNotFound", params: { termId } });
    if (data.length > MAX_TERM_WRITE) throw new IpcError({ code: "invalidArgument", params: { field: "data" } });
    // Like the raw IPC body: the backend gets a copy, so later changes by the caller do not leak in.
    t.shell.write(Uint8Array.from(data));
  }
  async termResize(termId: string, size: TermSize): Promise<void> {
    const t = this.terminals.get(termId);
    if (!t) throw new IpcError({ code: "terminalNotFound", params: { termId } });
    t.shell.resize(size.cols, size.rows);
  }
  async termAck(termId: string, bytes: number): Promise<void> {
    this.terminals.get(termId)?.out.ack(bytes);
  }
  /** Like the backend: respawns the shell also while it is running (the old one's exit is not reported). */
  async termRestart(termId: string): Promise<void> {
    const t = this.terminals.get(termId);
    if (!t) throw new IpcError({ code: "terminalNotFound", params: { termId } });
    if (!this.sessions.has(t.sessionId)) throw new IpcError({ code: "notConnected", params: { sessionId: t.sessionId } });
    t.shell.restart();
  }
  async termClose(termId: string): Promise<void> {
    const t = this.terminals.get(termId);
    if (t) t.closed = true;
    this.terminals.delete(termId);
  }

  // ---- sftp ----
  /** Like the backend: a lost connection fails SFTP calls with `notConnected` (§5.8: in-flight requests fail). */
  private requireSftp(sessionId: string): MockSession {
    const session = this.sessions.get(sessionId);
    if (!session) throw new IpcError({ code: "sessionNotFound", params: { sessionId } });
    if (this.hooks.sftpAvailable === false) throw new IpcError({ code: "sftpUnavailable" });
    if (session.lastState?.phase !== "connected") throw new IpcError({ code: "notConnected", params: { sessionId } });
    return session;
  }
  async sftpRealpath(sessionId: string, path: string): Promise<string> {
    this.requireSftp(sessionId);
    return path === "." ? this.fs.homeDir() : path;
  }
  async sftpList(sessionId: string, path: string) {
    this.requireSftp(sessionId);
    await this.delay();
    return this.fs.list(path);
  }
  async sftpStat(sessionId: string, path: string) {
    this.requireSftp(sessionId);
    return this.fs.stat(path);
  }
  async sftpReadText(sessionId: string, path: string, encoding: string | null): Promise<TextDocument> {
    this.requireSftp(sessionId);
    await this.delay();
    const doc = this.fs.read(path, encoding, this.settings.editor.maxFileSizeMiB * 1024 * 1024);
    return {
      path,
      text: doc.text,
      encoding: doc.encoding,
      hasBom: doc.hasBom,
      eol: doc.eol,
      mixedEol: doc.mixedEol,
      permissions: doc.entry.permissions,
      etag: doc.etag,
      ...(doc.resolvedPath !== undefined ? { resolvedPath: doc.resolvedPath } : {}),
    };
  }
  async sftpWriteText(sessionId: string, req: SaveRequest): Promise<SaveResult> {
    this.requireSftp(sessionId);
    await this.delay();
    // saveConflict: somebody else edits the file right before our first (non-forced) save of it.
    if (this.hooks.conflictOnSave && !req.force && this.fs.has(req.path) && !this.conflicted.has(req.path)) {
      this.conflicted.add(req.path);
      this.fs.externalEdit(req.path, `${this.fs.read(req.path, null, Number.MAX_SAFE_INTEGER).text}# changed on the server\n`);
    }
    this.fs.write(req.path, req);
    return { etag: this.fs.etag(req.path), strategy: "atomic", warnings: [] };
  }
  async sftpCreateFile(sessionId: string, path: string) {
    this.requireSftp(sessionId);
    return this.fs.create(path, "file");
  }
  async sftpMkdir(sessionId: string, path: string) {
    this.requireSftp(sessionId);
    return this.fs.create(path, "dir");
  }
  async sftpRename(sessionId: string, from: string, to: string) {
    this.requireSftp(sessionId);
    return this.fs.rename(from, to);
  }
  async sftpDelete(sessionId: string, path: string): Promise<void> {
    this.requireSftp(sessionId);
    this.fs.remove(path);
  }

  // ---- transfers ----
  private pause(): Promise<void> {
    return this.wait(this.transferStepMs);
  }

  private startTransfer(
    session: MockSession,
    spec: { name: string; direction: TransferDirection; size: number; remoteDir: string; remotePath: string | null; localIsDir?: boolean },
    onEvent: (event: TransferEvent) => void,
    overwrite: boolean,
  ): string {
    const transfer: MockTransfer = { id: this.id("transfer"), sessionId: session.id, ...spec, onEvent, cancelled: false, failed: false };
    this.transfers.set(transfer.id, transfer);
    void this.runTransfer(transfer, overwrite);
    return transfer.id;
  }

  private async runTransfer(t: MockTransfer, overwrite: boolean): Promise<void> {
    const alive = () => this.sessions.has(t.sessionId);
    const emit = (event: TransferEvent) => {
      if (alive()) t.onEvent(event);
    };
    const fail = (error: AppError, retryable: boolean) => {
      t.failed = retryable;
      if (!retryable) this.transfers.delete(t.id);
      emit({ type: "failed", transferId: t.id, error, retryable });
    };
    emit({ type: "started", transferId: t.id, name: t.name, direction: t.direction, total: t.size });
    if (t.localIsDir) return fail({ code: "localIsADirectory", params: { path: t.name } }, false);
    const target = t.direction === "upload" ? joinPath(t.remoteDir, t.name) : t.remotePath;
    if (t.direction === "upload" && target && this.fs.has(target) && !overwrite) {
      // Same as the backend: the setting decides when the file exists and the call did not force it.
      const policy = this.settings.transfers.onConflict;
      if (policy === "skip") {
        this.transfers.delete(t.id);
        return emit({ type: "skipped", transferId: t.id });
      }
      if (policy === "ask") return fail({ code: "remoteExists", params: { path: target } }, true);
    }
    const steps = 4;
    for (let i = 1; i <= steps; i += 1) {
      await this.pause();
      if (t.cancelled) {
        this.transfers.delete(t.id);
        emit({ type: "cancelled", transferId: t.id });
        return;
      }
      if (this.sessions.get(t.sessionId)?.lastState?.phase !== "connected") {
        return fail({ code: "disconnected", params: { reason: "connectionReset", message: null } }, true);
      }
      emit({ type: "progress", transferId: t.id, bytes: Math.round((t.size * i) / steps), total: t.size });
    }
    this.transfers.delete(t.id);
    const entry = t.direction === "upload" && target ? this.fs.put(target, t.size, true) : null;
    emit({ type: "done", transferId: t.id, bytes: t.size, entry });
  }

  async transferDownload(
    sessionId: string,
    remotePath: string,
    _dialogTitle: string,
    onEvent: (event: TransferEvent) => void,
  ): Promise<string | null> {
    const session = this.requireSftp(sessionId);
    const entry = this.fs.stat(remotePath);
    if (entry.kind === "dir") throw new IpcError({ code: "isADirectory", params: { path: remotePath } });
    return this.startTransfer(
      session,
      { name: entry.name, direction: "download", size: entry.size ?? 0, remoteDir: dirname(remotePath), remotePath },
      onEvent,
      true,
    );
  }
  async transferUploadPick(
    sessionId: string,
    remoteDir: string,
    _dialogTitle: string,
    overwrite: boolean,
    onEvent: (event: TransferEvent) => void,
  ): Promise<string[]> {
    const session = this.requireSftp(sessionId);
    const picked = this.hooks.pickedFiles ?? [{ name: "report.txt", size: 40_000 }];
    return picked.map((f) =>
      this.startTransfer(session, { name: f.name, direction: "upload", size: f.size, remoteDir, remotePath: null }, onEvent, overwrite),
    );
  }
  async transferUploadDropped(
    sessionId: string,
    dropId: string,
    remoteDir: string,
    overwrite: boolean,
    onEvent: (event: TransferEvent) => void,
  ): Promise<string[]> {
    const session = this.requireSftp(sessionId);
    const drop = this.drops.get(dropId);
    // The registry entry is consumed by the first call and expires after 60 s (§6.6).
    this.drops.delete(dropId);
    if (!drop || this.now() - drop.at > DROP_TTL_MS) throw new IpcError({ code: "dropExpired", params: { dropId } });
    return drop.entries.map((e) =>
      this.startTransfer(
        session,
        { name: e.name, direction: "upload", size: 30_000, remoteDir, remotePath: null, localIsDir: e.isDir },
        onEvent,
        overwrite,
      ),
    );
  }
  async transferRetry(transferId: string, overwrite: boolean, onEvent: (event: TransferEvent) => void): Promise<string> {
    const old = this.transfers.get(transferId);
    if (!old || !old.failed) throw new IpcError({ code: "notFound", params: { what: "transfer", id: transferId } });
    const session = this.sessions.get(old.sessionId);
    if (!session) throw new IpcError({ code: "sessionNotFound", params: { sessionId: old.sessionId } });
    this.transfers.delete(transferId);
    return this.startTransfer(
      session,
      { name: old.name, direction: old.direction, size: old.size, remoteDir: old.remoteDir, remotePath: old.remotePath },
      onEvent,
      overwrite,
    );
  }
  async transferCancel(transferId: string): Promise<void> {
    const t = this.transfers.get(transferId);
    if (!t) return;
    t.cancelled = true;
    if (t.failed) {
      this.transfers.delete(transferId);
      t.onEvent({ type: "cancelled", transferId });
    }
  }

  // ---- test / dev helpers (the real backend does these on its own) ----
  /** An OS file drop: the backend keeps the paths and registers a `dropId`; the UI gets names and `isDir` only. */
  simulateDrop(entries: DropEntry[], position: { x: number; y: number } = { x: 0, y: 0 }): string {
    const dropId = this.id("drop");
    this.drops.set(dropId, { entries, at: this.now() });
    this.emitGlobal("drop:files", { dropId, entries, positionX: position.x, positionY: position.y });
    return dropId;
  }
  simulateDropHover(x: number, y: number): void {
    this.emitGlobal("drop:hover", { positionX: x, positionY: y });
  }
  simulateDropLeave(): void {
    this.emitGlobal("drop:leave", {});
  }

  /** Connection lost, auto-reconnect waiting (the §5.8 `reconnecting` phase). */
  loseConnection(sessionId: string): void {
    const session = this.sessions.get(sessionId);
    if (!session) return;
    const cause: AppError = { code: "disconnected", params: { reason: "keepaliveTimeout", message: null } };
    session.lastState = {
      phase: "reconnecting",
      stage: "waiting",
      hop: null,
      attempt: 1,
      maxAttempts: 5,
      nextAttemptAtMs: this.now() + 1000,
      cause,
    };
    session.onEvent({ type: "state", state: session.lastState });
  }

  /** The reconnect succeeded: SFTP works again and terminals get a new shell (`reconnected`). */
  restoreConnection(sessionId: string): void {
    const session = this.sessions.get(sessionId);
    if (!session) return;
    const hop = { index: session.hop.index, label: session.hop.label, user: session.hop.user, host: session.hop.host, port: session.hop.port };
    const cause: AppError = { code: "disconnected", params: { reason: "keepaliveTimeout", message: null } };
    for (const stage of ["connecting", "authenticating"] as const) {
      session.lastState = { phase: "reconnecting", stage, hop, attempt: 1, maxAttempts: 5, nextAttemptAtMs: null, cause };
      session.onEvent({ type: "state", state: session.lastState });
    }
    session.lastState = { phase: "connected", info: session.info };
    session.onEvent({ type: "state", state: session.lastState });
    this.respawnShells(sessionId);
  }

  /** Number of `session_probe_all` calls so far. */
  probeCount = 0;

  // ---- updates (models the updater contract, src-tauri/src/updater.rs) ----
  /** The update the channel manifest offers (`null` = up to date). Tests set it directly. */
  publishedUpdate: UpdateInfo | null = null;
  /** The version stored by "Skip this version". */
  skippedUpdateVersion: string | null = null;
  /** `update_check` rejects with this (e.g. `updatesDisabled`, `updateCheckFailed`). */
  updateCheckFailure: AppError | null = null;
  /** `update_install` rejects with this after the progress events (e.g. `updateVerifyFailed`). */
  updateDownloadFailure: AppError | null = null;
  /** The staged install fails at the end of the quit flow (`update:install-failed`). */
  updateInstallFailure: AppError | null = null;
  /** Holds the download open until `releaseDownload()` (to test progress and cancel). */
  holdDownload = false;
  /** The version installed at the end of the quit flow (tests). */
  installedUpdate: UpdateInfo | null = null;
  private currentUpdate: UpdateInfo | null = null;
  private stagedUpdate: UpdateInfo | null = null;
  private downloading: { cancel: () => void; release: () => void } | null = null;

  /** Like the automatic check of the backend: announces the published update unless it was skipped. */
  announceUpdate(): void {
    const update = this.publishedUpdate;
    if (!update || this.skippedUpdateVersion === update.version) return;
    this.currentUpdate = update;
    this.emitGlobal("update:available", update);
  }

  releaseDownload(): void {
    this.downloading?.release();
  }

  async updateCheck(): Promise<UpdateCheckOutcome> {
    await this.delay();
    if (this.updateCheckFailure) throw new IpcError(this.updateCheckFailure);
    if (this.downloading || this.stagedUpdate) throw new IpcError({ code: "updateInProgress" });
    this.currentUpdate = this.publishedUpdate;
    return this.publishedUpdate ? { status: "available", update: this.publishedUpdate } : { status: "upToDate" };
  }

  async updateInstall(version: string, onProgress: (event: UpdateProgress) => void): Promise<void> {
    await this.delay();
    const update = this.currentUpdate;
    if (update?.install === "manualDownload") throw new IpcError({ code: "updateManualOnly" });
    if (this.downloading || this.stagedUpdate) throw new IpcError({ code: "updateInProgress" });
    if (!update || update.version !== version) throw new IpcError({ code: "updateNotFound" });
    const total = 4 * 1024 * 1024;
    onProgress({ phase: "downloading", downloaded: total / 4, total });
    if (this.holdDownload) {
      const outcome = await new Promise<"done" | "cancelled">((resolve) => {
        this.downloading = { cancel: () => resolve("cancelled"), release: () => resolve("done") };
      });
      this.downloading = null;
      if (outcome === "cancelled") throw new IpcError({ code: "cancelled" });
    }
    onProgress({ phase: "downloading", downloaded: total, total });
    if (this.updateDownloadFailure) throw new IpcError(this.updateDownloadFailure);
    onProgress({ phase: "ready" });
    this.stagedUpdate = update;
    // Like the backend: the quit flow starts (it asks about unsaved changes); the install happens at its end.
    this.emitGlobal("app:quit-requested", {});
  }

  async updateCancel(): Promise<void> {
    this.downloading?.cancel();
    this.stagedUpdate = null;
  }

  async updateSkipVersion(version: string): Promise<void> {
    if (!/^[0-9][0-9A-Za-z.+-]{0,63}$/.test(version)) {
      throw new IpcError({ code: "invalidArgument", params: { field: "version" } });
    }
    this.skippedUpdateVersion = version;
  }

  // ---- events ----
  async listen<E extends BackendEventName>(event: E, callback: (payload: BackendEvents[E]) => void): Promise<Unlisten> {
    let set = this.listeners.get(event);
    if (!set) {
      set = new Set();
      this.listeners.set(event, set);
    }
    const cb = callback as (payload: never) => void;
    set.add(cb);
    return () => {
      set.delete(cb);
    };
  }
}
