import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { IpcError, setBackend } from "../../ipc";
import { MOCK_SECRETS } from "../../ipc/mock/scenarios";
import type { AppError } from "../../ipc/types";
import { quickTarget, sshConfigTarget } from "../../lib/targets";
import { useStore } from "../../state/store";
import { setupApp } from "../../test/harness";
import { handleQuitRequested } from "./quitFlow";
import { openSession } from "./sessionActions";

afterEach(() => {
  setBackend(null);
  vi.restoreAllMocks();
});

function dirty() {
  useStore.getState().addBuffer({
    id: "q:/x", sessionId: "q", path: "/x", load: "ready", dirty: true, encoding: "utf-8", eol: "lf", etag: null,
    savedAtMs: null,
  });
}

function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

describe("A late close never closes the quit dialog", () => {
  it("bookmark dialog: save finishes after the quit dialog replaced the form", async () => {
    const { user, backend } = setupApp();
    const gate = deferred<void>();
    const realSave = backend.bookmarkSave.bind(backend);
    vi.spyOn(backend, "bookmarkSave").mockImplementation(async (input) => {
      await gate.promise;
      return realSave(input);
    });
    act(() => useStore.getState().openDialog({ kind: "bookmark", bookmarkId: null }));
    const dialog = await screen.findByRole("dialog", { name: "New connection" });
    await user.type(within(dialog).getByLabelText("Name (optional)", { selector: "input" }), "late");
    await user.type(within(dialog).getByLabelText("Host", { selector: "input" }), "late.example.com");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));

    dirty();
    await act(async () => handleQuitRequested());
    expect(useStore.getState().dialog).toEqual({ kind: "quit" });
    gate.resolve();
    await waitFor(() => expect(useStore.getState().bookmarks?.bookmarks.some((b) => b.name === "late")).toBe(true));
    await act(async () => undefined);
    expect(useStore.getState().dialog).toEqual({ kind: "quit" });
    expect(screen.getByRole("alertdialog", { name: "Unsaved changes" })).toBeTruthy();
  });

  it("settings dialog: same race", async () => {
    const { user, backend } = setupApp();
    const gate = deferred<void>();
    const real = backend.settingsUpdate.bind(backend);
    vi.spyOn(backend, "settingsUpdate").mockImplementation(async (s) => {
      await gate.promise;
      return real(s);
    });
    await user.click(screen.getByRole("button", { name: "Settings" }));
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    dirty();
    await act(async () => handleQuitRequested());
    gate.resolve();
    await act(async () => {
      await new Promise((r) => setTimeout(r, 30));
    });
    expect(useStore.getState().dialog).toEqual({ kind: "quit" });
  });

  it("closeDialog(kind) only closes that kind", () => {
    setupApp();
    useStore.getState().openDialog({ kind: "keys" });
    useStore.getState().closeDialog("settings");
    expect(useStore.getState().dialog).toEqual({ kind: "keys" });
    useStore.getState().closeDialog("keys");
    expect(useStore.getState().dialog).toBeNull();
  });
});

describe("A failed answer re-enables the prompt dialog", () => {
  it("secret prompt: error notice, buttons usable again, Escape cancels for real", async () => {
    const { user, backend } = setupApp({ scenario: "passwordRetry" });
    const send = vi.spyOn(backend, "sessionAnswerPrompt");
    send.mockRejectedValueOnce(new IpcError({ code: "internal", detail: "ipc down" }));
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    await openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
    await user.type(await screen.findByLabelText("Password", { selector: "input" }), "x");
    await user.click(screen.getByRole("button", { name: "Connect" }));
    await waitFor(() => expect(screen.getByRole("region", { name: "Notifications" }).textContent).toContain("An internal error occurred."));
    expect((within(screen.getByRole("dialog", { name: "Password" })).getByRole("button", { name: "Cancel" }) as HTMLButtonElement).disabled).toBe(false);
    await user.keyboard("{Escape}");
    await waitFor(() => expect(send).toHaveBeenCalledTimes(2));
    expect(send.mock.calls[1]?.[2]).toEqual({ kind: "cancel" });
  });

  it("host key dialog: Trust works again after a failed send", async () => {
    const { user, backend } = setupApp({ scenario: "hostKeyUnknown" });
    const send = vi.spyOn(backend, "sessionAnswerPrompt");
    send.mockRejectedValueOnce(new IpcError({ code: "internal" }));
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    await openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
    const trust = await screen.findByRole("button", { name: "Trust and connect" });
    await user.click(trust);
    await waitFor(() => expect((screen.getByRole("button", { name: "Trust and connect" }) as HTMLButtonElement).disabled).toBe(false));
    await user.click(screen.getByRole("button", { name: "Trust and connect" }));
    await waitFor(() => expect(Object.values(useStore.getState().sessionsById)[0]?.state.phase).toBe("connected"));
  });
});

describe("Groups only exist for saved bookmarks", () => {
  async function open(user: ReturnType<typeof setupApp>["user"]) {
    act(() => useStore.getState().openDialog({ kind: "bookmark", bookmarkId: null }));
    const dialog = await screen.findByRole("dialog", { name: "New connection" });
    await user.type(within(dialog).getByLabelText("Name (optional)", { selector: "input" }), "g");
    await user.type(within(dialog).getByLabelText("Host", { selector: "input" }), "g.example.com");
    return dialog;
  }

  it("connect-only: a chosen new group is ignored and no group is created", async () => {
    const { user, backend } = setupApp();
    const groupSave = vi.spyOn(backend, "groupSave");
    const dialog = await open(user);
    await user.selectOptions(within(dialog).getByLabelText("Group"), "New group…");
    await user.click(within(dialog).getByRole("button", { name: "Connect" })); // no "Enter a group name." block
    await waitFor(() => expect(useStore.getState().sessionOrder).toHaveLength(1));
    expect(groupSave).not.toHaveBeenCalled();
    expect(useStore.getState().bookmarks?.groups).toHaveLength(0);
  });

  it("read-only store: Connect works without touching groups or bookmarks", async () => {
    const { user, backend } = setupApp();
    useStore.getState().setBookmarks({ ...(useStore.getState().bookmarks ?? { bookmarks: [], groups: [] }), readOnly: true });
    const groupSave = vi.spyOn(backend, "groupSave");
    const save = vi.spyOn(backend, "bookmarkSave");
    const dialog = await open(user);
    expect((within(dialog).getByLabelText("Group") as HTMLSelectElement).disabled).toBe(true);
    await user.click(within(dialog).getByRole("button", { name: "Connect" }));
    await waitFor(() => expect(useStore.getState().sessionOrder).toHaveLength(1));
    expect(groupSave).not.toHaveBeenCalled();
    expect(save).not.toHaveBeenCalled();
  });
});

describe("The copy button copies the backend string verbatim", () => {
  const changed = (removeHint: string | null): AppError =>
    ({
      code: "hostKeyChanged",
      params: { host: "h", port: 22, keyType: "ssh-ed25519", fingerprint: "SHA256:x", knownHostsFile: "~/.ssh/known_hosts", line: 3, removeHint, removeHintShell: "posix" },
    }) as AppError;

  function show(error: AppError) {
    useStore.getState().addSession("s1", quickTarget("a@h:22"), "h");
    act(() => useStore.getState().applySessionEvent("s1", { type: "state", state: { phase: "disconnected", error, canRetry: false } }));
  }

  it("a long (but sane) hint is displayed and copied in full", async () => {
    const { user } = setupApp();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const hint = `ssh-keygen -R 'h' -f '/${"d/".repeat(600)}known_hosts'`; // ~1.2k chars, past the old 1024 cut
    expect(hint.length).toBeGreaterThan(1024);
    show(changed(hint));
    const alert = await screen.findByRole("alert");
    expect(alert.querySelector("code")?.textContent).toBe(hint);
    await user.click(within(alert).getByRole("button", { name: "Copy" }));
    expect(writeText).toHaveBeenCalledWith(hint);
  });

  it("an absurdly long hint is not offered at all (instruction only)", async () => {
    setupApp();
    show(changed(`ssh-keygen -R 'h' -f '${"x".repeat(3000)}'`));
    const alert = await screen.findByRole("alert");
    expect(alert.querySelector("code")).toBeNull();
    expect(within(alert).queryByRole("button", { name: "Copy" })).toBeNull();
    expect(alert.textContent).toContain("remove the old entry from that file");
  });
});

describe("One re-dial after a connection loss during authentication", () => {
  it("passwordRetry: promptClosed{connectionLost} -> re-dial -> the password is asked again -> connected", async () => {
    const { user, backend } = setupApp({ scenario: "passwordRetry" });
    await openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
    await screen.findByRole("dialog", { name: "Password" });
    const id = useStore.getState().sessionOrder[0] ?? "";
    act(() => backend.dropPendingPrompt(id, "connectionLost"));
    await waitFor(() => expect(useStore.getState().prompts[id]?.id).toBe("prompt-2"));
    expect(Object.values(useStore.getState().sessionsById)[0]?.state.phase).not.toBe("disconnected");
    await user.type(await screen.findByLabelText("Password", { selector: "input" }), MOCK_SECRETS.password);
    await user.click(screen.getByRole("button", { name: "Connect" }));
    await waitFor(() => expect(Object.values(useStore.getState().sessionsById)[0]?.state.phase).toBe("connected"));
  });

  it("kbdInteractive2fa: the cached password is replayed, only the code is asked again", async () => {
    const { user, backend } = setupApp({ scenario: "kbdInteractive2fa" });
    await openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
    await user.type(await screen.findByLabelText("Password", { selector: "input" }), MOCK_SECRETS.password);
    await user.click(screen.getByRole("button", { name: "Connect" }));
    await screen.findByLabelText("Verification code:");
    const id = useStore.getState().sessionOrder[0] ?? "";
    act(() => backend.dropPendingPrompt(id, "connectionLost"));
    await waitFor(() => expect(useStore.getState().prompts[id]?.kind).toBe("keyboardInteractive"));
    expect(screen.queryByLabelText("Password", { selector: "input" })).toBeNull(); // not asked again
    await user.type(await screen.findByLabelText("Verification code:"), MOCK_SECRETS.verificationCode);
    await user.click(screen.getByRole("button", { name: "Continue" }));
    await waitFor(() => expect(Object.values(useStore.getState().sessionsById)[0]?.state.phase).toBe("connected"));
  });

  it("a second loss ends in disconnected (retryable)", async () => {
    const { backend } = setupApp({ scenario: "passwordRetry" });
    await openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
    await screen.findByRole("dialog", { name: "Password" });
    const id = useStore.getState().sessionOrder[0] ?? "";
    act(() => backend.dropPendingPrompt(id, "connectionLost"));
    await waitFor(() => expect(useStore.getState().prompts[id]?.id).toBe("prompt-2"));
    act(() => backend.dropPendingPrompt(id, "connectionLost"));
    await waitFor(() => expect(Object.values(useStore.getState().sessionsById)[0]?.state.phase).toBe("disconnected"));
    expect((await screen.findByRole("alert")).textContent).toContain("The connection was lost.");
    expect(screen.getByRole("button", { name: "Try again" })).toBeTruthy();
  });
});
