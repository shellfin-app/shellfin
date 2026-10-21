import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { IpcError, setBackend } from "../../ipc";
import { sshConfigTarget } from "../../lib/targets";
import { useStore } from "../../state/store";
import { setupApp } from "../../test/harness";
import { openSession } from "../session/sessionActions";
import { disposeAllTerminals, getTerminal, terminalCount, type TerminalEntry } from "../terminal/terminalRegistry";
import { useMenuStore } from "./menuStore";

const ESC = String.fromCharCode(0x1b);

afterEach(() => {
  useMenuStore.getState().close();
  disposeAllTerminals();
  setBackend(null);
  vi.restoreAllMocks();
});

function stubClipboard(initial = "") {
  let text = initial;
  const writeText = vi.fn(async (t: string) => {
    text = t;
  });
  const readText = vi.fn(async () => text);
  Object.defineProperty(navigator, "clipboard", { value: { writeText, readText }, configurable: true });
  return { writeText, readText, current: () => text };
}

async function connected() {
  const app = setupApp();
  const sessionId = await openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
  await waitFor(() => expect(useStore.getState().workspaces[sessionId]?.terminalTabs).toHaveLength(1), { timeout: 3000 });
  const termId = useStore.getState().workspaces[sessionId]?.activeTerminal as string;
  const entry = getTerminal(termId) as TerminalEntry;
  await new Promise<void>((r) => entry.term.write("", r));
  return { ...app, sessionId, termId, entry };
}

function rightClick(el: Element, init: MouseEventInit = {}): MouseEvent {
  const event = new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 40, clientY: 40, ...init });
  act(() => {
    el.dispatchEvent(event);
  });
  return event;
}

describe("No native context menu anywhere", () => {
  it("every contextmenu event is cancelled, also on chrome that has no menu of its own", async () => {
    setupApp();
    for (const el of [document.body, document.querySelector(".shell") as Element, document.querySelector(".status-bar") ?? document.body]) {
      expect(rightClick(el).defaultPrevented).toBe(true);
    }
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("terminal: Copy (disabled without a selection), Paste, Select all, Find, Clear — with key hints", async () => {
    const { entry } = await connected();
    rightClick(entry.host.querySelector(".xterm-screen") ?? entry.host);
    const menu = await screen.findByRole("menu", { name: "Context menu" });
    const items = within(menu).getAllByRole("menuitem");
    expect(items.map((i) => i.textContent)).toEqual([
      "CopyCtrl+Shift+C",
      "PasteCtrl+Shift+V",
      "Select all",
      "Find…Ctrl+Shift+F",
      "Clear",
      "Save output…",
    ]);
    expect(within(menu).getByRole("menuitem", { name: /^Copy/ }).getAttribute("aria-disabled")).toBe("true");
    // The first enabled item has focus; Escape closes and gives the terminal its focus back.
    expect(document.activeElement).toBe(within(menu).getByRole("menuitem", { name: /^Paste/ }));
    fireEvent.keyDown(menu, { key: "Escape" });
    expect(screen.queryByRole("menu")).toBeNull();
    await waitFor(() => expect(document.activeElement).toBe(entry.term.textarea));
  });

  it("Save output… is in the terminal area menu and in the terminal tab menu (same action)", async () => {
    const { entry, user, backend } = await connected();
    const save = vi.spyOn(backend, "appSaveTextFile");
    entry.term.write("hello output\r\n");
    await new Promise<void>((r) => entry.term.write("", r));
    rightClick(entry.host);
    await user.click(await screen.findByRole("menuitem", { name: "Save output…" }));
    await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
    expect(String(save.mock.calls[0]?.[1])).toContain("hello output");
    await waitFor(() => expect(useStore.getState().notices.some((n) => n.info?.kind === "outputSaved")).toBe(true));

    const panel = screen.getByRole("region", { name: "Terminal panel" });
    rightClick(within(panel).getByRole("tab"));
    await user.click(await screen.findByRole("menuitem", { name: "Save output…" }));
    await waitFor(() => expect(save).toHaveBeenCalledTimes(2));
  });

  it("terminal Paste goes through the sanitiser (and the multi-line confirmation)", async () => {
    const { entry, user, backend } = await connected();
    const clipboard = stubClipboard(`echo a${ESC}[201~b`);
    const write = vi.spyOn(backend, "termWrite");
    rightClick(entry.host);
    await user.click(await screen.findByRole("menuitem", { name: /^Paste/ }));
    await waitFor(() => expect(write).toHaveBeenCalled());
    expect(clipboard.readText).toHaveBeenCalled();
    const sent = new TextDecoder().decode(Uint8Array.from(write.mock.calls.flatMap((c) => Array.from(c[1]))));
    expect(sent).toBe(`${ESC}[200~echo a[201~b${ESC}[201~`); // no ESC inside the bracketed payload

    entry.term.write(`${ESC}[?2004l`); // shell without bracketed paste + multi-line clipboard: confirm first
    await new Promise<void>((r) => entry.term.write("", r));
    stubClipboard("l1\nl2");
    write.mockClear();
    rightClick(entry.host);
    await user.click(await screen.findByRole("menuitem", { name: /^Paste/ }));
    await screen.findByRole("alertdialog", { name: "Paste 2 lines?" });
    expect(write).not.toHaveBeenCalled();
  });

  it("terminal Copy with a selection, Select all, Clear and Find", async () => {
    const { entry, user } = await connected();
    const clipboard = stubClipboard();
    entry.term.write("hello\r\n");
    await new Promise<void>((r) => entry.term.write("", r));
    entry.term.selectAll();
    rightClick(entry.host);
    await user.click(await screen.findByRole("menuitem", { name: /^Copy/ }));
    expect(clipboard.writeText).toHaveBeenCalledWith(expect.stringContaining("hello"));

    entry.term.clearSelection();
    rightClick(entry.host);
    await user.click(await screen.findByRole("menuitem", { name: "Select all" }));
    expect(entry.term.hasSelection()).toBe(true);

    rightClick(entry.host);
    await user.click(await screen.findByRole("menuitem", { name: /^Find/ }));
    expect(useStore.getState().terminalSearch).toBe(entry.termId);

    rightClick(entry.host);
    await user.click(await screen.findByRole("menuitem", { name: "Clear" }));
    expect(entry.term.buffer.active.baseY).toBe(0);
  });

  it("arrow keys move through the items and Enter runs one", async () => {
    const { entry, termId } = await connected();
    rightClick(entry.host);
    const menu = await screen.findByRole("menu");
    const paste = within(menu).getByRole("menuitem", { name: /^Paste/ });
    const all = within(menu).getByRole("menuitem", { name: "Select all" });
    expect(document.activeElement).toBe(paste);
    fireEvent.keyDown(menu, { key: "ArrowDown" });
    expect(document.activeElement).toBe(all);
    fireEvent.keyDown(menu, { key: "ArrowUp" });
    expect(document.activeElement).toBe(paste);
    fireEvent.keyDown(menu, { key: "End" });
    expect(document.activeElement).toBe(within(menu).getByRole("menuitem", { name: "Save output…" }));
    fireEvent.keyDown(menu, { key: "Home" });
    fireEvent.keyDown(menu, { key: "ArrowDown" });
    fireEvent.click(document.activeElement as Element);
    expect(screen.queryByRole("menu")).toBeNull();
    expect(getTerminal(termId)?.term.hasSelection()).toBe(true); // "Select all" ran
  });

  it("clicking elsewhere closes the menu", async () => {
    const { entry, user } = await connected();
    rightClick(entry.host);
    await screen.findByRole("menu");
    await user.click(document.body);
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("terminal tab: Rename, Close, Close others", async () => {
    const { sessionId, user } = await connected();
    await user.click(screen.getByRole("button", { name: "New terminal" }));
    await waitFor(() => expect(terminalCount()).toBe(2));
    const panel = screen.getByRole("region", { name: "Terminal panel" });

    rightClick(within(panel).getByRole("tab", { name: /1 · Terminal/ }));
    await user.click(await screen.findByRole("menuitem", { name: "Rename" }));
    const input = within(panel).getByLabelText("Terminal name");
    await user.type(input, "logs{Enter}");
    expect(within(panel).getByRole("tab", { name: "1 · logs" })).toBeTruthy();

    rightClick(within(panel).getByRole("tab", { name: "1 · logs" }));
    await user.click(await screen.findByRole("menuitem", { name: "Close others" }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Close terminal" })); // a shell is running
    await waitFor(() => expect(terminalCount()).toBe(1));
    expect(within(panel).getAllByRole("tab")).toHaveLength(1);

    rightClick(within(panel).getByRole("tab", { name: "logs" }));
    expect((await screen.findByRole("menuitem", { name: "Close others" })).getAttribute("aria-disabled")).toBe("true");
    await user.click(screen.getByRole("menuitem", { name: "Close" }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Close terminal" }));
    expect(terminalCount()).toBe(0);
    expect(useStore.getState().workspaces[sessionId]?.terminalTabs).toEqual([]);
  });

  it("connection tab: Rename changes the label shown, Close closes the session", async () => {
    const { sessionId, user } = await connected();
    const tabs = screen.getByRole("tablist", { name: "Connections" });
    rightClick(within(tabs).getByRole("tab", { name: /prod-web-01/ }));
    await user.click(await screen.findByRole("menuitem", { name: "Rename" }));
    await user.type(within(tabs).getByLabelText("Connection name"), "production{Enter}");
    expect(within(tabs).getByRole("tab", { name: /production/ })).toBeTruthy();
    expect(useStore.getState().sessionsById[sessionId]?.label).toBe("prod-web-01"); // the host identity is untouched

    rightClick(within(tabs).getByRole("tab", { name: /production/ }));
    await user.click(await screen.findByRole("menuitem", { name: "Close" }));
    await user.click(within(await screen.findByRole("alertdialog", { name: "Close this connection?" })).getByRole("button", { name: "Close connection" }));
    await waitFor(() => expect(useStore.getState().sessionOrder).toEqual([]));
    expect(terminalCount()).toBe(0);
  });

  it("text inputs: Cut / Copy / Paste / Select all; password fields cannot be copied", async () => {
    const { user } = setupApp();
    const clipboard = stubClipboard("pasted");
    const input = screen.getByPlaceholderText("Bookmark name or user@host:port") as HTMLInputElement;
    await user.type(input, "hello");
    input.setSelectionRange(0, 5);
    rightClick(input);
    await user.click(await screen.findByRole("menuitem", { name: "Copy" }));
    expect(clipboard.writeText).toHaveBeenCalledWith("hello");

    input.setSelectionRange(0, 5);
    rightClick(input);
    await user.click(await screen.findByRole("menuitem", { name: "Cut" }));
    expect(input.value).toBe("");

    rightClick(input);
    await user.click(await screen.findByRole("menuitem", { name: "Paste" }));
    await waitFor(() => expect(input.value).toBe("hello")); // the clipboard now holds what was cut
    rightClick(input);
    await user.click(await screen.findByRole("menuitem", { name: "Select all" }));
    expect(input.selectionStart).toBe(0);
    expect(input.selectionEnd).toBe(5);

    const secret = document.createElement("input");
    secret.type = "password";
    secret.value = "hunter2";
    document.body.appendChild(secret);
    secret.setSelectionRange(0, 7);
    rightClick(secret);
    const menu = await screen.findByRole("menu");
    expect(within(menu).getByRole("menuitem", { name: "Copy" }).getAttribute("aria-disabled")).toBe("true");
    expect(within(menu).getByRole("menuitem", { name: "Cut" }).getAttribute("aria-disabled")).toBe("true");
    secret.remove();
  });
});

describe("Files panel when listing fails", () => {
  it("other listing failures are reported once and shown as 'could not load'", async () => {
    const app = setupApp();
    vi.spyOn(app.backend, "sftpList").mockRejectedValue(new IpcError({ code: "permissionDenied", params: { path: "/root" } } as never));
    const sessionId = await openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
    await waitFor(() => expect(useStore.getState().workspaces[sessionId]?.filesStatus).toBe("failed"), { timeout: 3000 });
    expect(await screen.findByText("Could not load this folder.")).toBeTruthy();
    expect(useStore.getState().notices).toHaveLength(1);
  });
});

describe("Palette focus and discoverable Find", () => {
  it("closing the palette with Escape returns focus to the terminal", async () => {
    const { entry, user } = await connected();
    entry.term.focus();
    expect(document.activeElement).toBe(entry.term.textarea);
    fireEvent.keyDown(entry.term.textarea as Element, { key: "K", code: "KeyK", keyCode: 75, ctrlKey: true, shiftKey: true });
    await screen.findByRole("dialog", { name: "Command palette" });
    const input = document.querySelector("[cmdk-input]") as HTMLElement;
    await waitFor(() => expect(document.activeElement).toBe(input));
    await user.keyboard("{Escape}");
    await waitFor(() => expect(document.activeElement).toBe(entry.term.textarea));
  });

  it("the Find button names its shortcut", async () => {
    await connected();
    expect(screen.getByRole("button", { name: "Find in terminal (Ctrl+Shift+F)" })).toBeTruthy();
  });
});
