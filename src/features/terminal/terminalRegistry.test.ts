import { act } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import { makeSettings } from "../../ipc/mock/fixtures";
import { MockBackend } from "../../ipc/mock/mockBackend";
import type { RecentTarget } from "../../ipc/types";
import { CAN_SGR_RESET } from "../../lib/ansi";
import { resetStore, useStore } from "../../state/store";
import { answerPasteConfirm } from "./pasteConfirm";
import {
  attachTerminal,
  createTerminal,
  disposeTerminal,
  getTerminal,
  handleMessage,
  handleTermEvent,
  linksForRow,
  openLink,
  PASTE_FALLBACK_MS,
  terminalCount,
  type TerminalEntry,
} from "./terminalRegistry";
import { closeTerminalTab, openTerminalTab } from "./terminalActions";
import { closeSession } from "../session/sessionActions";

const ESC = String.fromCharCode(0x1b);
const encoder = new TextEncoder();
const decoder = new TextDecoder();
const tick = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

/** Resolves once xterm has processed everything written so far. */
function drained(entry: TerminalEntry): Promise<void> {
  return new Promise((resolve) => entry.term.write("", resolve));
}

function screen(entry: TerminalEntry): string {
  const buffer = entry.term.buffer.active;
  const lines: string[] = [];
  for (let i = 0; i < buffer.length; i++) lines.push(buffer.getLine(i)?.translateToString(true) ?? "");
  return lines.join("\n");
}

async function connect(backend: MockBackend, alias: string): Promise<string> {
  const target: RecentTarget = { kind: "sshConfig", alias };
  const sessionId = await backend.openSession({ target, probeOnly: false }, () => undefined);
  useStore.getState().addSession(sessionId, target, alias);
  useStore.getState().ensureWorkspace(sessionId);
  return sessionId;
}

/** Everything sent to the remote via term_write, decoded. */
function sentInput(spy: { mock: { calls: unknown[][] } }): string {
  return spy.mock.calls.map((c: unknown[]) => decoder.decode(Uint8Array.from(c[1] as number[]))).join("");
}

function keyDown(target: Element, init: KeyboardEventInit & { keyCode: number }): KeyboardEvent {
  const event = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
  target.dispatchEvent(event);
  return event;
}

describe("terminal registry", () => {
  beforeEach(() => resetStore());
  afterEach(() => setBackend(null));

  it("outputDropped writes CAN + SGR reset first, then the localized dim line (§4.5)", () => {
    const write = vi.fn();
    const entry = { termId: "t", term: { write } } as unknown as TerminalEntry;
    handleTermEvent(entry, { type: "outputDropped", bytes: 2048 });
    expect(write).toHaveBeenCalledTimes(1);
    const out = String(write.mock.calls[0]?.[0]);
    expect(out.startsWith(CAN_SGR_RESET)).toBe(true);
    expect(out).toContain("2 KiB of output discarded");
  });

  it("does not leak the xterm host when term_open fails", async () => {
    const backend = new MockBackend();
    useStore.getState().addSession("s1", { kind: "sshConfig", alias: "a" }, "a");
    setBackend(backend);
    const before = document.querySelectorAll(".terminal-host").length;
    await expect(createTerminal("s1")).rejects.toMatchObject({ error: { code: "notConnected" } });
    expect(document.querySelectorAll(".terminal-host").length).toBe(before);
  });

  it("cleans up when the session was closed while term_open was pending", async () => {
    const backend = new MockBackend({ latencyMs: 20 });
    setBackend(backend);
    const sessionId = await backend.openSession({ target: { kind: "sshConfig", alias: "prod-web-01" }, probeOnly: false }, () => undefined);
    useStore.getState().addSession(sessionId, { kind: "sshConfig", alias: "prod-web-01" }, "p");
    const pending = createTerminal(sessionId);
    useStore.getState().removeSession(sessionId);
    expect(await pending).toBeNull();
    expect(document.querySelectorAll(".terminal-host").length).toBe(0);
    expect(terminalCount()).toBe(0);
  });
});

describe("terminal registry — lifecycle and scrollback", () => {
  let backend: MockBackend;
  beforeEach(() => {
    resetStore();
    backend = new MockBackend();
    setBackend(backend);
  });
  afterEach(() => setBackend(null));

  it("keeps scrollback across detach/attach and while parked (tab switches)", async () => {
    const sessionId = await connect(backend, "prod-web-01");
    const termId = (await openTerminalTab(sessionId)) as string;
    const entry = getTerminal(termId) as TerminalEntry;
    await tick(5);

    const first = document.createElement("div");
    document.body.appendChild(first);
    const detach = attachTerminal(termId, first);
    expect(first.contains(entry.host)).toBe(true);

    handleMessage(entry, encoder.encode(Array.from({ length: 300 }, (_, i) => `line-${i}`).join("\r\n") + "\r\n").buffer);
    await drained(entry);
    expect(screen(entry)).toContain("line-0");
    expect(screen(entry)).toContain("line-299");

    detach(); // parked: hidden, but still alive and still receiving output
    expect(first.contains(entry.host)).toBe(false);
    expect(entry.host.parentElement?.hidden).toBe(true);
    handleMessage(entry, encoder.encode("while-parked\r\n").buffer);
    await drained(entry);

    const second = document.createElement("div");
    document.body.appendChild(second);
    const detach2 = attachTerminal(termId, second);
    expect(second.contains(entry.host)).toBe(true);
    expect(getTerminal(termId)).toBe(entry); // the same instance
    expect(screen(entry)).toContain("line-0");
    expect(screen(entry)).toContain("while-parked");
    detach2();
    first.remove();
    second.remove();
    closeTerminalTab(sessionId, termId);
  });

  it("10 terminals across 3 sessions work independently and are all released on close", async () => {
    const sessions = [
      await connect(backend, "prod-web-01"),
      await connect(backend, "staging-db"),
      await connect(backend, "bastion"),
    ];
    const counts = [4, 3, 3];
    const ids: string[] = [];
    for (const [i, sessionId] of sessions.entries()) {
      for (let n = 0; n < (counts[i] ?? 0); n++) ids.push((await openTerminalTab(sessionId)) as string);
    }
    expect(ids).toHaveLength(10);
    expect(new Set(ids).size).toBe(10);
    expect(terminalCount()).toBe(10);
    await tick(10);

    // Each terminal has its own shell: its own prompt, its own history.
    for (const [i, termId] of ids.entries()) {
      const entry = getTerminal(termId) as TerminalEntry;
      entry.term.input(`echo marker-${i}\r`);
    }
    await tick(10);
    for (const [i, termId] of ids.entries()) {
      const entry = getTerminal(termId) as TerminalEntry;
      await drained(entry);
      const text = screen(entry);
      expect(text, termId).toContain(`marker-${i}`);
      for (let other = 0; other < ids.length; other++) {
        if (other !== i) expect(text, `${termId} must not see output of terminal ${other}`).not.toContain(`marker-${other}`);
      }
    }
    // Switching around keeps every instance.
    const container = document.createElement("div");
    document.body.appendChild(container);
    for (const termId of ids) attachTerminal(termId, container)();
    expect(terminalCount()).toBe(10);

    await closeSession(sessions[0] as string);
    expect(terminalCount()).toBe(6);
    await closeSession(sessions[1] as string);
    await closeSession(sessions[2] as string);
    expect(terminalCount()).toBe(0);
    expect(document.querySelectorAll(".terminal-host").length).toBe(0);
    container.remove();
  });

  it("closing a tab releases everything: instance, DOM node, store entry, pending resize, backend terminal (no leaks)", async () => {
    const sessionId = await connect(backend, "prod-web-01");
    const termId = (await openTerminalTab(sessionId)) as string;
    const entry = getTerminal(termId) as TerminalEntry;
    const termClose = vi.spyOn(backend, "termClose");
    const termResize = vi.spyOn(backend, "termResize");
    entry.term.resize(120, 30); // schedules a debounced term_resize
    expect(entry.resizeTimer).not.toBeNull();

    closeTerminalTab(sessionId, termId);
    expect(getTerminal(termId)).toBeUndefined();
    expect(terminalCount()).toBe(0);
    expect(entry.resizeTimer).toBeNull();
    expect(entry.host.isConnected).toBe(false);
    expect(termClose).toHaveBeenCalledWith(termId);
    expect(useStore.getState().terminals[termId]).toBeUndefined();
    expect(useStore.getState().workspaces[sessionId]?.terminalTabs).toEqual([]);
    await tick(80);
    expect(termResize).not.toHaveBeenCalled(); // the debounce timer was cleared
    disposeTerminal(termId); // idempotent
  });

  it("debounces term_resize and only sends real size changes (§4.5)", async () => {
    const sessionId = await connect(backend, "prod-web-01");
    const termId = (await openTerminalTab(sessionId)) as string;
    const entry = getTerminal(termId) as TerminalEntry;
    const termResize = vi.spyOn(backend, "termResize");
    entry.term.resize(100, 30);
    entry.term.resize(110, 31);
    entry.term.resize(120, 32);
    await tick(80);
    expect(termResize).toHaveBeenCalledTimes(1);
    expect(termResize.mock.calls[0]?.[1]).toMatchObject({ cols: 120, rows: 32 });
    entry.term.resize(80, 24); // back to the open size after 120x32 was sent: a change
    await tick(80);
    expect(termResize).toHaveBeenCalledTimes(2);
    closeTerminalTab(sessionId, termId);
  });
});

describe("terminal registry — output flow (acks, outputDropped, flood)", () => {
  beforeEach(() => resetStore());
  afterEach(() => setBackend(null));

  it("acknowledges processed bytes in batches of at least 256 KiB, never losing a byte", async () => {
    const backend = new MockBackend();
    setBackend(backend);
    const sessionId = await connect(backend, "prod-web-01");
    const termId = (await openTerminalTab(sessionId)) as string;
    const entry = getTerminal(termId) as TerminalEntry;
    await tick(5);
    await drained(entry);
    const ack = vi.spyOn(backend, "termAck");
    const sizeOfPrompt = entry.acks.unacked;

    const chunk = new Uint8Array(100 * 1024).fill(0x61).buffer;
    for (let i = 0; i < 2; i++) handleMessage(entry, chunk.slice(0));
    await drained(entry);
    expect(ack).not.toHaveBeenCalled(); // 200 KiB < 256 KiB
    handleMessage(entry, chunk.slice(0));
    await drained(entry);
    expect(ack).toHaveBeenCalledTimes(1);
    expect(ack.mock.calls[0]?.[1]).toBe(sizeOfPrompt + 300 * 1024);
    expect(entry.acks.unacked).toBe(0);
    closeTerminalTab(sessionId, termId);
  });

  it("terminalFlood: the backlog overflows, acks keep it moving, outputDropped writes CAN + SGR reset then the notice", async () => {
    const backend = new MockBackend({ scenario: "terminalFlood", floodBytes: 14 * 1024 * 1024 });
    setBackend(backend);
    const sessionId = await connect(backend, "prod-web-01");
    const ack = vi.spyOn(backend, "termAck");
    const termId = (await openTerminalTab(sessionId)) as string;
    const entry = getTerminal(termId) as TerminalEntry;
    const write = vi.spyOn(entry.term, "write");

    await vi.waitFor(
      () => {
        const dropped = write.mock.calls.find((c) => typeof c[0] === "string" && c[0].includes("of output discarded"));
        expect(dropped).toBeTruthy();
      },
      { timeout: 30_000, interval: 50 },
    );
    const dropCall = write.mock.calls.find((c) => typeof c[0] === "string" && c[0].includes("of output discarded"));
    expect(String(dropCall?.[0]).startsWith(CAN_SGR_RESET)).toBe(true);
    // 14 MiB were produced; 4 MiB window + 8 MiB backlog fit, the remaining ~2 MiB were discarded.
    expect(String(dropCall?.[0])).toMatch(/2(\.\d)? MiB of output discarded/);
    await drained(entry);
    const acked = ack.mock.calls.reduce((sum, c) => sum + (c[1] as number), 0);
    expect(acked).toBeGreaterThan(10 * 1024 * 1024); // the flow never stalled
    expect(useStore.getState().terminals[termId]?.status).toBe("running");
    closeTerminalTab(sessionId, termId);
  }, 60_000);
});

describe("terminal registry — input, paste, shortcuts", () => {
  let backend: MockBackend;
  let termId: string;
  let entry: TerminalEntry;
  let write: ReturnType<typeof vi.spyOn>;
  beforeEach(async () => {
    resetStore();
    backend = new MockBackend();
    setBackend(backend);
    const sessionId = await connect(backend, "prod-web-01");
    termId = (await openTerminalTab(sessionId)) as string;
    entry = getTerminal(termId) as TerminalEntry;
    await tick(5);
    await drained(entry); // the shell enabled bracketed-paste mode
    write = vi.spyOn(backend, "termWrite");
  });
  afterEach(() => {
    disposeTerminal(termId);
    setBackend(null);
  });

  function pasteOnTextarea(text: string): Event {
    const event = new Event("paste", { bubbles: true, cancelable: true });
    Object.defineProperty(event, "clipboardData", { value: { getData: () => text } });
    entry.term.textarea?.dispatchEvent(event);
    return event;
  }

  it("a paste event on xterm's textarea is captured ONCE and sanitised (single capture-phase listener)", async () => {
    expect(entry.term.modes.bracketedPasteMode).toBe(true);
    const textareaListener = vi.fn();
    entry.term.textarea?.addEventListener("paste", textareaListener); // stands in for xterm's own handler
    const event = pasteOnTextarea(`echo hi${ESC}[201~; echo pwned`);
    expect(event.defaultPrevented).toBe(true);
    expect(textareaListener).not.toHaveBeenCalled(); // never reached xterm's handlers
    await vi.waitFor(() => expect(write).toHaveBeenCalled());
    // One bracketed paste whose payload has no ESC of its own: the end marker appears exactly once, at the end.
    expect(sentInput(write)).toBe(`${ESC}[200~echo hi[201~; echo pwned${ESC}[201~`);
    expect(write.mock.calls.length).toBe(1);
  });

  it("asks before pasting multi-line text when bracketed-paste mode is off, and honours Cancel/Paste", async () => {
    entry.term.write(`${ESC}[?2004l`);
    await drained(entry);
    pasteOnTextarea("ls\nwhoami\n");
    expect(useStore.getState().dialog).toMatchObject({ kind: "confirmPaste", lines: 2 });
    expect(write).not.toHaveBeenCalled();
    act(() => answerPasteConfirm(false));
    await tick(5);
    expect(write).not.toHaveBeenCalled();
    expect(useStore.getState().dialog).toBeNull();

    pasteOnTextarea("ls\nwhoami\n");
    act(() => answerPasteConfirm(true));
    await vi.waitFor(() => expect(write).toHaveBeenCalled());
    expect(sentInput(write)).toBe("ls\rwhoami\r"); // xterm converts line breaks to CR
  });

  it("single-line pastes outside bracketed mode go straight through", async () => {
    entry.term.write(`${ESC}[?2004l`);
    await drained(entry);
    pasteOnTextarea("pwd");
    await vi.waitFor(() => expect(write).toHaveBeenCalled());
    expect(sentInput(write)).toBe("pwd");
    expect(useStore.getState().dialog).toBeNull();
  });

  it("plain Ctrl+K reaches the shell when the terminal is focused (§8.7)", async () => {
    const textarea = entry.term.textarea as HTMLTextAreaElement;
    const event = keyDown(textarea, { key: "k", code: "KeyK", keyCode: 75, ctrlKey: true });
    await vi.waitFor(() => expect(write).toHaveBeenCalled());
    expect(sentInput(write)).toBe("\x0b");
    expect(event.defaultPrevented).toBe(true); // consumed by xterm, i.e. NOT left to the app's palette shortcut
  });

  it("Ctrl+Shift+K is left to the app (command palette) instead of being sent", async () => {
    const textarea = entry.term.textarea as HTMLTextAreaElement;
    keyDown(textarea, { key: "K", code: "KeyK", keyCode: 75, ctrlKey: true, shiftKey: true });
    await tick(5);
    expect(write).not.toHaveBeenCalled();
  });

  it("Ctrl+C and Ctrl+V go to the remote; Ctrl+Shift+V is left to the browser's paste event", async () => {
    const textarea = entry.term.textarea as HTMLTextAreaElement;
    keyDown(textarea, { key: "c", code: "KeyC", keyCode: 67, ctrlKey: true });
    const paste = keyDown(textarea, { key: "V", code: "KeyV", keyCode: 86, ctrlKey: true, shiftKey: true });
    await vi.waitFor(() => expect(write).toHaveBeenCalled());
    expect(sentInput(write)).toBe("\x03");
    expect(paste.defaultPrevented).toBe(false); // the default action (the paste event) must run
  });

  it("Ctrl+Shift+C copies the selection", async () => {
    const writeText = vi.fn(async () => undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    entry.term.select(0, 0, 5);
    const event = keyDown(entry.term.textarea as HTMLTextAreaElement, {
      key: "C", code: "KeyC", keyCode: 67, ctrlKey: true, shiftKey: true,
    });
    expect(event.defaultPrevented).toBe(true);
    expect(writeText).toHaveBeenCalledWith(entry.term.getSelection());
    expect(sentInput(write)).toBe("");
  });

  it("OSC title changes land in the store (rendered through displayServerText)", async () => {
    entry.term.write(`${ESC}]0;build ${ESC}[31mred\x07`);
    await drained(entry);
    // The store is patched on the next animation frame.
    await vi.waitFor(() => expect(useStore.getState().terminals[termId]?.title).toBe("build "));
    entry.term.write(`${ESC}]0;ssh: deploy@prod\x07`);
    await drained(entry);
    await vi.waitFor(() => expect(useStore.getState().terminals[termId]?.title).toBe("ssh: deploy@prod"));
  });

  it("A flood of OSC titles patches the store once, with the last title", async () => {
    const seen: Array<string | null | undefined> = [];
    const unsubscribe = useStore.subscribe((s) => {
      const title = s.terminals[termId]?.title;
      if (seen[seen.length - 1] !== title) seen.push(title);
    });
    entry.term.write(Array.from({ length: 500 }, (_, i) => `${ESC}]0;t${i}\x07`).join(""));
    await drained(entry);
    await vi.waitFor(() => expect(useStore.getState().terminals[termId]?.title).toBe("t499"));
    unsubscribe();
    expect(seen).toEqual(["t499"]);
  });

  it("ScreenReaderMode follows the setting, also for an open terminal", async () => {
    expect(entry.term.options.screenReaderMode).toBe(false);
    const settings = makeSettings();
    act(() => useStore.getState().setSettings({ ...settings, terminal: { ...settings.terminal, screenReaderMode: true } }));
    expect(entry.term.options.screenReaderMode).toBe(true);
    act(() => useStore.getState().setSettings({ ...settings, terminal: { ...settings.terminal, screenReaderMode: false } }));
    expect(entry.term.options.screenReaderMode).toBe(false);
  });

  describe("Ctrl+Shift+V on Linux", () => {
    let readText: ReturnType<typeof vi.fn>;
    beforeEach(() => {
      useStore.setState({ platform: "linux" });
      readText = vi.fn(async () => "pasted-by-fallback");
      Object.defineProperty(navigator, "clipboard", { value: { readText, writeText: vi.fn() }, configurable: true });
    });

    const pasteChord = () =>
      keyDown(entry.term.textarea as HTMLTextAreaElement, { key: "V", code: "KeyV", keyCode: 86, ctrlKey: true, shiftKey: true });

    it("reads the clipboard itself when the webview fires no paste event", async () => {
      const event = pasteChord();
      expect(event.defaultPrevented).toBe(false); // the native path stays available
      await vi.waitFor(() => expect(readText).toHaveBeenCalledTimes(1), { timeout: 1000 });
      await vi.waitFor(() => expect(sentInput(write)).toContain("pasted-by-fallback"));
    });

    it("does not paste twice when the native paste event does arrive", async () => {
      pasteChord();
      pasteOnTextarea("native-paste");
      await tick(PASTE_FALLBACK_MS + 80);
      expect(readText).not.toHaveBeenCalled();
      expect(sentInput(write)).toContain("native-paste");
    });

    it("swallows a native paste event that arrives after the fallback already pasted", async () => {
      pasteChord();
      await vi.waitFor(() => expect(sentInput(write)).toContain("pasted-by-fallback"), { timeout: 1000 });
      const late = pasteOnTextarea("late-native");
      expect(late.defaultPrevented).toBe(true);
      await tick(20);
      expect(sentInput(write)).not.toContain("late-native");
      expect(sentInput(write).split("pasted-by-fallback").length - 1).toBe(1);
      // An unrelated paste later on goes through as usual.
      pasteOnTextarea("next-paste");
      await vi.waitFor(() => expect(sentInput(write)).toContain("next-paste"));
    });

    it("is not used on Windows", async () => {
      useStore.setState({ platform: "windows" });
      pasteChord();
      await tick(PASTE_FALLBACK_MS + 80);
      expect(readText).not.toHaveBeenCalled();
    });
  });

  it("Input chunks go out one at a time, in order, and an error does not stall or drop the rest", async () => {
    let inFlight = 0;
    let maxInFlight = 0;
    const received: number[][] = [];
    let call = 0;
    write.mockImplementation(async (_id: string, data: Uint8Array | readonly number[]) => {
      inFlight += 1;
      maxInFlight = Math.max(maxInFlight, inFlight);
      const n = ++call;
      await tick(n === 1 ? 20 : 1); // the first (largest) call is the slowest, so a racing sender would overtake it
      inFlight -= 1;
      received.push(Array.from(data));
      if (n === 2) throw new Error("notConnected");
    });
    const big = "a".repeat(150 * 1024); // three chunks: 64 KiB, 64 KiB, 22 KiB
    entry.term.input(big);
    entry.term.input("b");
    entry.term.input("c");
    await vi.waitFor(() => expect(received.reduce((n, d) => n + d.length, 0)).toBe(big.length + 2), { timeout: 3000 });
    expect(maxInFlight).toBe(1);
    const all = received.flat();
    expect(all.slice(0, big.length).every((b) => b === 97)).toBe(true);
    expect(all.slice(big.length)).toEqual([98, 99]); // b, c after the paste, in order
    expect(received.every((d) => d.length <= 64 * 1024)).toBe(true);
  });

  it("Disposing the terminal drops what is still queued", async () => {
    let release: () => void = () => undefined;
    write.mockImplementation(() => new Promise<void>((resolve) => (release = resolve)));
    entry.term.input("x");
    entry.term.input("y");
    entry.term.input("z");
    expect(write).toHaveBeenCalledTimes(1);
    disposeTerminal(termId);
    release();
    await tick(10);
    expect(write).toHaveBeenCalledTimes(1);
  });

  it("Messages that arrive after disposeTerminal never reach the disposed xterm", async () => {
    const spy = vi.spyOn(entry.term, "write");
    disposeTerminal(termId);
    handleMessage(entry, encoder.encode("late output").buffer);
    handleMessage(entry, { type: "exit", code: 0, signal: null });
    expect(spy).not.toHaveBeenCalled();
  });

  it("never offers OSC 52 clipboard writes (addon not loaded)", async () => {
    const writeText = vi.fn(async () => undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    entry.term.write(`${ESC}]52;c;${btoa("secret")}\x07`);
    await drained(entry);
    expect(writeText).not.toHaveBeenCalled();
  });
});

describe("terminal registry — links", () => {
  beforeEach(() => resetStore());
  afterEach(() => setBackend(null));

  const event = (init: MouseEventInit) => new MouseEvent("click", init);

  it("opens a link only on Ctrl/Cmd+click, only http(s), through the backend's native confirm", () => {
    const backend = new MockBackend();
    setBackend(backend);
    const open = vi.spyOn(backend, "appOpenExternal").mockResolvedValue(true);
    openLink(event({}), "https://example.com/");
    expect(open).not.toHaveBeenCalled();
    openLink(event({ ctrlKey: true }), "javascript:alert(1)");
    openLink(event({ ctrlKey: true }), "file:///etc/passwd");
    openLink(event({ metaKey: true }), "https://user:pw@example.com/");
    expect(open).not.toHaveBeenCalled();
    openLink(event({ ctrlKey: true }), "https://example.com/docs");
    openLink(event({ metaKey: true }), "http://example.org/");
    expect(open).toHaveBeenCalledTimes(2);
    expect(open).toHaveBeenNthCalledWith(1, "https://example.com/docs", "Open this link in your browser?", expect.stringContaining("terminal output"));
  });

  it("provides clickable ranges for plain-text URLs, also after wide characters", async () => {
    const backend = new MockBackend();
    setBackend(backend);
    const open = vi.spyOn(backend, "appOpenExternal").mockResolvedValue(true);
    const sessionId = await connect(backend, "prod-web-01");
    const termId = (await openTerminalTab(sessionId)) as string;
    const entry = getTerminal(termId) as TerminalEntry;
    entry.term.write(`${ESC}[2J${ESC}[H日本 https://example.com/a?b=1, thanks\r\nno link here\r\n`);
    await drained(entry);
    const links = linksForRow(entry.term, 1);
    expect(links).toHaveLength(1);
    expect(links[0]?.text).toBe("https://example.com/a?b=1");
    // "日本" occupies 4 cells, then a space: the URL starts at column 6 and is 25 cells long.
    expect(links[0]?.range).toEqual({ start: { x: 6, y: 1 }, end: { x: 30, y: 1 } });
    expect(linksForRow(entry.term, 2)).toEqual([]);
    links[0]?.activate(new MouseEvent("click"), "https://example.com/a?b=1");
    expect(open).not.toHaveBeenCalled(); // plain click does nothing
    links[0]?.activate(new MouseEvent("click", { ctrlKey: true }), "https://example.com/a?b=1");
    expect(open).toHaveBeenCalledTimes(1);
    closeTerminalTab(sessionId, termId);
  });
});

describe("terminal registry — exit, restart, reconnected", () => {
  let backend: MockBackend;
  beforeEach(() => {
    resetStore();
    backend = new MockBackend();
    setBackend(backend);
  });
  afterEach(() => setBackend(null));

  it("exit shows the localized line; only Enter restarts the shell; nothing else is sent", async () => {
    const sessionId = await connect(backend, "prod-web-01");
    const termId = (await openTerminalTab(sessionId)) as string;
    const entry = getTerminal(termId) as TerminalEntry;
    await tick(5);
    entry.term.input("exit 3\r");
    await vi.waitFor(() => expect(useStore.getState().terminals[termId]?.status).toBe("exited"));
    await drained(entry);
    expect(useStore.getState().terminals[termId]?.exitCode).toBe(3);
    expect(screen(entry)).toContain("[Process exited with code 3] Press Enter to restart");

    const write = vi.spyOn(backend, "termWrite");
    const restart = vi.spyOn(backend, "termRestart");
    entry.term.input("x");
    await tick(5);
    expect(write).not.toHaveBeenCalled();
    expect(restart).not.toHaveBeenCalled();

    entry.term.input("\r");
    await vi.waitFor(() => expect(useStore.getState().terminals[termId]?.status).toBe("running"));
    expect(restart).toHaveBeenCalledTimes(1);
    await tick(5);
    await drained(entry);
    expect(screen(entry).match(/deploy@prod-web-01/g)?.length).toBeGreaterThanOrEqual(2); // a fresh prompt
    closeTerminalTab(sessionId, termId);
  });

  it("closed (channel closed without exit status) and reconnected events update status and write localized lines", async () => {
    const sessionId = await connect(backend, "prod-web-01");
    const termId = (await openTerminalTab(sessionId)) as string;
    const entry = getTerminal(termId) as TerminalEntry;
    handleTermEvent(entry, { type: "closed", error: null });
    await drained(entry);
    expect(useStore.getState().terminals[termId]?.status).toBe("closed");
    expect(screen(entry)).toContain("[Terminal closed]");
    handleTermEvent(entry, { type: "reconnected" });
    await drained(entry);
    expect(useStore.getState().terminals[termId]?.status).toBe("running");
    expect(screen(entry)).toContain("[Reconnected]");
    closeTerminalTab(sessionId, termId);
  });

  it("a closed event with an error shows the localized error, control characters escaped", async () => {
    const sessionId = await connect(backend, "prod-web-01");
    const termId = (await openTerminalTab(sessionId)) as string;
    const entry = getTerminal(termId) as TerminalEntry;
    handleTermEvent(entry, { type: "closed", error: { code: "disconnected", params: { reason: `bye${ESC}[2J` } } as never });
    await drained(entry);
    expect(screen(entry)).toContain("[Terminal closed:");
    expect(screen(entry)).not.toContain(`${ESC}[2J`);
    closeTerminalTab(sessionId, termId);
  });
});
