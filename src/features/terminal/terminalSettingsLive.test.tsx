// Terminal settings (ARCHITECTURE §8.11): live xterm options, mouse reporting, pointer hiding, paste warning, exit action,
// the folder of new terminals, the first terminal on connect, the sound bell.
import { act, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import { resetBeepForTests } from "../../lib/beep";
import { pasteText } from "../../lib/paste";
import { sshConfigTarget } from "../../lib/targets";
import type { Settings } from "../../ipc/types";
import { useStore } from "../../state/store";
import { setupApp } from "../../test/harness";
import { openSession } from "../session/sessionActions";
import { newTerminalStart, openTerminalTab } from "./terminalActions";
import { POINTER_HIDDEN_CLASS, disposeAllTerminals, getTerminal, handleTermEvent, type TerminalEntry } from "./terminalRegistry";

afterEach(() => {
  disposeAllTerminals();
  resetBeepForTests();
  setBackend(null);
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

async function connected() {
  const app = setupApp();
  const sessionId = await openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
  await waitFor(() => expect(useStore.getState().workspaces[sessionId]?.terminalTabs).toHaveLength(1), { timeout: 3000 });
  await screen.findByRole("region", { name: "Terminal panel" });
  const termId = useStore.getState().workspaces[sessionId]?.activeTerminal as string;
  return { ...app, sessionId, termId, entry: getTerminal(termId) as TerminalEntry };
}

function patchSettings(patch: (s: Settings) => Settings): void {
  const settings = useStore.getState().settings;
  if (!settings) throw new Error("no settings");
  act(() => useStore.getState().setSettings(patch(settings)));
}
const patchTerminal = (p: Partial<Settings["terminal"]>) => patchSettings((s) => ({ ...s, terminal: { ...s.terminal, ...p } }));
const write = (entry: TerminalEntry, data: string) => new Promise<void>((resolve) => entry.term.write(data, resolve));

describe("xterm options follow the settings, live", () => {
  it("letter spacing, minimum contrast, word separators and scroll speed reach open terminals", async () => {
    const { entry } = await connected();
    expect(entry.term.options.letterSpacing).toBe(0);
    expect(entry.term.options.minimumContrastRatio).toBe(1);
    expect(entry.term.options.scrollSensitivity).toBe(1);
    expect(entry.term.options.wordSeparator).toBe(" ()[]{}'\"");
    patchTerminal({ letterSpacing: 3, minimumContrastRatio: 4.5, wordSeparators: " /:", scrollSensitivity: 6 });
    expect(entry.term.options.letterSpacing).toBe(3);
    expect(entry.term.options.minimumContrastRatio).toBe(4.5);
    expect(entry.term.options.wordSeparator).toBe(" /:");
    expect(entry.term.options.scrollSensitivity).toBe(6);
  });

  it("Option-as-Meta applies on macOS only", async () => {
    const { entry } = await connected();
    patchTerminal({ altIsMeta: true });
    expect(entry.term.options.macOptionIsMeta).toBe(false); // Windows in the fixtures
    act(() => useStore.setState({ platform: "macos" }));
    patchTerminal({ altIsMeta: false });
    patchTerminal({ altIsMeta: true });
    expect(entry.term.options.macOptionIsMeta).toBe(true);
  });

  it("a scrollback of 1 000 000 lines is passed to xterm", async () => {
    const { entry } = await connected();
    patchTerminal({ scrollback: 1_000_000 });
    expect(entry.term.options.scrollback).toBe(1_000_000);
  });
});

describe("mouse reporting", () => {
  it("on: a program can switch mouse tracking on", async () => {
    const { entry } = await connected();
    await write(entry, "\u001b[?1000h");
    expect(entry.term.modes.mouseTrackingMode).toBe("vt200");
  });

  it("off: the mouse tracking modes are swallowed, other modes still work", async () => {
    const { entry } = await connected();
    patchTerminal({ mouseReporting: false });
    await write(entry, "\u001b[?1000h\u001b[?1002h\u001b[?1006h");
    expect(entry.term.modes.mouseTrackingMode).toBe("none");
    // a mixed sequence keeps the non-mouse mode (bracketed paste)
    await write(entry, "\u001b[?1000;2004h");
    await waitFor(() => expect(entry.term.modes.bracketedPasteMode).toBe(true));
    await waitFor(() => expect(entry.term.modes.mouseTrackingMode).toBe("none"));
  });

  it("off: a mixed sequence switches the alternate screen before the output that follows it", async () => {
    const { entry } = await connected();
    patchTerminal({ mouseReporting: false });
    await write(entry, "\u001b[?1049;1000hFULLSCREEN");
    expect(entry.term.buffer.active.type).toBe("alternate");
    const line = entry.term.buffer.active.getLine(entry.term.buffer.active.cursorY)?.translateToString(true) ?? "";
    expect(line).toContain("FULLSCREEN"); // painted on the alternate screen, not into the normal buffer
    await waitFor(() => expect(entry.term.modes.mouseTrackingMode).toBe("none"));
  });

  it("switching it off live resets a mode the program already set; switching it on obeys new requests", async () => {
    const { entry } = await connected();
    await write(entry, "\u001b[?1003h");
    expect(entry.term.modes.mouseTrackingMode).toBe("any");
    patchTerminal({ mouseReporting: false });
    await waitFor(() => expect(entry.term.modes.mouseTrackingMode).toBe("none"));
    patchTerminal({ mouseReporting: true });
    await write(entry, "\u001b[?1000h");
    expect(entry.term.modes.mouseTrackingMode).toBe("vt200");
  });
});

describe("hide the mouse pointer while typing", () => {
  it("typing hides it, moving the mouse shows it; off by default", async () => {
    const { entry } = await connected();
    const key = () => entry.host.dispatchEvent(new KeyboardEvent("keydown", { key: "a", bubbles: true }));
    key();
    expect(entry.host.classList.contains(POINTER_HIDDEN_CLASS)).toBe(false);
    patchTerminal({ hideMousePointerWhileTyping: true });
    entry.host.dispatchEvent(new KeyboardEvent("keydown", { key: "Shift", bubbles: true }));
    expect(entry.host.classList.contains(POINTER_HIDDEN_CLASS)).toBe(false); // a modifier alone is not typing
    key();
    expect(entry.host.classList.contains(POINTER_HIDDEN_CLASS)).toBe(true);
    entry.host.dispatchEvent(new MouseEvent("mousemove", { bubbles: true }));
    expect(entry.host.classList.contains(POINTER_HIDDEN_CLASS)).toBe(false);
  });
});

describe("multi-line paste warning", () => {
  it("asks by default; with the warning off the text is pasted at once", async () => {
    const { entry } = await connected();
    // Outside bracketed-paste mode a line break would run the line at once: that is what the warning is about.
    await write(entry, "\u001b[?2004l");
    const paste = vi.spyOn(entry.term, "paste");
    pasteText(entry.pasteTarget, "echo a\necho b");
    await screen.findByRole("alertdialog", { name: "Paste 2 lines?" });
    expect(paste).not.toHaveBeenCalled();
    act(() => useStore.getState().closeDialog());

    patchTerminal({ multilinePasteWarning: false });
    pasteText(entry.pasteTarget, "echo a\necho b");
    expect(paste).toHaveBeenCalledWith("echo a\necho b");
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });
});

describe("what happens when the shell ends", () => {
  const exit = (entry: TerminalEntry, code: number | null) => handleTermEvent(entry, { type: "exit", code, signal: null });

  it("keep (default): the tab stays and says how it ended", async () => {
    const { entry, sessionId, termId } = await connected();
    exit(entry, 0);
    expect(useStore.getState().terminals[termId]?.status).toBe("exited");
    expect(useStore.getState().workspaces[sessionId]?.terminalTabs).toContain(termId);
  });

  it("closeOnSuccess: exit code 0 closes the tab, another code keeps it", async () => {
    const { entry, sessionId, termId } = await connected();
    patchTerminal({ onExit: "closeOnSuccess" });
    exit(entry, 2);
    expect(useStore.getState().workspaces[sessionId]?.terminalTabs).toContain(termId);
    exit(entry, 0);
    expect(useStore.getState().workspaces[sessionId]?.terminalTabs).not.toContain(termId);
    expect(getTerminal(termId)).toBeUndefined();
    expect(useStore.getState().terminals[termId]).toBeUndefined();
  });

  it("close: any exit closes the tab, also one without a code (killed by a signal)", async () => {
    const { entry, sessionId, termId } = await connected();
    patchTerminal({ onExit: "close" });
    exit(entry, null);
    expect(useStore.getState().workspaces[sessionId]?.terminalTabs).not.toContain(termId);
    expect(getTerminal(termId)).toBeUndefined();
  });
});

describe("where a new terminal starts", () => {
  it("home (default): no folder is sent", async () => {
    const { backend, sessionId } = await connected();
    await openTerminalTab(sessionId);
    expect(backend.terminalOpens.at(-1)).toMatchObject({ cwd: null, cwdFallback: false });
  });

  it("currentFolder: the tree's folder is sent with the fallback flag", async () => {
    const { backend, sessionId } = await connected();
    patchTerminal({ newTerminalCwd: "currentFolder" });
    const root = useStore.getState().workspaces[sessionId]?.tree.root;
    expect(root).toBeTruthy();
    expect(newTerminalStart(sessionId)).toEqual({ cwd: root, fallback: true });
    await openTerminalTab(sessionId);
    expect(backend.terminalOpens.at(-1)).toMatchObject({ cwd: root, cwdFallback: true });
  });

  it("'Open terminal here' stays strict (no fallback) whatever the setting says", async () => {
    const { backend, sessionId } = await connected();
    patchTerminal({ newTerminalCwd: "currentFolder" });
    await openTerminalTab(sessionId, "/var/log");
    expect(backend.terminalOpens.at(-1)).toMatchObject({ cwd: "/var/log", cwdFallback: false });
  });

  it("the TERM type is the setting at open time", async () => {
    const { backend, sessionId } = await connected();
    expect(backend.terminalOpens.at(-1)?.termType).toBe("xterm-256color");
    const settings = useStore.getState().settings as Settings;
    const saved = await backend.settingsUpdate({ ...settings, terminal: { ...settings.terminal, termType: "vt100" } });
    act(() => useStore.getState().setSettings(saved));
    await openTerminalTab(sessionId);
    expect(backend.terminalOpens.at(-1)?.termType).toBe("vt100");
  });
});

describe("open a terminal on connect", () => {
  it("off: the connection opens with the file tree only", async () => {
    setupApp();
    patchTerminal({ openTerminalOnConnect: false });
    const sessionId = await openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
    await waitFor(() => expect(useStore.getState().sessionsById[sessionId]?.state.phase).toBe("connected"));
    await screen.findByRole("treeitem", { name: /docker-compose\.yml/ });
    await new Promise((r) => setTimeout(r, 150));
    expect(useStore.getState().workspaces[sessionId]?.terminalTabs).toEqual([]);
    expect(screen.getByText("No terminal is open in this session.")).toBeTruthy();
  });
});

describe("bell: sound", () => {
  it("rings a Web Audio beep; off and visual stay silent", async () => {
    const start = vi.fn();
    class FakeAudio {
      state = "running";
      currentTime = 0;
      destination = {};
      createOscillator = () => ({ type: "", frequency: { value: 0 }, connect: vi.fn(), start, stop: vi.fn() });
      createGain = () => ({ gain: { setValueAtTime: vi.fn(), linearRampToValueAtTime: vi.fn() }, connect: vi.fn() });
      resume = () => Promise.resolve();
    }
    vi.stubGlobal("AudioContext", FakeAudio);
    const { entry } = await connected();
    await write(entry, "\u0007");
    expect(start).not.toHaveBeenCalled();
    patchTerminal({ bell: "visual" });
    await write(entry, "\u0007");
    expect(start).not.toHaveBeenCalled();
    patchTerminal({ bell: "sound" });
    await write(entry, "\u0007");
    expect(start).toHaveBeenCalledOnce();
  });
});
