// Fixture data for the MockBackend, matching the "A · Graphite" mockups.
import type {
  AgentStatus,
  AppBootstrap,
  Bookmark,
  BookmarksDto,
  EnvInfo,
  RecentEntry,
  Settings,
  SshConfigHost,
  SshConfigSnapshot,
  UpdateInfo,
} from "../types";

/** Updater: the update the `updateAvailable` / `updateManualDownload` scenarios publish. */
export function makeMockUpdate(install: "self" | "manual" = "self"): UpdateInfo {
  return {
    version: "0.2.0",
    currentVersion: "0.1.0",
    date: "2026-10-20T12:00:00Z",
    notes: "## Shellfin 0.2.0\n\n- Faster file tree\n- Fixed: terminal resize after reconnect",
    notesTruncated: false,
    install: install === "self" ? "selfUpdate" : "manualDownload",
    releaseUrl: "https://github.com/shellfin-app/shellfin/releases/tag/v0.2.0",
  };
}

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

export const SSH_CONFIG_FILE = "~/.ssh/config";

function configHost(alias: string, hostname: string, line: number, extra: Partial<SshConfigHost> = {}): SshConfigHost {
  return {
    alias,
    hostname,
    user: "deploy",
    port: 22,
    proxyJump: null,
    identityFiles: ["~/.ssh/id_ed25519"],
    origin: { file: SSH_CONFIG_FILE, line },
    hasWarnings: false,
    ...extra,
  };
}

export function makeSshConfig(now: number): SshConfigSnapshot {
  return {
    mainFile: SSH_CONFIG_FILE,
    files: [SSH_CONFIG_FILE, "~/.ssh/config.d/work.conf"],
    hosts: [
      configHost("prod-web-01", "10.0.4.12", 3),
      configHost("prod-web-02", "10.0.4.13", 8),
      configHost("staging-db", "10.0.6.4", 13, { user: "postgres", proxyJump: "bastion" }),
      configHost("bastion", "jump.example.net", 19, { user: "jump" }),
      configHost("gitlab-runner", "10.0.9.2", 24, { user: "runner" }),
    ],
    warnings: [],
    droppedWarnings: 0,
    loadedAtMs: now,
  };
}

function bookmark(id: string, name: string, host: string, port: number | null, user: string, now: number, order: number): Bookmark {
  return {
    id,
    name,
    host,
    port,
    user,
    auth: { method: "auto" },
    proxyJump: null,
    groupId: null,
    order,
    color: null,
    tags: [],
    startDir: null,
    useSshConfig: true,
    uploadLimitKiBps: null,
    downloadLimitKiBps: null,
    confirmConnect: false,
    startupCommand: null,
    notes: null,
    createdAtMs: now - 30 * DAY,
    updatedAtMs: now - 30 * DAY,
  };
}

export function makeBookmarks(now: number): BookmarksDto {
  return {
    bookmarks: [
      bookmark("11111111-1111-4111-8111-111111111111", "pi-home", "192.168.1.40", null, "pi", now, 0),
      bookmark("22222222-2222-4222-8222-222222222222", "nas", "nas.local", 2222, "admin", now, 1),
      bookmark("33333333-3333-4333-8333-333333333333", "vps-hetzner", "95.216.0.17", null, "root", now, 2),
    ],
    groups: [],
    readOnly: false,
  };
}

export function makeRecents(now: number): RecentEntry[] {
  return [
    {
      target: { kind: "sshConfig", alias: "prod-web-01" },
      label: "prod-web-01",
      userAtHost: "deploy@10.0.4.12",
      via: null,
      lastConnectedAtMs: now - 2 * HOUR,
      lastDir: "/srv/app",
      terminalCount: 3,
    },
    {
      target: { kind: "sshConfig", alias: "staging-db" },
      label: "staging-db",
      userAtHost: "postgres@10.0.6.4",
      via: "bastion",
      lastConnectedAtMs: now - DAY,
      lastDir: null,
      terminalCount: 1,
    },
    {
      target: { kind: "bookmark", id: "11111111-1111-4111-8111-111111111111" },
      label: "pi-home",
      userAtHost: "pi@192.168.1.40",
      via: null,
      lastConnectedAtMs: now - 3 * DAY,
      lastDir: "/home/pi",
      terminalCount: 0,
    },
  ];
}

export function makeSettings(): Settings {
  return {
    version: 1,
    locale: "system",
    general: {
      confirmCloseWithTerminals: true,
      confirmQuit: "withConnections",
      onStartup: "welcome",
      notifyOnDisconnect: false,
      autoUpdate: true,
    },
    terminal: {
      fontSize: 13,
      scrollback: 10000,
      cursorBlink: true,
      copyOnSelect: false,
      fontFamily: null,
      cursorStyle: "block",
      lineHeight: 1.2,
      bell: "off",
      rightClick: "menu",
      letterSpacing: 0,
      mouseReporting: true,
      hideMousePointerWhileTyping: false,
      minimumContrastRatio: 1,
      wordSeparators: " ()[]{}'\"",
      multilinePasteWarning: true,
      onExit: "keep",
      newTerminalCwd: "home",
      openTerminalOnConnect: true,
      termType: "xterm-256color",
      altIsMeta: false,
      scrollSensitivity: 1,
      background: null,
      screenReaderMode: false,
    },
    editor: {
      fontSize: 13,
      tabSize: 2,
      wordWrap: false,
      maxFileSizeMiB: 5,
      fontFamily: null,
      lineNumbers: true,
      highlightActiveLine: true,
      indentWithTabs: false,
      trimTrailingWhitespace: false,
      insertFinalNewline: false,
      detectIndentation: true,
      newFileEol: "lf",
      autoCloseBrackets: false,
      bracketMatching: true,
      showWhitespace: "none",
      codeFolding: true,
      fontLigatures: false,
    },
    connection: {
      keepaliveIntervalSecs: 15,
      keepaliveCountMax: 3,
      connectTimeoutSecs: 15,
      promptTimeoutSecs: 110,
      reconnect: { enabled: true, maxAttempts: 5 },
      defaultUser: "",
      readSshConfig: true,
      rememberPasswords: true,
    },
    files: {
      showHidden: true,
      foldersFirst: true,
      confirmDelete: true,
      doubleClick: "open",
      sizeUnits: "binary",
      dateFormat: "relative",
      showPermissions: false,
      autoRefresh: 0,
    },
    ui: { sidebarWidth: 280, terminalPanelHeight: 300, terminalPanelOpen: true },
    transfers: {
      uploadLimitKiBps: 0,
      downloadLimitKiBps: 0,
      maxConcurrent: 3,
      onConflict: "ask",
      preserveTimestamps: true,
      notifyOnComplete: false,
    },
    advanced: { logLevel: "info" },
    appearance: { theme: "dark", accent: "teal", uiFontSize: 13, reduceMotion: "system" },
  };
}

export function makeAgent(): AgentStatus {
  return {
    available: true,
    kind: "opensshPipe",
    endpoint: "\\\\.\\pipe\\openssh-ssh-agent",
    identities: [
      { keyType: "ssh-ed25519", fingerprint: "SHA256:0f4Q1mW8xk3dQ0v2yq9tJt7Zl2x0Yb5cRr1uXn8PzAs", comment: "id_ed25519" },
      { keyType: "ssh-rsa", fingerprint: "SHA256:9aB3kLq7Vd2sPz1Hn6TgE4uYw0MxCj5rFo8iQe2NhKc", comment: "work_rsa", bits: 4096 },
    ],
  };
}

export function makeEnv(now: number): EnvInfo {
  return {
    sshConfig: { path: SSH_CONFIG_FILE, exists: true, hostCount: 5, includeCount: 1, loadedAtMs: now },
    knownHosts: { path: "~/.ssh/known_hosts", entryCount: 14 },
    agent: makeAgent(),
    logDir: "~/.local/share/app.shellfin.desktop/logs",
  };
}

export function makeBootstrap(now: number): AppBootstrap {
  return {
    appVersion: "0.1.0",
    platform: "windows",
    settings: makeSettings(),
    sshConfig: makeSshConfig(now),
    bookmarks: makeBookmarks(now),
    recents: makeRecents(now),
    warnings: [],
    window: { maximized: false, fullscreen: false, focused: true },
    restoreBookmarkIds: [],
  };
}
