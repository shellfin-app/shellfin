import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import { sshConfigTarget } from "../../lib/targets";
import { useStore } from "../../state/store";
import { setupApp } from "../../test/harness";
import { Notices, ERROR_NOTICE_TIMEOUT_MS, NOTICE_TIMEOUT_MS } from "../shell/Notices";
import { openSession } from "../session/sessionActions";
import { disposeAllTerminals, getTerminal, type TerminalEntry } from "../terminal/terminalRegistry";

afterEach(() => {
  disposeAllTerminals();
  setBackend(null);
  vi.useRealTimers();
});

function keydown(target: Element | Window, init: KeyboardEventInit & { keyCode?: number }): KeyboardEvent {
  const event = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
  act(() => {
    target.dispatchEvent(event);
  });
  return event;
}

describe("Browser accelerators in the running app", () => {
  it("F5, Ctrl+R, Ctrl+P, Ctrl+U, Ctrl+S, Ctrl+=, Alt+Left are cancelled outside the terminal", () => {
    setupApp();
    for (const init of [
      { key: "F5" },
      { key: "r", ctrlKey: true },
      { key: "R", ctrlKey: true, shiftKey: true },
      { key: "p", ctrlKey: true },
      { key: "u", ctrlKey: true },
      { key: "s", ctrlKey: true },
      { key: "f", ctrlKey: true },
      { key: "=", ctrlKey: true },
      { key: "ArrowLeft", altKey: true },
    ]) {
      expect(keydown(document.body, init).defaultPrevented, JSON.stringify(init)).toBe(true);
    }
    // Ordinary typing and editing shortcuts are untouched.
    expect(keydown(document.body, { key: "a", ctrlKey: true }).defaultPrevented).toBe(false);
  });

  it("Ctrl+R / Ctrl+P / Ctrl+U still reach the shell when the terminal is focused", async () => {
    const { backend } = setupApp();
    const sessionId = await openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
    await waitFor(() => expect(useStore.getState().workspaces[sessionId]?.terminalTabs).toHaveLength(1), { timeout: 3000 });
    const entry = getTerminal(useStore.getState().workspaces[sessionId]?.activeTerminal as string) as TerminalEntry;
    const write = vi.spyOn(backend, "termWrite");
    const textarea = entry.term.textarea as HTMLTextAreaElement;
    keydown(textarea, { key: "r", code: "KeyR", keyCode: 82, ctrlKey: true });
    keydown(textarea, { key: "p", code: "KeyP", keyCode: 80, ctrlKey: true });
    keydown(textarea, { key: "u", code: "KeyU", keyCode: 85, ctrlKey: true });
    await waitFor(() => expect(write).toHaveBeenCalledTimes(3));
    const bytes = write.mock.calls.map((c) => (c[1] as number[])[0]);
    expect(bytes).toEqual([0x12, 0x10, 0x15]); // ^R ^P ^U
  });

  it("Ctrl+wheel (page zoom) is cancelled on Linux and macOS", () => {
    setupApp();
    act(() => useStore.setState({ platform: "linux" }));
    const event = new WheelEvent("wheel", { ctrlKey: true, bubbles: true, cancelable: true, deltaY: -100 });
    act(() => {
      document.body.dispatchEvent(event);
    });
    expect(event.defaultPrevented).toBe(true);
    const plain = new WheelEvent("wheel", { bubbles: true, cancelable: true, deltaY: 100 });
    document.body.dispatchEvent(plain);
    expect(plain.defaultPrevented).toBe(false);
  });

  it("registers no wheel listener on Windows (WebView2 has zoom control off), so scrolling never waits for it", () => {
    const add = vi.spyOn(window, "addEventListener");
    setupApp();
    expect(add.mock.calls.some(([type]) => type === "wheel")).toBe(false);
    add.mockRestore();
  });
});

describe("Dropping onto the app window", () => {
  it("does not navigate or open anything", () => {
    setupApp();
    const event = new Event("drop", { bubbles: true, cancelable: true });
    document.body.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
  });
});

describe("Toasts", () => {
  it("identical errors are shown once", () => {
    setupApp();
    const { pushNotice } = useStore.getState();
    act(() => {
      pushNotice({ code: "internal" });
      pushNotice({ code: "internal" });
      pushNotice({ code: "notConnected", params: { sessionId: "s" } });
    });
    expect(useStore.getState().notices).toHaveLength(2);
  });

  it("toasts dismiss themselves", () => {
    vi.useFakeTimers();
    useStore.setState(useStore.getInitialState(), true);
    render(<Notices />);
    act(() => useStore.getState().pushNotice({ code: "internal" }));
    expect(screen.getAllByRole("alert")).toHaveLength(1);
    act(() => {
      vi.advanceTimersByTime(NOTICE_TIMEOUT_MS + 10);
    });
    expect(screen.getAllByRole("alert")).toHaveLength(1); // errors stay longer than informational toasts
    act(() => {
      vi.advanceTimersByTime(ERROR_NOTICE_TIMEOUT_MS);
    });
    expect(screen.queryByRole("alert")).toBeNull();
    expect(useStore.getState().notices).toEqual([]);
  });

  it("the live region stays mounted when empty, and hovering a toast pauses its timer", () => {
    vi.useFakeTimers();
    useStore.setState(useStore.getInitialState(), true);
    render(<Notices />);
    expect(screen.getByRole("region", { name: "Notifications" })).toBeTruthy();
    act(() => useStore.getState().pushInfo({ kind: "copied" }));
    const toast = screen.getByRole("status");
    act(() => {
      fireEvent.mouseEnter(toast);
    });
    act(() => {
      vi.advanceTimersByTime(NOTICE_TIMEOUT_MS * 3);
    });
    expect(screen.queryByRole("status")).not.toBeNull();
    act(() => {
      fireEvent.mouseLeave(toast);
    });
    act(() => {
      vi.advanceTimersByTime(NOTICE_TIMEOUT_MS + 10);
    });
    expect(screen.queryByRole("status")).toBeNull();
    expect(screen.getByRole("region", { name: "Notifications" })).toBeTruthy();
  });
});

describe("Long config paths", () => {
  it("the Home cards carry the full path as a tooltip (CSS truncates the visible text)", () => {
    setupApp();
    const value = document.querySelector(".info-card__value[title]");
    expect(value?.getAttribute("title")).toBeTruthy();
  });
});

describe("Known_hosts count on Home", () => {
  it("env info is refreshed when a session connects", async () => {
    const { backend } = setupApp();
    const fresh = { ...(await backend.envInfo()), knownHosts: { ...(await backend.envInfo()).knownHosts, entryCount: 7 } };
    vi.spyOn(backend, "envInfo").mockResolvedValue(fresh);
    await openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
    await waitFor(() => expect(useStore.getState().env?.knownHosts.entryCount).toBe(7), { timeout: 3000 });
  });
});
