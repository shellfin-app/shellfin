// IME safety of Enter / Escape handlers (#17, ARCHITECTURE §8.7): a key that confirms or cancels an IME conversion
// (`isComposing`, or `keyCode` 229 during or just after a composition, as WKWebView sends the confirming Enter) never
// submits, commits or closes; a 229 key outside a composition (IBus / Fcitx5) is an ordinary key.
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Dialog } from "../components/Dialog";
import { InlineRename } from "../components/InlineRename";
import { TerminalFindBar } from "../features/terminal/TerminalFindBar";
import { setBackend } from "../ipc";
import { useStore } from "../state/store";
import { settleApp, setupApp, setupConnected } from "../test/harness";
import { IME_CONFIRM_WINDOW_MS, isImeKeyEvent } from "./ime";

afterEach(async () => {
  await settleApp();
  setBackend(null);
  vi.restoreAllMocks();
});

const composing = { isComposing: true };

/** WKWebView's confirming key: the composition ends, then the keydown arrives with keyCode 229, isComposing false. */
function confirmKey(target: Element, key: string): void {
  fireEvent.compositionStart(target);
  fireEvent.compositionEnd(target);
  fireEvent.keyDown(target, { key, keyCode: 229 });
}

const key229 = (k: string) => new KeyboardEvent("keydown", { key: k, keyCode: 229 });

describe("isImeKeyEvent", () => {
  it("is true while composing, for DOM and React events", () => {
    expect(isImeKeyEvent(new KeyboardEvent("keydown", { key: "Enter", isComposing: true }))).toBe(true);
    expect(isImeKeyEvent(new KeyboardEvent("keydown", { key: "Enter", keyCode: 13 }))).toBe(false);
    expect(isImeKeyEvent({ nativeEvent: new KeyboardEvent("keydown", { key: "Escape", isComposing: true }) })).toBe(true);
    expect(isImeKeyEvent({ nativeEvent: new KeyboardEvent("keydown", { key: "Escape" }) })).toBe(false);
  });

  it("keyCode 229 is an IME key during a composition and just after it ends (WKWebView's confirming Enter)", () => {
    const now = vi.spyOn(performance, "now").mockReturnValue(10_000);
    document.dispatchEvent(new CompositionEvent("compositionstart"));
    expect(isImeKeyEvent(key229("a"))).toBe(true);
    document.dispatchEvent(new CompositionEvent("compositionend"));
    expect(isImeKeyEvent(key229("Enter"))).toBe(true);
    now.mockReturnValue(10_000 + IME_CONFIRM_WINDOW_MS);
    expect(isImeKeyEvent(key229("Enter"))).toBe(true);
    now.mockReturnValue(10_000 + IME_CONFIRM_WINDOW_MS + 1);
    expect(isImeKeyEvent(key229("Enter"))).toBe(false);
  });

  it("keyCode 229 without a composition is an ordinary key (IBus / Fcitx5 report 229 for every key)", async () => {
    const now = vi.spyOn(performance, "now").mockReturnValue(50_000);
    expect(isImeKeyEvent(key229("Escape"))).toBe(false);
    expect(isImeKeyEvent({ nativeEvent: key229("Escape") })).toBe(false);
    // Escape, a global shortcut and Alt+letter still work there.
    const close = vi.fn();
    render(
      <Dialog title="Test" onClose={close}>
        <input aria-label="Field" />
      </Dialog>,
    );
    const input = screen.getByRole("textbox", { name: "Field" });
    input.focus();
    fireEvent.keyDown(input, { key: "Escape", keyCode: 229 });
    expect(close).toHaveBeenCalledTimes(1);
    now.mockRestore();
  });

  it("global shortcuts and Alt+letter work with keyCode 229 outside a composition", async () => {
    vi.spyOn(performance, "now").mockReturnValue(90_000);
    setupApp();
    await settleApp();
    fireEvent.keyDown(document.body, { key: "f", code: "KeyF", altKey: true, keyCode: 229 });
    expect(await screen.findByRole("menu", { name: "File" })).toBeTruthy();
  });
});

describe("Enter / Escape during an IME conversion", () => {
  it("InlineRename: neither commits nor cancels; a plain Enter commits", () => {
    const done = vi.fn();
    render(<InlineRename initial="old" label="Rename" onDone={done} />);
    const input = screen.getByRole("textbox", { name: "Rename" });
    fireEvent.change(input, { target: { value: "新しい" } });
    fireEvent.keyDown(input, { key: "Enter", ...composing });
    confirmKey(input, "Enter");
    fireEvent.keyDown(input, { key: "Escape", ...composing });
    expect(done).not.toHaveBeenCalled();
    fireEvent.keyDown(input, { key: "Enter" });
    expect(done).toHaveBeenCalledWith("新しい");
  });

  it("Dialog: Escape that cancels a composition keeps the dialog open; a plain Escape closes it", () => {
    const close = vi.fn();
    render(
      <Dialog title="Test" onClose={close}>
        <input aria-label="Field" />
      </Dialog>,
    );
    const input = screen.getByRole("textbox", { name: "Field" });
    input.focus();
    fireEvent.keyDown(input, { key: "Escape", ...composing });
    confirmKey(input, "Escape");
    expect(close).not.toHaveBeenCalled();
    fireEvent.keyDown(input, { key: "Escape" });
    expect(close).toHaveBeenCalledTimes(1);
  });

  it("TerminalFindBar: Escape while composing keeps the find bar", () => {
    useStore.setState({ terminalSearch: "t1" });
    render(<TerminalFindBar termId="t1" />);
    const input = screen.getByRole("textbox", { name: "Find in terminal" });
    fireEvent.keyDown(input, { key: "Escape", ...composing });
    confirmKey(input, "Enter");
    expect(useStore.getState().terminalSearch).toBe("t1");
    fireEvent.keyDown(input, { key: "Escape" });
    expect(useStore.getState().terminalSearch).toBeNull();
  });

  it("command palette: Escape while composing keeps it open", async () => {
    setupApp();
    act(() => useStore.getState().openPalette("all"));
    const palette = await screen.findByRole("dialog", { name: "Command palette" });
    const input = palette.querySelector("input") as HTMLInputElement;
    fireEvent.keyDown(input, { key: "Escape", ...composing });
    expect(useStore.getState().palette).not.toBeNull();
    fireEvent.keyDown(input, { key: "Escape" });
    await waitFor(() => expect(useStore.getState().palette).toBeNull());
  });

  it("file tree: the new-file field neither creates nor cancels on IME keys", async () => {
    const { user, sessionId } = await setupConnected();
    fireEvent.contextMenu(screen.getByRole("treeitem", { name: "app" }), { clientX: 20, clientY: 20 });
    await user.click(within(await screen.findByRole("menu")).getByRole("menuitem", { name: "New file…" }));
    const input = await screen.findByRole("textbox", { name: "New file name" });
    fireEvent.change(input, { target: { value: "メモ.txt" } });
    fireEvent.keyDown(input, { key: "Enter", ...composing });
    confirmKey(input, "Enter");
    fireEvent.keyDown(input, { key: "Escape", ...composing });
    // Checked at once: the session's first terminal opens a moment later and takes the focus (which ends the field).
    expect(screen.getByRole("textbox", { name: "New file name" })).toBe(input);
    expect((input as HTMLInputElement).disabled).toBe(false); // a commit would make the field busy (disabled)
    fireEvent.keyDown(input, { key: "Enter" });
    await screen.findByRole("treeitem", { name: /メモ\.txt/ });
    expect(useStore.getState().workspaces[sessionId]?.tree.selection).toBe("/srv/app/メモ.txt");
  });

  it("global shortcuts and Alt+letter ignore keys the IME is composing with", async () => {
    setupApp();
    await settleApp();
    fireEvent.keyDown(document.body, { key: "k", code: "KeyK", ctrlKey: true, ...composing });
    expect(useStore.getState().palette).toBeNull();
    fireEvent.keyDown(document.body, { key: "f", code: "KeyF", altKey: true, ...composing });
    expect(screen.queryByRole("menu", { name: "File" })).toBeNull();
    fireEvent.keyDown(document.body, { key: "f", code: "KeyF", altKey: true });
    expect(await screen.findByRole("menu", { name: "File" })).toBeTruthy();
  });
});
