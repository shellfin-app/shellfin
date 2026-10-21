// Terminal registry (docs/ARCHITECTURE.md §8.6): a module-level Map of xterm.js instances, NOT in Zustand.
// Instances survive tab and session switches (scrollback is kept); parked terminals (not visible) keep receiving
// writes. Output never goes through React. Lifecycle: createTerminal -> attachTerminal/detach (any number of
// times) -> disposeTerminal (tab closed / session closed), which releases the xterm instance, its addons, timers,
// listeners and DOM node.
import type { IBufferLine, IDisposable, ILink } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import i18n from "i18next";
import { errorMessage } from "../../i18n/errorMessage";
import { formatBytes } from "../../i18n/format";
import { getBackend, isRawBytes } from "../../ipc";
import type { TermMessage } from "../../ipc";
import type { Platform, TermEvent } from "../../ipc/types";
import { CAN_SGR_RESET, dimNotice } from "../../lib/ansi";
import { describeKeyEvent, matchShortcut, terminalClipboardKey } from "../../lib/keybindings";
import { shortcutSwallowedNotice } from "../../lib/notices";
import { readFlag, writeFlag } from "../../state/layoutPrefs";
import { playBeep } from "../../lib/beep";
import { CSI_PRIVATE_PREFIX, CSI_SET_FINAL } from "../../lib/identifiers";
import { findUrls, isOpenableUrl } from "../../lib/links";
import { RESET_MOUSE_MODES, resetModesSequence, splitMouseModes } from "../../lib/mouseModes";
import { createPasteListener, pasteText, type PasteTarget } from "../../lib/paste";
import { guard, reportError } from "../../lib/reportError";
import { displayServerText } from "../../lib/serverText";
import { useStore } from "../../state/store";
import { terminalFontStack } from "../../lib/fontFamily";
import { terminalFontSpec } from "../../styles/xtermTheme";
import { subscribeAppearance } from "../settings/appearance";
import { isModalOpen } from "../session/promptVisibility";
import { AckTracker } from "./ackTracker";
import { requestPasteConfirm } from "./pasteConfirm";
import { currentSearchDecorations, currentTerminalTheme } from "./terminalTheme";
import { createTitleCoalescer } from "./titleCoalescer";

const WRITE_CHUNK = 64 * 1024;
const RESIZE_DEBOUNCE_MS = 50;

interface Size {
  cols: number;
  rows: number;
}

export interface TerminalEntry {
  termId: string;
  sessionId: string;
  term: Terminal;
  host: HTMLDivElement;
  fit: FitAddon;
  search: SearchAddon;
  webgl: WebglAddon | null;
  acks: AckTracker;
  resizeTimer: ReturnType<typeof setTimeout> | null;
  /** The size the backend knows (`term_open` / last `term_resize`). */
  sentSize: Size;
  restarting: boolean;
  /** Counts `exit` / `closed` events: a restart must not overwrite an end that arrived while it was in flight. */
  endEvents: number;
  /** The one paste path (event listener and context menu). */
  pasteTarget: PasteTarget;
  /** Everything to undo in `disposeTerminal` (listeners, link provider ...). */
  cleanup: (() => void)[];
  /** Whether the emulator may currently have mouse tracking on (`terminal.mouseReporting`); undefined = yes. */
  mouseReportingOn?: boolean;
}

const registry = new Map<string, TerminalEntry>();
let parking: HTMLDivElement | null = null;

function parkingLot(): HTMLDivElement {
  if (!parking || !parking.isConnected) {
    parking = document.createElement("div");
    parking.hidden = true;
    parking.setAttribute("aria-hidden", "true");
    document.body.appendChild(parking);
  }
  return parking;
}

export function getTerminal(termId: string): TerminalEntry | undefined {
  return registry.get(termId);
}

/** Number of live terminal instances (tests, diagnostics). */
export function terminalCount(): number {
  return registry.size;
}

function statusOf(termId: string): "running" | "exited" | "closed" | undefined {
  return useStore.getState().terminals[termId]?.status;
}

/**
 * Per-terminal send queue. Tauri runs every async command as its own task, so fire-and-forget `term_write`
 * calls can be applied out of order (a 64 KiB chunk parses slower than a later small one). Chunks are therefore sent
 * one after another: the next `termWrite` is issued only after the previous one settled (success or failure).
 * The queue is bounded by what the callers already bound (paste limits, typing speed); it is dropped when the
 * terminal is disposed.
 */
interface SendQueue {
  pending: Uint8Array[];
  busy: boolean;
}
const sendQueues = new Map<string, SendQueue>();

function pumpInput(termId: string, queue: SendQueue): void {
  if (queue.busy) return;
  const first = queue.pending.shift();
  if (!first) {
    sendQueues.delete(termId);
    return;
  }
  const chunk = first;
  queue.busy = true;
  const done = () => {
    queue.busy = false;
    if (sendQueues.get(termId) === queue) pumpInput(termId, queue);
  };
  // `notConnected` etc. are expected while reconnecting (the overlay blocks input); ignore and carry on.
  getBackend()
    .termWrite(termId, chunk)
    .then(done, done);
}

function sendInput(termId: string, bytes: Uint8Array): void {
  if (bytes.length === 0) return;
  let queue = sendQueues.get(termId);
  if (!queue) {
    queue = { pending: [], busy: false };
    sendQueues.set(termId, queue);
  }
  for (let offset = 0; offset < bytes.length; offset += WRITE_CHUNK) {
    queue.pending.push(bytes.slice(offset, offset + WRITE_CHUNK));
  }
  pumpInput(termId, queue);
}

function writeClipboard(text: string): void {
  if (text === "") return;
  navigator.clipboard.writeText(text).catch(() => undefined);
}

/** `terminal.onExit`: does a shell that ended with `code` take its tab with it? */
export function closesOnExit(code: number | null): boolean {
  const mode = useStore.getState().settings?.terminal.onExit ?? "keep";
  return mode === "close" || (mode === "closeOnSuccess" && code === 0);
}

function closeEndedTerminal(entry: TerminalEntry): void {
  const { sessionId, termId } = entry;
  disposeTerminal(termId);
  const s = useStore.getState();
  s.removeTerminalTab(sessionId, termId);
  s.forgetTerminal(termId);
}

/** Exported for tests. */
export function handleTermEvent(entry: TerminalEntry, event: TermEvent): void {
  const { patchTerminal } = useStore.getState();
  switch (event.type) {
    case "exit":
      entry.endEvents += 1;
      patchTerminal(entry.termId, { status: "exited", exitCode: event.code });
      if (closesOnExit(event.code)) {
        // `terminal.onExit`: the tab goes away with its shell (nothing to restart, nothing to read).
        closeEndedTerminal(entry);
        break;
      }
      entry.term.write(
        dimNotice(
          event.code !== null
            ? i18n.t("terminal.exited", { code: event.code })
            : event.signal !== null
              ? i18n.t("terminal.exitedSignal", { signal: displayServerText(event.signal, 64) })
              : i18n.t("terminal.exitedNoCode"),
        ),
      );
      break;
    case "closed":
      entry.endEvents += 1;
      patchTerminal(entry.termId, { status: "closed" });
      entry.term.write(
        dimNotice(
          event.error === null
            ? i18n.t("terminal.closed")
            : i18n.t("terminal.closedWithError", { reason: displayServerText(errorMessage(event.error), 256) }),
        ),
      );
      break;
    case "reconnected":
      patchTerminal(entry.termId, { status: "running", exitCode: null });
      entry.term.write(dimNotice(i18n.t("terminal.reconnected")));
      break;
    case "outputDropped":
      // CAN first: it returns xterm's parser to the ground state even in the middle of a dropped escape sequence.
      entry.term.write(CAN_SGR_RESET + dimNotice(i18n.t("terminal.outputDropped", { size: formatBytes(event.bytes) })));
      break;
  }
}

/** Exported for tests. */
export function handleMessage(entry: TerminalEntry, message: TermMessage): void {
  // Messages already queued on the Channel after `disposeTerminal` must not reach the disposed xterm.
  if (registry.get(entry.termId) !== entry) return;
  if (isRawBytes(message)) {
    const bytes = new Uint8Array(message);
    const length = bytes.length;
    entry.term.write(bytes, () => {
      const n = entry.acks.processed(length);
      if (n > 0) getBackend().termAck(entry.termId, n).catch(() => undefined);
    });
  } else {
    handleTermEvent(entry, message);
  }
}

/** Ctrl/Cmd+click on a link: the backend shows the native confirmation with the real URL (§8.6, §9.4). */
export function openLink(event: MouseEvent, uri: string): void {
  if (!(event.ctrlKey || event.metaKey) || !isOpenableUrl(uri)) return;
  guard(getBackend().appOpenExternal(uri, i18n.t("link.confirmTitle"), i18n.t("link.confirmBody")));
}

/** Text of one buffer line plus, per UTF-16 unit, the 1-based first/last cell column (wide characters). */
function lineText(line: IBufferLine): { text: string; startCol: number[]; endCol: number[] } {
  let text = "";
  const startCol: number[] = [];
  const endCol: number[] = [];
  for (let x = 0; x < line.length; x++) {
    const cell = line.getCell(x);
    if (!cell) continue;
    const width = cell.getWidth();
    if (width === 0) continue; // second half of a wide character
    const chars = cell.getChars() || " ";
    for (let i = 0; i < chars.length; i++) {
      text += chars[i];
      startCol.push(x + 1);
      endCol.push(x + width);
    }
  }
  return { text, startCol, endCol };
}

/** Plain-text http(s) links on buffer row `y` (1-based); Ctrl/Cmd+click opens them (exported for tests). */
export function linksForRow(term: Terminal, y: number): ILink[] {
  const line = term.buffer.active.getLine(y - 1);
  if (!line) return [];
  const { text, startCol, endCol } = lineText(line);
  const links: ILink[] = [];
  for (const found of findUrls(text)) {
    const first = startCol[found.start];
    const last = endCol[found.end - 1];
    if (first === undefined || last === undefined) continue;
    links.push({
      range: { start: { x: first, y }, end: { x: last, y } },
      text: found.url,
      activate: (event, uri) => openLink(event, uri),
    });
  }
  return links;
}

function registerUrlLinks(term: Terminal): IDisposable {
  return term.registerLinkProvider({
    provideLinks(y, callback) {
      const links = linksForRow(term, y);
      callback(links.length > 0 ? links : undefined);
    },
  });
}

// ---- swallowed shortcuts ----

let swallowedHintShown = false;
const SWALLOWED_FLAG = "shortcutSwallowed";

/**
 * An app shortcut (Ctrl+B, Ctrl+N, Ctrl+W ...) typed into the focused terminal goes to the shell, by design. The first
 * time that happens, say so once (and where the shortcuts do work), so "nothing happens" is not a mystery.
 */
function maybeHintSwallowedShortcut(ev: KeyboardEvent, platform: Platform): void {
  if (swallowedHintShown || matchShortcut(ev, platform, false) === null) return;
  swallowedHintShown = true;
  if (readFlag(SWALLOWED_FLAG)) return;
  writeFlag(SWALLOWED_FLAG);
  useStore.getState().pushInfo(shortcutSwallowedNotice(describeKeyEvent(ev, platform)));
}

/** Visual bell: the host gets this class for BELL_FLASH_MS (terminal.css animates it). */
export const BELL_CLASS = "terminal-host--bell";
/** `hideMousePointerWhileTyping`: the host gets this class after a key press (terminal.css hides the cursor). */
export const POINTER_HIDDEN_CLASS = "terminal-host--typing";
const BELL_FLASH_MS = 150;
/** How long the Linux paste chord waits for the native `paste` event before reading the clipboard itself. */
export const PASTE_FALLBACK_MS = 120;
/**
 * After the fallback pasted, a native `paste` event that arrives this late (a slow clipboard read in the webview) belongs
 * to the same chord and is swallowed, so the text is not pasted a second time.
 */
export const PASTE_LATE_NATIVE_MS = 1000;

// ---- settings sync ----

let unsubscribeSettings: (() => void) | null = null;

/**
 * `terminal.mouseReporting`: switching it off live also resets the mouse modes a program already switched on, so the
 * mouse selects again at once; the handler registered in `createTerminal` keeps new requests out.
 */
function applyMouseReporting(entry: TerminalEntry, enabled: boolean): void {
  if (!enabled && entry.mouseReportingOn !== false) entry.term.write(RESET_MOUSE_MODES);
  entry.mouseReportingOn = enabled;
}

function applySettings(entry: TerminalEntry): void {
  const settings = useStore.getState().settings?.terminal;
  if (!settings) return;
  const { term } = entry;
  const family = terminalFontStack(settings.fontFamily);
  const metricsChanged =
    term.options.fontSize !== settings.fontSize ||
    term.options.fontFamily !== family ||
    term.options.letterSpacing !== settings.letterSpacing;
  term.options.letterSpacing = settings.letterSpacing;
  term.options.minimumContrastRatio = settings.minimumContrastRatio;
  term.options.wordSeparator = settings.wordSeparators;
  term.options.scrollSensitivity = settings.scrollSensitivity;
  term.options.screenReaderMode = settings.screenReaderMode;
  term.options.macOptionIsMeta = useStore.getState().platform === "macos" && settings.altIsMeta;
  applyMouseReporting(entry, settings.mouseReporting);
  term.options.fontSize = settings.fontSize;
  term.options.fontFamily = family;
  term.options.scrollback = settings.scrollback;
  term.options.cursorBlink = settings.cursorBlink;
  term.options.cursorStyle = settings.cursorStyle;
  const lineHeightChanged = term.options.lineHeight !== settings.lineHeight;
  term.options.lineHeight = settings.lineHeight;
  if ((metricsChanged || lineHeightChanged) && entry.host.parentElement && entry.host.parentElement !== parking) {
    fitNow(entry);
    if (term.rows > 0) term.refresh(0, term.rows - 1);
  }
}

function ensureSettingsSync(): void {
  if (unsubscribeSettings) return;
  unsubscribeSettings = useStore.subscribe((state, previous) => {
    if (state.settings?.terminal === previous.settings?.terminal) return;
    for (const entry of registry.values()) applySettings(entry);
  });
  // Theme, accent, system theme and terminal.background: re-theme after the appearance reached <html>.
  subscribeAppearance(() => {
    const theme = currentTerminalTheme();
    for (const entry of registry.values()) entry.term.options.theme = theme;
  });
}

function fitNow(entry: TerminalEntry): void {
  const parent = entry.host.parentElement;
  if (!parent || parent.clientWidth <= 0 || parent.clientHeight <= 0) return;
  try {
    entry.fit.fit();
  } catch {
    // The terminal may be mid-dispose.
  }
}

function scheduleResize(entry: TerminalEntry): void {
  if (entry.resizeTimer) clearTimeout(entry.resizeTimer);
  entry.resizeTimer = setTimeout(() => {
    entry.resizeTimer = null;
    const { cols, rows } = entry.term;
    if (cols === entry.sentSize.cols && rows === entry.sentSize.rows) return;
    entry.sentSize = { cols, rows };
    getBackend()
      .termResize(entry.termId, { cols, rows, pixelWidth: entry.host.clientWidth, pixelHeight: entry.host.clientHeight })
      .catch(() => undefined);
  }, RESIZE_DEBOUNCE_MS);
}

/** Enter in an exited/closed terminal respawns the shell (§4.5). */
/** Restarts the shell of an ended terminal (the same action as pressing Enter in it). */
export async function restartTerminalById(termId: string): Promise<void> {
  const entry = registry.get(termId);
  if (entry) await restartTerminal(entry);
}

async function restartTerminal(entry: TerminalEntry): Promise<void> {
  if (entry.restarting) return;
  entry.restarting = true;
  try {
    const endsBefore = entry.endEvents;
    await getBackend().termRestart(entry.termId);
    // The new shell may already have ended (its exit can overtake this response): never overwrite that.
    if (entry.endEvents === endsBefore) {
      useStore.getState().patchTerminal(entry.termId, { status: "running", exitCode: null });
    }
  } catch (e) {
    reportError(e);
  } finally {
    entry.restarting = false;
  }
}

/**
 * Opens a terminal on the backend and registers its xterm instance. Returns the termId, or `null` when the
 * session was closed in the meantime (everything is cleaned up).
 */
export async function createTerminal(
  sessionId: string,
  cwd: string | null = null,
  cwdFallback = false,
): Promise<string | null> {
  const state = useStore.getState();
  const settings = state.settings;
  const fontSize = settings?.terminal.fontSize ?? 13;

  // Make sure the monospace font is loaded before the first measurement (§8.5).
  try {
    await document.fonts.load(terminalFontSpec(fontSize));
  } catch {
    // Font loading is best-effort (jsdom has no FontFaceSet).
  }

  const term = new Terminal({
    fontFamily: terminalFontStack(settings?.terminal.fontFamily),
    fontSize,
    scrollback: settings?.terminal.scrollback ?? 10000,
    cursorBlink: settings?.terminal.cursorBlink ?? true,
    cursorStyle: settings?.terminal.cursorStyle ?? "block",
    lineHeight: settings?.terminal.lineHeight ?? 1.2,
    letterSpacing: settings?.terminal.letterSpacing ?? 0,
    minimumContrastRatio: settings?.terminal.minimumContrastRatio ?? 1,
    scrollSensitivity: settings?.terminal.scrollSensitivity ?? 1,
    macOptionIsMeta: state.platform === "macos" && (settings?.terminal.altIsMeta ?? false),
    screenReaderMode: settings?.terminal.screenReaderMode ?? false,
    ...(settings ? { wordSeparator: settings.terminal.wordSeparators } : {}),
    allowProposedApi: true,
    theme: currentTerminalTheme(),
    // `windowOptions` is deliberately left at its default (all reporting off), §8.6.
    // `@xterm/addon-clipboard` (OSC 52) is deliberately NOT loaded.
    linkHandler: { allowNonHttpProtocols: false, activate: (event, uri) => openLink(event, uri) },
  });
  const fit = new FitAddon();
  const search = new SearchAddon();
  term.loadAddon(fit);
  term.loadAddon(search);
  term.loadAddon(new Unicode11Addon());
  term.unicode.activeVersion = "11";

  const host = document.createElement("div");
  host.className = "terminal-host";
  parkingLot().appendChild(host);
  term.open(host);

  // `terminal.mouseReporting = false`: a program asking for mouse tracking (DECSET 9/1000-1016) is not obeyed, so the
  // mouse keeps selecting text. Disabling (`l`) always passes. A sequence of mouse modes only is swallowed; one that
  // mixes them with other modes (e.g. 1049, the alternate screen) is applied by xterm in order and the mouse modes
  // are reset right after it, so the other modes are never reordered against the output around them.
  const mouseGuard = term.parser.registerCsiHandler({ prefix: CSI_PRIVATE_PREFIX, final: CSI_SET_FINAL }, (params) => {
    if (useStore.getState().settings?.terminal.mouseReporting !== false) return false;
    const { mouse, rest } = splitMouseModes(params);
    if (mouse.length === 0) return false;
    if (rest.length === 0) return true;
    term.write(resetModesSequence(mouse));
    return false;
  });

  // Messages can arrive before `openTerminal` resolves with the termId.
  let entry: TerminalEntry | null = null;
  const early: TermMessage[] = [];
  const discard = () => {
    term.dispose();
    host.remove();
  };
  const openSize: Size = { cols: term.cols, rows: term.rows };
  let termId: string;
  try {
    termId = await getBackend().openTerminal(
      sessionId,
      { ...openSize, pixelWidth: 0, pixelHeight: 0 },
      cwd,
      (message) => {
        if (entry) handleMessage(entry, message);
        else early.push(message);
      },
      cwdFallback,
    );
  } catch (e) {
    discard(); // no xterm instance / DOM node leak when the backend refuses
    throw e;
  }

  // The session may have been closed while `term_open` was pending: do not register an orphan.
  if (!useStore.getState().sessionsById[sessionId]) {
    discard();
    getBackend().termClose(termId).catch(() => undefined);
    return null;
  }

  const e: TerminalEntry = {
    termId,
    sessionId,
    term,
    host,
    fit,
    search,
    webgl: null,
    acks: new AckTracker(),
    resizeTimer: null,
    sentSize: openSize,
    restarting: false,
    endEvents: 0,
    pasteTarget: {
      paste: (text) => term.paste(text),
      bracketedPasteMode: () => term.modes.bracketedPasteMode,
      acceptsInput: () => statusOf(termId) === "running",
      confirm: requestPasteConfirm,
      warnMultiline: () => useStore.getState().settings?.terminal.multilinePasteWarning !== false,
    },
    cleanup: [() => mouseGuard.dispose()],
    mouseReportingOn: settings?.terminal.mouseReporting !== false,
  };
  entry = e;
  registry.set(termId, e);
  // The store entry exists BEFORE buffered messages are replayed: an early `exit` / `closed` must patch it.
  // With the home-folder fallback (`newTerminalCwd`) the real folder is unknown, so the tab does not claim one.
  useStore
    .getState()
    .addTerminal({ termId, sessionId, title: null, name: null, cwd: cwdFallback ? null : cwd, status: "running", exitCode: null });
  host.dataset["termId"] = termId;
  ensureSettingsSync();

  // Input. While the shell is gone, only Enter does something (restart); nothing is sent to a dead channel.
  const encoder = new TextEncoder();
  term.onData((data) => {
    const status = statusOf(termId);
    if (status !== undefined && status !== "running") {
      if (data === "\r") guard(restartTerminal(e));
      return;
    }
    sendInput(termId, encoder.encode(data));
  });
  term.onBinary((data) => {
    if (statusOf(termId) === "running") sendInput(termId, Uint8Array.from(data, (c) => c.charCodeAt(0) & 0xff));
  });
  term.onResize(() => scheduleResize(e));
  // Bell (setting): "visual" = a brief flash of the terminal pane; "sound" = a short Web Audio beep; "off" = nothing.
  let bellTimer: ReturnType<typeof setTimeout> | null = null;
  const bell = term.onBell(() => {
    const mode = useStore.getState().settings?.terminal.bell;
    if (mode === "sound") playBeep();
    if (mode !== "visual") return;
    host.classList.add(BELL_CLASS);
    if (bellTimer !== null) clearTimeout(bellTimer);
    bellTimer = setTimeout(() => host.classList.remove(BELL_CLASS), BELL_FLASH_MS);
  });
  e.cleanup.push(() => {
    bell.dispose();
    if (bellTimer !== null) clearTimeout(bellTimer);
  });
  // OSC titles are coalesced: the latest one is patched into the store once per animation frame, so a program that
  // sets the title in a loop costs one store update per frame, not one per sequence.
  const titles = createTitleCoalescer((title) => useStore.getState().patchTerminal(termId, { title }));
  const titleSub = term.onTitleChange((title) => titles.push(title));
  e.cleanup.push(() => {
    titleSub.dispose();
    titles.cancel();
  });
  const links = registerUrlLinks(term);
  e.cleanup.push(() => links.dispose());

  // Linux paste fallback: armed by the paste chord, disarmed by the native `paste` event (so it never pastes twice).
  // A native event that only arrives after the fallback already pasted is swallowed for the same reason.
  // Registered BEFORE the paste listener below, which stops the event for everything after it.
  let pasteFallback: ReturnType<typeof setTimeout> | null = null;
  let fallbackPastedAt: number | null = null;
  const disarmPasteFallback = () => {
    if (pasteFallback !== null) clearTimeout(pasteFallback);
    pasteFallback = null;
  };
  const onNativePaste = (ev: Event) => {
    if (pasteFallback !== null) {
      disarmPasteFallback(); // in time: the native paste is the one
      return;
    }
    const late = fallbackPastedAt !== null && Date.now() - fallbackPastedAt < PASTE_LATE_NATIVE_MS;
    fallbackPastedAt = null;
    if (late) {
      ev.preventDefault();
      ev.stopImmediatePropagation();
    }
  };
  const armPasteFallback = () => {
    disarmPasteFallback();
    fallbackPastedAt = null;
    pasteFallback = setTimeout(() => {
      pasteFallback = null;
      if (registry.get(termId) !== e) return;
      fallbackPastedAt = Date.now();
      guard(pasteClipboardInto(termId));
    }, PASTE_FALLBACK_MS);
  };
  host.addEventListener("paste", onNativePaste, { capture: true });
  e.cleanup.push(() => {
    host.removeEventListener("paste", onNativePaste, { capture: true });
    disarmPasteFallback();
  });

  // The ONE paste path: a capture-phase listener on the host runs before xterm's own (unsanitised) listeners.
  const onPaste = createPasteListener(e.pasteTarget);
  host.addEventListener("paste", onPaste, { capture: true });
  e.cleanup.push(() => host.removeEventListener("paste", onPaste, { capture: true }));

  // `terminal.hideMousePointerWhileTyping`: typing hides the pointer over the terminal, moving the mouse shows it again.
  const onTypingKey = (ev: KeyboardEvent) => {
    if (!useStore.getState().settings?.terminal.hideMousePointerWhileTyping) return;
    if (ev.key === "Shift" || ev.key === "Control" || ev.key === "Alt" || ev.key === "Meta") return;
    host.classList.add(POINTER_HIDDEN_CLASS);
  };
  const onPointerMove = () => host.classList.remove(POINTER_HIDDEN_CLASS);
  host.addEventListener("keydown", onTypingKey);
  host.addEventListener("mousemove", onPointerMove);
  e.cleanup.push(() => {
    host.removeEventListener("keydown", onTypingKey);
    host.removeEventListener("mousemove", onPointerMove);
  });

  // Copy-on-select (setting): copy when the selection gesture ends.
  const onMouseUp = () => {
    if (useStore.getState().settings?.terminal.copyOnSelect && term.hasSelection()) writeClipboard(term.getSelection());
  };
  host.addEventListener("mouseup", onMouseUp);
  e.cleanup.push(() => host.removeEventListener("mouseup", onMouseUp));

  // App shortcuts pass through to the window handler (return false); everything else, Ctrl+C/S/W/K included,
  // goes to the remote (§8.6, §8.7).
  term.attachCustomKeyEventHandler((ev) => {
    if (ev.type !== "keydown" || ev.isComposing) return true;
    const platform = useStore.getState().platform;
    const clipboard = terminalClipboardKey(ev, platform);
    if (clipboard === "copy") {
      writeClipboard(term.getSelection());
      ev.preventDefault();
      return false;
    }
    // Paste: no preventDefault, so the browser fires the `paste` event that the capture listener sanitises.
    if (clipboard === "paste") {
      // WebKitGTK may not turn Ctrl+Shift+V into a `paste` event: if none arrives, read the clipboard.
      if (platform === "linux") armPasteFallback();
      return false;
    }
    const appShortcut = matchShortcut(ev, platform, true) !== null;
    if (!appShortcut) maybeHintSwallowedShortcut(ev, platform);
    return !appShortcut;
  });

  for (const m of early) handleMessage(e, m);
  return termId;
}

/** Shows the terminal inside `container`. Returns a function that parks it again. */
export function attachTerminal(termId: string, container: HTMLElement): () => void {
  const entry = registry.get(termId);
  if (!entry) return () => undefined;

  container.appendChild(entry.host);
  if (!entry.webgl) {
    let webgl: WebglAddon | null = null;
    try {
      webgl = new WebglAddon();
      const addon = webgl;
      addon.onContextLoss(() => {
        addon.dispose();
        if (entry.webgl === addon) entry.webgl = null; // falls back to the DOM renderer
      });
      entry.term.loadAddon(addon);
      entry.webgl = addon;
    } catch {
      webgl?.dispose();
      entry.webgl = null; // no WebGL: DOM renderer
    }
  }

  let frame = 0;
  const refit = () => {
    cancelAnimationFrame(frame);
    frame = requestAnimationFrame(() => {
      fitNow(entry);
      entry.term.refresh(0, Math.max(0, entry.term.rows - 1));
    });
  };
  const observer = new ResizeObserver(refit);
  observer.observe(container);
  refit();
  if (!isModalOpen(useStore.getState())) entry.term.focus();

  return () => {
    observer.disconnect();
    cancelAnimationFrame(frame);
    entry.webgl?.dispose();
    entry.webgl = null;
    if (registry.has(termId)) parkingLot().appendChild(entry.host);
  };
}

export interface FindOptions {
  caseSensitive: boolean;
  regex: boolean;
}

/** Search the scrollback. `incremental` keeps the current match while the query is being typed. */
export function findInTerminal(
  termId: string,
  query: string,
  options: FindOptions,
  direction: "next" | "previous",
  incremental = false,
): boolean {
  const entry = registry.get(termId);
  if (!entry || query === "") return false;
  const searchOptions = { ...options, incremental, decorations: currentSearchDecorations() };
  try {
    return direction === "next"
      ? entry.search.findNext(query, searchOptions)
      : entry.search.findPrevious(query, searchOptions);
  } catch {
    return false; // invalid regular expression
  }
}

export function clearFind(termId: string): void {
  registry.get(termId)?.search.clearDecorations();
}

export function terminalHasSelection(termId: string): boolean {
  return registry.get(termId)?.term.hasSelection() ?? false;
}

export function copyTerminalSelection(termId: string): void {
  const entry = registry.get(termId);
  if (entry) writeClipboard(entry.term.getSelection());
}

/** Context-menu paste: reads the clipboard and goes through the same sanitiser as the paste event. */
export async function pasteClipboardInto(termId: string): Promise<void> {
  const entry = registry.get(termId);
  if (!entry) return;
  pasteText(entry.pasteTarget, await navigator.clipboard.readText());
}

export function selectAllInTerminal(termId: string): void {
  registry.get(termId)?.term.selectAll();
}

export function clearTerminal(termId: string): void {
  registry.get(termId)?.term.clear();
}

export function focusTerminal(termId: string): void {
  registry.get(termId)?.term.focus();
}

/** The scrollback and the screen as plain text, soft-wrapped rows joined again (Terminal > Save output…). */
export function terminalText(termId: string): string {
  const buffer = registry.get(termId)?.term.buffer.active;
  if (!buffer) return "";
  const lines: string[] = [];
  for (let y = 0; y < buffer.length; y += 1) {
    const line = buffer.getLine(y);
    if (!line) continue;
    const text = line.translateToString(true);
    if (line.isWrapped && lines.length > 0) lines[lines.length - 1] += text;
    else lines.push(text);
  }
  while (lines.length > 0 && lines[lines.length - 1] === "") lines.pop();
  return lines.join("\n");
}

/** Closes the backend terminal (best effort) and disposes the xterm instance. */
export function disposeTerminal(termId: string, closeOnBackend = true): void {
  const entry = registry.get(termId);
  if (!entry) return;
  registry.delete(termId);
  sendQueues.delete(termId);
  if (entry.resizeTimer) clearTimeout(entry.resizeTimer);
  entry.resizeTimer = null;
  if (closeOnBackend) getBackend().termClose(termId).catch(() => undefined);
  for (const undo of entry.cleanup) undo();
  entry.cleanup.length = 0;
  entry.webgl?.dispose();
  entry.webgl = null;
  entry.term.dispose();
  entry.host.remove();
}

/** Disposes every terminal (app teardown, tests). */
export function disposeAllTerminals(closeOnBackend = false): void {
  for (const termId of [...registry.keys()]) disposeTerminal(termId, closeOnBackend);
}
