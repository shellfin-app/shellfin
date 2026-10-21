// Terminal / General settings: cursor style, line height, bell, right-click, confirm before closing.
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import type { Settings } from "../../ipc/types";
import { sshConfigTarget } from "../../lib/targets";
import { useStore } from "../../state/store";
import { setupApp } from "../../test/harness";
import { useMenuStore } from "../menu/menuStore";
import { handleQuitRequested } from "../session/quitFlow";
import { openSession } from "../session/sessionActions";
import { BELL_CLASS, disposeAllTerminals, getTerminal, type TerminalEntry } from "./terminalRegistry";

afterEach(() => {
  disposeAllTerminals();
  useMenuStore.getState().close();
  setBackend(null);
  vi.restoreAllMocks();
});

async function connected() {
  const app = setupApp();
  const sessionId = await openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
  await waitFor(() => expect(useStore.getState().workspaces[sessionId]?.terminalTabs).toHaveLength(1), { timeout: 3000 });
  const panel = await screen.findByRole("region", { name: "Terminal panel" });
  const termId = useStore.getState().workspaces[sessionId]?.activeTerminal as string;
  return { ...app, sessionId, panel, termId, entry: getTerminal(termId) as TerminalEntry };
}

function patchSettings(patch: (s: Settings) => Settings): void {
  const settings = useStore.getState().settings;
  if (!settings) throw new Error("no settings");
  act(() => useStore.getState().setSettings(patch(settings)));
}

describe("terminal appearance", () => {
  it("cursor style and line height reach xterm, also for terminals that are already open", async () => {
    const { entry } = await connected();
    expect(entry.term.options.cursorStyle).toBe("block");
    expect(entry.term.options.lineHeight).toBe(1.2);
    patchSettings((s) => ({ ...s, terminal: { ...s.terminal, cursorStyle: "underline", lineHeight: 1.6 } }));
    expect(entry.term.options.cursorStyle).toBe("underline");
    expect(entry.term.options.lineHeight).toBe(1.6);
  });

  it("visual bell flashes the pane briefly; off does nothing", async () => {
    const { entry } = await connected();
    const bell = async () => {
      await new Promise<void>((r) => entry.term.write("\u0007", r));
    };
    await bell();
    expect(entry.host.classList.contains(BELL_CLASS)).toBe(false);

    patchSettings((s) => ({ ...s, terminal: { ...s.terminal, bell: "visual" } }));
    await bell();
    await waitFor(() => expect(entry.host.classList.contains(BELL_CLASS)).toBe(true));
    await waitFor(() => expect(entry.host.classList.contains(BELL_CLASS)).toBe(false), { timeout: 1500 });
  });
});

describe("right-click", () => {
  it("shows the terminal menu by default", async () => {
    const { entry } = await connected();
    fireEvent.contextMenu(entry.host);
    await screen.findByRole("menuitem", { name: /^Paste/ });
  });

  it("pastes the clipboard (sanitised path) and shows no menu when set to paste", async () => {
    const { entry } = await connected();
    patchSettings((s) => ({ ...s, terminal: { ...s.terminal, rightClick: "paste" } }));
    const readText = vi.fn().mockResolvedValue("echo hi");
    Object.defineProperty(navigator, "clipboard", { value: { readText, writeText: vi.fn() }, configurable: true });
    const paste = vi.spyOn(entry.term, "paste");
    fireEvent.contextMenu(entry.host);
    await waitFor(() => expect(paste).toHaveBeenCalledWith("echo hi"));
    expect(readText).toHaveBeenCalledOnce();
    expect(screen.queryByRole("menu")).toBeNull();
  });
});

describe("confirm before closing running terminals", () => {
  it("closing a connection with a running shell asks first; Cancel keeps it, confirm closes it", async () => {
    const { user, sessionId } = await connected();
    await user.click(screen.getByRole("button", { name: /Close connection/ }));
    const dialog = await screen.findByRole("alertdialog", { name: "Close this connection?" });
    expect(useStore.getState().sessionOrder).toContain(sessionId);
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(useStore.getState().sessionOrder).toContain(sessionId);

    await user.click(screen.getByRole("button", { name: /Close connection/ }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Close connection" }));
    await waitFor(() => expect(useStore.getState().sessionOrder).not.toContain(sessionId));
  });

  it("with the setting off, closing a connection needs no question", async () => {
    const { user, sessionId } = await connected();
    patchSettings((s) => ({ ...s, general: { ...s.general, confirmCloseWithTerminals: false } }));
    await user.click(screen.getByRole("button", { name: /Close connection/ }));
    await waitFor(() => expect(useStore.getState().sessionOrder).not.toContain(sessionId));
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("quit with a running terminal asks; Cancel aborts, Quit exits; confirmQuit = never quits at once", async () => {
    const { user, backend } = await connected();
    const quit = vi.spyOn(backend, "appQuit");
    const cancel = vi.spyOn(backend, "appQuitCancel");

    await act(async () => handleQuitRequested());
    const dialog = await screen.findByRole("alertdialog", { name: "Quit with running terminals?" });
    expect(dialog.textContent).toContain("A terminal is still running a shell");
    expect(quit).not.toHaveBeenCalled();
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(cancel).toHaveBeenCalledOnce());

    await act(async () => handleQuitRequested());
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Quit" }));
    await waitFor(() => expect(quit).toHaveBeenCalledOnce());

    // Quitting is governed by `general.confirmQuit` (confirmCloseWithTerminals only covers closing tabs/connections).
    patchSettings((s) => ({ ...s, general: { ...s.general, confirmQuit: "never" } }));
    await act(async () => handleQuitRequested());
    await waitFor(() => expect(quit).toHaveBeenCalledTimes(2));
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });
});
