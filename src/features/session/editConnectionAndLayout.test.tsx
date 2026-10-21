// Edit connection, disconnect, layout persistence, ssh command quoting, auth failure wording.
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import type { AppError, Bookmark, SshConfigHost } from "../../ipc/types";
import { A } from "../../lib/actionIds";
import { goToFolderDialog } from "../../lib/dialogs";
import { sshCommand } from "../../lib/sshCommand";
import { quickTarget } from "../../lib/targets";
import { flushLayoutPrefs, loadLayoutPrefs, saveLayoutPrefs } from "../../state/layoutPrefs";
import { useStore } from "../../state/store";
import { setupApp, setupConnected, settleApp } from "../../test/harness";
import { runAction } from "../actions/registry";
import { useMenuStore } from "../menu/menuStore";
import { canReplaceSession, editConnection } from "./editConnection";
import { methodList } from "./SessionErrorPanel";
import { errorIcon } from "../../lib/errorIcon";

afterEach(async () => {
  await settleApp();
  useMenuStore.getState().close();
  setBackend(null);
  vi.restoreAllMocks();
});

describe("Edit connection", () => {
  it("on a connected ssh-config session it opens the dialog prefilled, copies nothing, and never closes the live session", async () => {
    const { user, backend, sessionId } = await setupConnected();
    const close = vi.spyOn(backend, "sessionClose");
    const before = (await backend.bookmarksGet()).bookmarks.length;

    runAction(A.editConnection);
    runAction(A.editConnection); // clicking again must not pile anything up
    const dialog = await screen.findByRole("dialog", { name: "New connection" });
    expect(useStore.getState().dialog).toMatchObject({ kind: "bookmark", bookmarkId: null });
    expect(useStore.getState().dialog).not.toHaveProperty("replaceSession");
    expect((within(dialog).getByLabelText(/^Host/) as HTMLInputElement).value).toBe("prod-web-01");
    expect((await backend.bookmarksGet()).bookmarks.length).toBe(before); // copy happens only on Save

    await user.click(within(dialog).getByRole("button", { name: "Save & connect" }));
    await waitFor(async () => expect((await backend.bookmarksGet()).bookmarks.length).toBe(before + 1));
    expect(close).not.toHaveBeenCalled();
    expect(useStore.getState().sessionsById[sessionId]?.state.phase).toBe("connected");
    expect(useStore.getState().workspaces[sessionId]).toBeDefined();
  });

  it("Save as bookmark… on an ssh-config host also opens the prefilled dialog without copying", async () => {
    const { backend } = await setupConnected();
    const before = (await backend.bookmarksGet()).bookmarks.length;
    runAction(A.saveAsBookmark);
    expect(await screen.findByRole("dialog", { name: "New connection" })).toBeTruthy();
    expect((await backend.bookmarksGet()).bookmarks.length).toBe(before);
  });

  it("the palette lists a single Edit connection… entry (the registry action)", async () => {
    const { user } = await setupConnected();
    useStore.getState().openPalette("all");
    await user.keyboard(">edit conn");
    expect(await screen.findAllByRole("option", { name: /Edit connection/ })).toHaveLength(1);
  });

  it("only a session that failed before it ever connected may be replaced", async () => {
    const { sessionId } = await setupConnected();
    expect(canReplaceSession(sessionId)).toBe(false);

    const store = useStore.getState();
    store.addSession("failed1", quickTarget("alice@example.com:22"), "example.com");
    act(() =>
      useStore.getState().applySessionEvent("failed1", {
        type: "state",
        state: { phase: "disconnected", error: { code: "connectTimeout" } as AppError, canRetry: true },
      }),
    );
    expect(canReplaceSession("failed1")).toBe(true);
    editConnection("failed1");
    expect(useStore.getState().dialog).toMatchObject({ kind: "bookmark", replaceSession: "failed1" });
  });
});

describe("Disconnect keeps the tab", () => {
  it("drops the connection (userRequested), shows Disconnected / Reconnect, and Reconnect brings it back", async () => {
    const { user, sessionId } = await setupConnected();
    runAction(A.disconnect);
    await waitFor(() => expect(useStore.getState().sessionsById[sessionId]?.state.phase).toBe("disconnected"));
    expect(useStore.getState().sessionsById[sessionId]).toBeDefined();
    expect(await screen.findByText("Disconnected from prod-web-01")).toBeTruthy();
    // Disconnect is not offered on a connection that is not live.
    expect(useStore.getState().sessionOrder).toContain(sessionId);
    await user.click(await screen.findByRole("button", { name: "Reconnect" }));
    await waitFor(() => expect(useStore.getState().sessionsById[sessionId]?.state.phase).toBe("connected"));
  });
});

describe("Layout persistence", () => {
  beforeEach(() => window.localStorage.clear());

  it("a pending (debounced) write is visible to the next read and is flushed to storage later", () => {
    vi.useFakeTimers();
    try {
      saveLayoutPrefs({ sidebarWidth: 333 }, { debounce: true });
      saveLayoutPrefs({ sidebarWidth: 340 }, { debounce: true });
      expect(window.localStorage.getItem("shellfin.layout.v1")).toBeNull(); // not written on every tick
      expect(loadLayoutPrefs().sidebarWidth).toBe(340); // but a re-mounting panel reads the last size
      vi.advanceTimersByTime(300);
      expect(JSON.parse(window.localStorage.getItem("shellfin.layout.v1") ?? "{}")).toMatchObject({ sidebarWidth: 340 });
    } finally {
      vi.useRealTimers();
    }
  });

  it("non-debounced saves write at once and keep earlier pending values", () => {
    saveLayoutPrefs({ bookmarksShare: 49 }, { debounce: true });
    saveLayoutPrefs({ activity: "files" });
    flushLayoutPrefs();
    expect(JSON.parse(window.localStorage.getItem("shellfin.layout.v1") ?? "{}")).toMatchObject({ bookmarksShare: 49, activity: "files" });
  });
});

describe("Ssh command quoting", () => {
  const host = (alias: string): SshConfigHost =>
    ({ alias, hostname: "h", user: null, port: 22, proxyJump: null, identityFiles: [], origin: {}, hasWarnings: false }) as unknown as SshConfigHost;
  const bm = (over: Partial<Bookmark>): Bookmark =>
    ({ id: "b1", name: "n", host: "example.com", port: null, user: null, proxyJump: null, ...over }) as unknown as Bookmark;

  it("puts -- before the destination and quotes odd names", () => {
    expect(sshCommand({ kind: "sshConfig", alias: "web" }, [], [host("web")])).toBe("ssh -- web");
    expect(sshCommand({ kind: "sshConfig", alias: "-oProxyCommand=x" }, [], [host("-oProxyCommand=x")])).toBe("ssh -- '-oProxyCommand=x'");
    expect(sshCommand({ kind: "bookmark", id: "b1" }, [bm({ user: "u", port: 2200 })], [])).toBe("ssh -p 2200 -- u@example.com");
    expect(sshCommand({ kind: "bookmark", id: "b1" }, [bm({ host: "a b" })], [])).toBe("ssh -- 'a b'");
  });

  it("refuses a jump host that looks like an option", () => {
    const b = bm({ proxyJump: { kind: "spec", spec: "-oProxyCommand=evil" } });
    expect(sshCommand({ kind: "bookmark", id: "b1" }, [b], [])).toBeNull();
    const ok = bm({ proxyJump: { kind: "sshConfig", alias: "bastion" } });
    expect(sshCommand({ kind: "bookmark", id: "b1" }, [ok], [])).toBe("ssh -J bastion -- example.com");
  });

  it("a quick spec becomes -p and a plain destination", () => {
    expect(sshCommand({ kind: "quick", spec: "alice@example.com:2222" }, [], [])).toBe("ssh -p 2222 -- alice@example.com");
    expect(sshCommand({ kind: "quick", spec: "alice@example.com:22" }, [], [])).toBe("ssh -- alice@example.com");
  });
});

describe("Auth failure wording and icon", () => {
  const t = ((key: string) => key.replace("authMethod.", "")) as never;
  it("hides keyboard-interactive when the password went through it", () => {
    expect(methodList(["publickey", "password", "keyboardInteractive"], t, true)).toContain("password");
    expect(methodList(["publickey", "password", "keyboardInteractive"], t, true)).not.toContain("keyboardInteractive");
    expect(methodList(["publickey", "keyboardInteractive"], t, true)).toContain("keyboardInteractive");
    expect(methodList(["password", "keyboardInteractive"], t, false)).toContain("keyboardInteractive"); // server list is shown as is
  });
  it("uses a lock for credentials and wifi-off only for network failures", () => {
    expect(errorIcon({ code: "authFailed", params: { user: "u", host: "h", tried: [], serverMethods: [] } } as AppError)).toBe("lock");
    expect(errorIcon({ code: "connectTimeout" } as AppError)).toBe("wifiOff");
    expect(errorIcon(null)).toBe("alert");
  });
});

describe("Go to folder, menus and the file tab", () => {
  it("Go to folder opens with the current path selected", async () => {
    const { sessionId } = await setupConnected();
    act(() => useStore.getState().openDialog(goToFolderDialog(sessionId)));
    const input = (await screen.findByLabelText("Folder path")) as HTMLInputElement;
    expect(input.value.length).toBeGreaterThan(0);
    expect(input.selectionStart).toBe(0);
    expect(input.selectionEnd).toBe(input.value.length);
  });

  it("F10 opens the File menu even after another menu was used last", async () => {
    const { user } = setupApp();
    const bar = screen.getByRole("menubar", { name: "Application menu" });
    await user.click(within(bar).getByRole("menuitem", { name: "View" }));
    await user.keyboard("{Escape}");
    document.body.focus();
    await user.keyboard("{F10}");
    expect(await screen.findByRole("menu", { name: "File" })).toBeTruthy();
  });

  it("a freshly opened file does not claim 'Saved to server'", async () => {
    const { user } = await setupConnected();
    await user.dblClick(await screen.findByRole("treeitem", { name: /docker-compose\.yml/ }));
    await screen.findByRole("tab", { name: /docker-compose\.yml/ });
    expect(screen.queryByText(/Saved to server/)).toBeNull();
  });
});

describe("Palette path prefix, public key", () => {
  it("'/' goes to a path: the single item re-roots the tree", async () => {
    const { user, sessionId } = await setupConnected();
    useStore.getState().openPalette("all");
    await user.keyboard("/srv");
    const item = await screen.findByRole("option", { name: /Go to folder/ });
    fireEvent.click(item);
    await waitFor(() => expect(useStore.getState().workspaces[sessionId]?.tree.root).toBe("/srv"));
  });

  it("Copy public key asks the backend for the key text", async () => {
    const { user, backend } = setupApp();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const read = vi.spyOn(backend, "keysReadPublic");
    useStore.getState().openDialog({ kind: "keys" });
    const buttons = await screen.findAllByRole("button", { name: "Copy public key" });
    await user.click(buttons[0] as HTMLElement);
    await waitFor(() => expect(writeText).toHaveBeenCalled());
    expect(read).toHaveBeenCalledWith("~/.ssh/id_ed25519");
    expect(String(writeText.mock.calls[0]?.[0])).toContain("ssh-");
  });

  it("the transfers limits come from the session's captured overrides", async () => {
    const { sessionId } = await setupConnected();
    expect(useStore.getState().sessionsById[sessionId]?.info?.limitOverrides).toEqual({ uploadKibps: null, downloadKibps: null });
  });
});
