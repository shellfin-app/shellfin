import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import type { TermMessage } from "../../ipc";
import { MockBackend } from "../../ipc/mock/mockBackend";
import type { RecentTarget } from "../../ipc/types";
import { resetStore, useStore } from "../../state/store";
import { closeTerminalTab, openTerminalTab } from "./terminalActions";
import { disposeAllTerminals, getTerminal, type TerminalEntry } from "./terminalRegistry";

const tick = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

async function connect(backend: MockBackend): Promise<string> {
  const target: RecentTarget = { kind: "sshConfig", alias: "prod-web-01" };
  const sessionId = await backend.openSession({ target, probeOnly: false }, () => undefined);
  useStore.getState().addSession(sessionId, target, "prod-web-01");
  useStore.getState().ensureWorkspace(sessionId);
  return sessionId;
}

describe("terminal end-event races", () => {
  let backend: MockBackend;
  let channel: ((m: TermMessage) => void) | null;
  let openOriginal: MockBackend["openTerminal"];
  beforeEach(() => {
    resetStore();
    backend = new MockBackend();
    setBackend(backend);
    channel = null;
    openOriginal = backend.openTerminal.bind(backend);
    vi.spyOn(backend, "openTerminal").mockImplementation((sessionId, size, cwd, cb) => {
      channel = cb;
      return openOriginal(sessionId, size, cwd, cb);
    });
  });
  afterEach(() => {
    disposeAllTerminals();
    setBackend(null);
    vi.restoreAllMocks();
  });

  it("an `exit` buffered before term_open resolves is not lost: the tab ends up exited and Enter restarts", async () => {
    vi.spyOn(backend, "openTerminal").mockImplementation(async (sessionId, size, cwd, cb) => {
      cb({ type: "exit", code: 1, signal: null }); // arrives before the invoke response
      return openOriginal(sessionId, size, cwd, cb);
    });
    const sessionId = await connect(backend);
    const termId = (await openTerminalTab(sessionId)) as string;
    expect(useStore.getState().terminals[termId]).toMatchObject({ status: "exited", exitCode: 1 });

    const restart = vi.spyOn(backend, "termRestart");
    const write = vi.spyOn(backend, "termWrite");
    (getTerminal(termId) as TerminalEntry).term.input("\r");
    await vi.waitFor(() => expect(useStore.getState().terminals[termId]?.status).toBe("running"));
    expect(restart).toHaveBeenCalledTimes(1);
    expect(write).not.toHaveBeenCalled(); // Enter was a restart, not input to a dead channel
    closeTerminalTab(sessionId, termId);
  });

  it("a buffered `closed` event is kept, too", async () => {
    vi.spyOn(backend, "openTerminal").mockImplementation(async (sessionId, size, cwd, cb) => {
      cb({ type: "closed", error: null });
      return openOriginal(sessionId, size, cwd, cb);
    });
    const sessionId = await connect(backend);
    const termId = (await openTerminalTab(sessionId)) as string;
    expect(useStore.getState().terminals[termId]?.status).toBe("closed");
  });

  it("the term_restart response does not overwrite an exit that overtook it", async () => {
    const sessionId = await connect(backend);
    const termId = (await openTerminalTab(sessionId)) as string;
    const entry = getTerminal(termId) as TerminalEntry;
    await tick(5);
    entry.term.input("exit 3\r");
    await vi.waitFor(() => expect(useStore.getState().terminals[termId]?.status).toBe("exited"));

    const restart = backend.termRestart.bind(backend);
    vi.spyOn(backend, "termRestart").mockImplementation(async (id) => {
      await restart(id);
      channel?.({ type: "exit", code: 1, signal: null }); // the new shell died at once; the event came first
    });
    entry.term.input("\r");
    await vi.waitFor(() => expect(backend.termRestart).toHaveBeenCalled());
    await tick(20);
    expect(useStore.getState().terminals[termId]).toMatchObject({ status: "exited", exitCode: 1 });

    // And Enter still works afterwards.
    vi.mocked(backend.termRestart).mockImplementation(restart);
    entry.term.input("\r");
    await vi.waitFor(() => expect(useStore.getState().terminals[termId]?.status).toBe("running"));
  });

  it("an exit caused by a signal shows the (escaped) signal name", async () => {
    const sessionId = await connect(backend);
    const termId = (await openTerminalTab(sessionId)) as string;
    const entry = getTerminal(termId) as TerminalEntry;
    channel?.({ type: "exit", code: null, signal: "KILL" });
    await new Promise<void>((r) => entry.term.write("", r));
    const buffer = entry.term.buffer.active;
    const text = Array.from({ length: buffer.length }, (_, i) => buffer.getLine(i)?.translateToString(true) ?? "").join("\n");
    expect(text).toContain("[Process terminated by signal KILL] Press Enter to restart");
  });
});
