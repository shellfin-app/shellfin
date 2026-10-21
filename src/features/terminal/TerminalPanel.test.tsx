import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import { sshConfigTarget } from "../../lib/targets";
import { useStore } from "../../state/store";
import { setupApp } from "../../test/harness";
import { openSession } from "../session/sessionActions";
import { disposeAllTerminals, getTerminal, terminalCount, type TerminalEntry } from "./terminalRegistry";

afterEach(() => {
  disposeAllTerminals();
  setBackend(null);
});

const ESC = String.fromCharCode(0x1b);
const tick = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

async function connected() {
  const app = setupApp();
  const sessionId = await openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
  await waitFor(() => expect(useStore.getState().workspaces[sessionId]?.terminalTabs).toHaveLength(1), { timeout: 3000 });
  const panel = await screen.findByRole("region", { name: "Terminal panel" });
  const termId = useStore.getState().workspaces[sessionId]?.activeTerminal as string;
  const entry = getTerminal(termId) as TerminalEntry;
  return { ...app, sessionId, panel, termId, entry };
}

function key(target: Element, init: KeyboardEventInit & { keyCode: number }) {
  const event = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
  target.dispatchEvent(event);
  return event;
}

describe("TerminalPanel", () => {
  it("opens a terminal on connect and shows its tab; New terminal adds a second one that keeps the first alive", async () => {
    const { user, panel, sessionId, termId } = await connected();
    expect(within(panel).getByRole("tab", { name: /^Terminal$/ }).getAttribute("aria-selected")).toBe("true");
    await user.click(within(panel).getByRole("button", { name: "New terminal" }));
    await waitFor(() => expect(within(panel).getAllByRole("tab")).toHaveLength(2));
    const second = useStore.getState().workspaces[sessionId]?.activeTerminal;
    expect(second).not.toBe(termId);
    expect(terminalCount()).toBe(2);
    await user.click(within(panel).getByRole("tab", { name: /^1 · Terminal$/ }));
    expect(useStore.getState().workspaces[sessionId]?.activeTerminal).toBe(termId);
    expect(getTerminal(termId)).toBeDefined();
    expect(getTerminal(second as string)).toBeDefined(); // parked, not destroyed
  });

  it("renames a tab by double-click (Enter commits, Escape cancels, empty restores the terminal's own title)", async () => {
    const { user, panel, termId } = await connected();
    await user.dblClick(within(panel).getByRole("tab", { name: /^Terminal$/ }));
    let input = within(panel).getByLabelText("Terminal name");
    await user.type(input, "deploy logs{Enter}");
    expect(within(panel).getByRole("tab", { name: "deploy logs" })).toBeTruthy();
    expect(useStore.getState().terminals[termId]?.name).toBe("deploy logs");

    await user.dblClick(within(panel).getByRole("tab", { name: "deploy logs" }));
    input = within(panel).getByLabelText("Terminal name");
    await user.clear(input);
    await user.type(input, "discarded{Escape}");
    expect(within(panel).getByRole("tab", { name: "deploy logs" })).toBeTruthy();

    await user.dblClick(within(panel).getByRole("tab", { name: "deploy logs" }));
    await user.clear(within(panel).getByLabelText("Terminal name"));
    await user.keyboard("{Enter}");
    expect(within(panel).getByRole("tab", { name: "Terminal" })).toBeTruthy();
  });

  it("F2 renames the focused tab; the terminal's OSC title shows through displayServerText (controls escaped)", async () => {
    const { user, panel, entry } = await connected();
    entry.term.write(`${ESC}]0;ssh ${String.fromCharCode(0x202e)}x${String.fromCharCode(7)}`);
    await new Promise<void>((r) => entry.term.write("", r));
    // The bidi override in the title must be shown escaped, never rendered.
    expect(await within(panel).findByRole("tab", { name: /ssh .*⟨U\+202E⟩/ })).toBeTruthy();
    within(panel).getByRole("tab", { name: /ssh/ }).focus();
    await user.keyboard("{F2}");
    expect(within(panel).getByLabelText("Terminal name")).toBeTruthy();
  });

  it("closing the last tab releases the terminal and shows the empty state with a New terminal button", async () => {
    const { user, panel } = await connected();
    await user.click(within(panel).getByRole("button", { name: /Close terminal Terminal/ }));
    // A shell is still running: closing asks first (setting `general.confirmCloseWithTerminals`, on by default).
    const confirm = await screen.findByRole("alertdialog", { name: "Close the running terminal?" });
    expect(terminalCount()).toBe(1);
    await user.click(within(confirm).getByRole("button", { name: "Close terminal" }));
    expect(terminalCount()).toBe(0);
    expect(within(panel).getByText("No terminal is open in this session.")).toBeTruthy();
    await user.click(within(panel).getAllByRole("button", { name: "New terminal" }).at(-1) as HTMLElement);
    await waitFor(() => expect(terminalCount()).toBe(1));
  });

  it("middle-click on a tab closes it", async () => {
    const { panel } = await connected();
    const tab = within(panel).getByRole("tab", { name: /^Terminal$/ }).parentElement as HTMLElement;
    const settings = useStore.getState().settings;
    if (!settings) throw new Error("settings are loaded");
    useStore.getState().setSettings({ ...settings, general: { ...settings.general, confirmCloseWithTerminals: false } }); // no question
    fireEvent(tab, new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 1 }));
    expect(terminalCount()).toBe(0);
  });

  it("with the confirmation switched off, closing a running terminal needs no question", async () => {
    const { user, panel } = await connected();
    const settings = useStore.getState().settings;
    if (!settings) throw new Error("settings are loaded");
    useStore.getState().setSettings({ ...settings, general: { ...settings.general, confirmCloseWithTerminals: false } });
    await user.click(within(panel).getByRole("button", { name: /Close terminal Terminal/ }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(terminalCount()).toBe(0);
  });

  it("cancelling the question keeps the terminal", async () => {
    const { user, panel } = await connected();
    await user.click(within(panel).getByRole("button", { name: /Close terminal Terminal/ }));
    const confirm = await screen.findByRole("alertdialog", { name: "Close the running terminal?" });
    await user.click(within(confirm).getByRole("button", { name: "Cancel" }));
    expect(terminalCount()).toBe(1);
  });

  it("Ctrl+Shift+F in the terminal opens the find bar; Escape closes it and returns focus to the terminal", async () => {
    const { user, panel, entry } = await connected();
    const textarea = entry.term.textarea as HTMLTextAreaElement;
    const find = key(textarea, { key: "F", code: "KeyF", keyCode: 70, ctrlKey: true, shiftKey: true });
    expect(find.defaultPrevented).toBe(true);
    const input = await within(panel).findByLabelText("Find in terminal", { selector: "input" });
    await waitFor(() => expect(document.activeElement).toBe(input));

    entry.term.write("alpha beta\r\n");
    await new Promise<void>((r) => entry.term.write("", r));
    await user.type(input, "beta");
    expect(within(panel).queryByText("No matches")).toBeNull();
    expect(entry.term.getSelection()).toBe("beta");
    await user.clear(input);
    await user.type(input, "zzz");
    expect(await within(panel).findByText("No matches")).toBeTruthy();

    await user.keyboard("{Escape}");
    expect(within(panel).queryByLabelText("Find in terminal", { selector: "input" })).toBeNull();
    expect(useStore.getState().terminalSearch).toBeNull();
    await waitFor(() => expect(document.activeElement).toBe(textarea));
  });

  it("plain Ctrl+F is NOT intercepted in the terminal (readline forward-char)", async () => {
    const { entry, backend } = await connected();
    const write = vi.spyOn(backend, "termWrite");
    key(entry.term.textarea as HTMLTextAreaElement, { key: "f", code: "KeyF", keyCode: 70, ctrlKey: true });
    await waitFor(() => expect(write).toHaveBeenCalled());
    expect(Uint8Array.from(write.mock.calls[0]?.[1] as number[])[0]).toBe(0x06);
    expect(useStore.getState().terminalSearch).toBeNull();
  });

  it("Ctrl+K inside the terminal reaches the shell and does NOT open the palette; Ctrl+Shift+K does", async () => {
    const { entry, backend } = await connected();
    const write = vi.spyOn(backend, "termWrite");
    const textarea = entry.term.textarea as HTMLTextAreaElement;
    key(textarea, { key: "k", code: "KeyK", keyCode: 75, ctrlKey: true });
    await waitFor(() => expect(write).toHaveBeenCalled());
    expect(Uint8Array.from(write.mock.calls[0]?.[1] as number[])[0]).toBe(0x0b);
    expect(useStore.getState().palette).toBeNull();

    key(textarea, { key: "K", code: "KeyK", keyCode: 75, ctrlKey: true, shiftKey: true });
    expect(useStore.getState().palette).toBe("all");
    expect(write).toHaveBeenCalledTimes(1);
  });

  it("Ctrl+` toggles the panel and Ctrl+Shift+` opens a new terminal, also from inside the terminal", async () => {
    const { entry, sessionId } = await connected();
    const textarea = entry.term.textarea as HTMLTextAreaElement;
    key(textarea, { key: "~", code: "Backquote", keyCode: 192, ctrlKey: true, shiftKey: true });
    await waitFor(() => expect(terminalCount()).toBe(2));
    key(textarea, { key: "`", code: "Backquote", keyCode: 192, ctrlKey: true });
    expect(useStore.getState().workspaces[sessionId]?.panelOpen).toBe(false);
    act(() => useStore.getState().setPanelOpen(sessionId, true));
    expect(useStore.getState().workspaces[sessionId]?.panelOpen).toBe(true);
  });

  it("a multi-line paste into a shell without bracketed paste shows the confirmation dialog, then pastes on confirm", async () => {
    const { user, entry, backend } = await connected();
    entry.term.write(`${ESC}[?2004l`);
    await new Promise<void>((r) => entry.term.write("", r));
    const write = vi.spyOn(backend, "termWrite");
    const event = new Event("paste", { bubbles: true, cancelable: true });
    Object.defineProperty(event, "clipboardData", { value: { getData: () => `echo one${ESC}[201~\necho two\n` } });
    entry.term.textarea?.dispatchEvent(event);

    const dialog = await screen.findByRole("alertdialog", { name: "Paste 2 lines?" });
    expect(within(dialog).getByText(/echo one\[201~/)).toBeTruthy(); // the preview is sanitised, too
    expect(write).not.toHaveBeenCalled();
    await user.click(within(dialog).getByRole("button", { name: "Paste" }));
    await waitFor(() => expect(write).toHaveBeenCalled());
    const sent = new TextDecoder().decode(Uint8Array.from(write.mock.calls.flatMap((c) => Array.from(c[1]))));
    expect(sent).toBe("echo one[201~\recho two\r");
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("a multi-line paste can be cancelled with Escape", async () => {
    const { user, entry, backend } = await connected();
    entry.term.write(`${ESC}[?2004l`);
    await new Promise<void>((r) => entry.term.write("", r));
    const write = vi.spyOn(backend, "termWrite");
    const event = new Event("paste", { bubbles: true, cancelable: true });
    Object.defineProperty(event, "clipboardData", { value: { getData: () => "a\nb" } });
    entry.term.textarea?.dispatchEvent(event);
    await screen.findByRole("alertdialog");
    await user.keyboard("{Escape}");
    await tick(10);
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(write).not.toHaveBeenCalled();
  });

  it("an exited terminal's tab is marked and the restart hint is announced", async () => {
    const { panel, termId, entry } = await connected();
    entry.term.input("exit\r");
    await waitFor(() => expect(useStore.getState().terminals[termId]?.status).toBe("exited"));
    const tab = within(panel).getByRole("tab", { name: /^Terminal$/ });
    expect(tab.title).toMatch(/press Enter to restart/i);
    expect(tab.parentElement?.className).toContain("terminal-tab--ended");
  });
});
