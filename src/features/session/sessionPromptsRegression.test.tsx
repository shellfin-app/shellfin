import { act, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { bootstrap } from "../../bootstrap";
import { App } from "../../App";
import { IpcError, setBackend } from "../../ipc";
import { makeBootstrap } from "../../ipc/mock/fixtures";
import { MockBackend } from "../../ipc/mock/mockBackend";
import { MOCK_SECRETS } from "../../ipc/mock/scenarios";
import type { AppError, BookmarkInput, SessionEvent, SessionInfo } from "../../ipc/types";
import { cancelAnswer } from "../../lib/answers";
import { bookmarkTarget, quickTarget, sshConfigTarget } from "../../lib/targets";
import { resetStore, useStore } from "../../state/store";
import { setupApp } from "../../test/harness";
import { refreshBookmarks } from "../hosts/hostActions";
import { setPromptInputGuardMs } from "./promptGuard";
import { handleQuitRequested } from "./quitFlow";
import { answerPrompt, openSession, startProbe } from "./sessionActions";

afterEach(() => {
  setBackend(null);
  vi.restoreAllMocks();
});

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
/** Polls until `pred` holds (a fixed sleep flakes when the machine is busy). */
const until = (pred: () => boolean) => waitFor(() => expect(pred()).toBe(true), { timeout: 5000 });
const lastPhase = (events: SessionEvent[]) => {
  const last = [...events].reverse().find((e) => e.type === "state");
  return last?.type === "state" ? last.state.phase : null;
};
const RLO = String.fromCodePoint(0x202e);
const hop = { index: 0, label: "prod", user: "alice", host: "example.com", port: 22 };

function dirtyBuffer(sessionId: string) {
  useStore.getState().addBuffer({
    id: `${sessionId}:/x`, sessionId, path: "/x", load: "ready", dirty: true, encoding: "utf-8", eol: "lf", etag: null,
    savedAtMs: null,
  });
}

describe("The quit confirmation always wins", () => {
  it("cannot be replaced by dialogs, the palette or shortcuts; is reopened if it was lost", async () => {
    const { user } = setupApp();
    useStore.getState().addSession("s1", sshConfigTarget("prod-web-01"), "prod-web-01");
    useStore.getState().addSession("s2", sshConfigTarget("prod-web-02"), "prod-web-02");
    dirtyBuffer("s1");
    await act(async () => handleQuitRequested());
    expect(await screen.findByRole("alertdialog", { name: "Unsaved changes" })).toBeTruthy();

    await user.keyboard("{Control>}n{/Control}");
    await user.keyboard("{Control>}k{/Control}");
    await user.keyboard("{Control>}1{/Control}");
    act(() => useStore.getState().openDialog({ kind: "keys" }));
    act(() => useStore.getState().openPalette("all"));
    expect(useStore.getState().dialog).toEqual({ kind: "quit" });
    expect(useStore.getState().palette).toBeNull();
    expect(useStore.getState().activeSessionId).toBe("s2"); // Mod+1 did not switch tabs under the modal

    // Defence in depth: even if the dialog slot was lost, the next close request brings it back.
    act(() => useStore.setState({ dialog: null }));
    await act(async () => handleQuitRequested());
    expect(useStore.getState().dialog).toEqual({ kind: "quit" });
    expect(useStore.getState().quit).toBe("confirming");
  });

  it("quit takes precedence over a pending prompt dialog and the prompt waits behind it", async () => {
    setupApp({ scenario: "passwordRetry" });
    await openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
    await screen.findByRole("dialog", { name: "Password" });
    dirtyBuffer("other");
    await act(async () => handleQuitRequested());
    expect(await screen.findByRole("alertdialog", { name: "Unsaved changes" })).toBeTruthy();
    expect(screen.queryByRole("dialog", { name: "Password" })).toBeNull();
  });
});

describe("Bookmark flags are refetched after connected", () => {
  it("a remembered password shows up as passwordSaved without reopening anything", async () => {
    const { user, backend } = setupApp({ scenario: "passwordRetry" });
    const bookmark = await backend.bookmarkSave({
      id: null, name: "vault", host: "vault.example.com", port: null, user: "alice", auth: { method: "password" },
      proxyJump: null, groupId: null, tags: [], startDir: null, useSshConfig: true,
    });
    await refreshBookmarks();
    expect(useStore.getState().bookmarks?.bookmarks.find((b) => b.id === bookmark.id)?.auth).toEqual({
      method: "password", passwordSaved: false,
    });
    await openSession(bookmarkTarget(bookmark.id), "vault");
    await screen.findByRole("dialog", { name: "Password" });
    await user.type(screen.getByLabelText("Password", { selector: "input" }), MOCK_SECRETS.password);
    await user.click(screen.getByRole("checkbox", { name: "Save in system keychain" }));
    await user.click(screen.getByRole("button", { name: "Connect" }));
    await waitFor(() =>
      expect(useStore.getState().bookmarks?.bookmarks.find((b) => b.id === bookmark.id)?.auth).toEqual({
        method: "password", passwordSaved: true,
      }),
    );
  });
});

describe("Startup warnings and read-only storage are visible", () => {
  it("bootstrap warnings become notices; a read-only store disables add/edit/delete with a hint", async () => {
    resetStore();
    const backend = new MockBackend();
    const base = makeBootstrap(Date.now());
    vi.spyOn(backend, "appFrontendReady").mockResolvedValue({
      ...base,
      warnings: [{ code: "storageReadOnly" }, { code: "storageCorrupt", params: { file: "bookmarks.json", quarantinedAs: "bookmarks.json.bad" } }],
      bookmarks: { ...base.bookmarks, readOnly: true },
    });
    setBackend(backend);
    await bootstrap();
    expect(useStore.getState().notices.map((n) => n.error?.code)).toEqual(["storageReadOnly", "storageCorrupt"]);

    render(<App />);
    const notices = screen.getByRole("region", { name: "Notifications" });
    expect(notices.textContent).toContain("Settings and bookmarks are read-only.");
    expect(notices.textContent).toContain("bookmarks.json was damaged");
    expect((screen.getByRole("button", { name: "Add bookmark" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Edit nas" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Delete nas" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Copy bastion to my bookmarks" }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText(/Bookmarks are read-only right now/)).toBeTruthy();
  });

  it("the new-connection dialog still connects but cannot save when read-only", async () => {
    const { user } = setupApp();
    useStore.getState().setBookmarks({ ...(useStore.getState().bookmarks ?? { bookmarks: [], groups: [] }), readOnly: true });
    act(() => useStore.getState().openDialog({ kind: "bookmark", bookmarkId: null }));
    const dialog = await screen.findByRole("dialog", { name: "New connection" });
    expect(within(dialog).queryByRole("button", { name: "Save" })).toBeNull();
    expect(within(dialog).queryByRole("button", { name: "Save & connect" })).toBeNull();
    expect(within(dialog).getByRole("button", { name: "Connect" })).toBeTruthy();
    expect(within(dialog).getByText(/read-only right now/)).toBeTruthy();
    void user;
  });
});

describe("Cancelling Test connection closes the probe session", () => {
  it("a cancel before session_open resolved still closes the backend session; no prompt appears later", async () => {
    resetStore();
    const backend = new MockBackend({ scenario: "hostKeyUnknown", latencyMs: 40 });
    setBackend(backend);
    const close = vi.spyOn(backend, "sessionClose");
    const bookmark: BookmarkInput = {
      id: null, name: "t", host: "t.example.com", port: null, user: null, auth: { method: "auto" }, proxyJump: null,
      groupId: null, tags: [], startDir: null, useSshConfig: true,
    };
    const probe = startProbe({ bookmark, password: null });
    probe.cancel();
    expect(await probe.result).toEqual({ ok: false, error: null });
    await sleep(250);
    expect(close).toHaveBeenCalledTimes(1);
    expect(useStore.getState().prompts).toEqual({});
    expect(Object.keys(useStore.getState().sessionsById)).toHaveLength(0);
  });
});

describe("Late promptNotFound / duplicate answers", () => {
  it("a stale promptNotFound does not clear a newer prompt", async () => {
    const { backend } = setupApp();
    useStore.getState().addSession("s1", quickTarget("a@b:22"), "b");
    const prompt = (id: string): SessionEvent => ({
      type: "prompt",
      prompt: { kind: "password", id, hop, user: "a", host: "b", attemptsLeft: 2, lastError: "passwordIncorrect", canRemember: false, savedPasswordAvailable: false, serverLabel: null },
    });
    useStore.getState().applySessionEvent("s1", prompt("p2"));
    vi.spyOn(backend, "sessionAnswerPrompt").mockRejectedValue(new IpcError({ code: "promptNotFound" }));
    await answerPrompt("s1", "p1", cancelAnswer()); // late answer for the OLD prompt
    expect(useStore.getState().prompts["s1"]?.id).toBe("p2");
    await answerPrompt("s1", "p2", cancelAnswer()); // the real current prompt is gone
    expect(useStore.getState().prompts["s1"]).toBeUndefined();
  });

  it("Escape twice sends only one answer", async () => {
    const { user, backend } = setupApp({ scenario: "passwordRetry" });
    const answer = vi.spyOn(backend, "sessionAnswerPrompt");
    await openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
    await screen.findByRole("dialog", { name: "Password" });
    await user.keyboard("{Escape}{Escape}");
    await sleep(50);
    expect(answer).toHaveBeenCalledTimes(1);
  });
});

describe("No frontend-built ssh-keygen command", () => {
  const changed = (extra: object = {}): AppError =>
    ({
      code: "hostKeyChanged",
      params: {
        host: "h", port: 22, keyType: "ssh-ed25519", fingerprint: "SHA256:x", knownHostsFile: "~/.ssh/known_hosts", line: 3,
        removeHint: null, removeHintShell: "posix", ...extra,
      },
    }) as AppError;

  function showError(error: AppError) {
    useStore.getState().addSession("s1", quickTarget("a@h:22"), "h");
    act(() => useStore.getState().applySessionEvent("s1", { type: "state", state: { phase: "disconnected", error, canRetry: false } }));
  }

  it("removeHint null: facts and an instruction only", async () => {
    setupApp();
    showError(changed());
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("~/.ssh/known_hosts:3");
    expect(alert.querySelector("code")).toBeNull();
    expect(alert.textContent).not.toContain("ssh-keygen -R");
  });

  it("shows the backend hint labelled by shell (sanitised for display) and copies the backend string verbatim", async () => {
    const { user } = setupApp();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    showError(changed({ removeHint: `ssh-keygen -R 'h'${RLO}x` }));
    const alert = await screen.findByRole("alert");
    expect(within(alert).getByText("Run in a terminal:")).toBeTruthy();
    expect(alert.querySelector("code")?.textContent).toBe("ssh-keygen -R 'h'⟨U+202E⟩x");
    await user.click(within(alert).getByRole("button", { name: "Copy" }));
    expect(writeText).toHaveBeenCalledWith(`ssh-keygen -R 'h'${RLO}x`);
  });
});

describe("The mock behaves like the real backend", () => {
  it("promptNotFound for an unknown session or prompt", async () => {
    const backend = new MockBackend();
    await expect(backend.sessionAnswerPrompt("nope", "p", cancelAnswer())).rejects.toMatchObject({ error: { code: "promptNotFound" } });
    const events: SessionEvent[] = [];
    const id = await backend.openSession({ target: sshConfigTarget("prod-web-01"), probeOnly: false }, (e) => events.push(e));
    await until(() => lastPhase(events) === "connected");
    await expect(backend.sessionAnswerPrompt(id, "wrong", cancelAnswer())).rejects.toMatchObject({ error: { code: "promptNotFound" } });
  });

  it("cancel and host-key reject are retryable; fatal host-key failures are not", async () => {
    const run = async (scenario: "passwordRetry" | "hostKeyUnknown" | "hostKeyChanged", answer?: Parameters<MockBackend["sessionAnswerPrompt"]>[2]) => {
      const backend = new MockBackend({ scenario });
      const events: SessionEvent[] = [];
      const id = await backend.openSession({ target: sshConfigTarget("prod-web-01"), probeOnly: false }, (e) => events.push(e));
      // hostKeyChanged fails without any prompt; the others stop at one.
      await until(() => events.some((e) => e.type === "prompt") || lastPhase(events) === "disconnected");
      const prompt = events.find((e) => e.type === "prompt");
      if (prompt?.type === "prompt" && answer) await backend.sessionAnswerPrompt(id, prompt.prompt.id, answer);
      await until(() => lastPhase(events) === "disconnected");
      backend.dispose();
      const last = [...events].reverse().find((e) => e.type === "state");
      return last?.type === "state" && last.state.phase === "disconnected" ? last.state.canRetry : null;
    };
    expect(await run("passwordRetry", cancelAnswer())).toBe(true);
    expect(await run("hostKeyUnknown", { kind: "hostKey", accept: false, remember: false })).toBe(true);
    expect(await run("hostKeyChanged")).toBe(false);
  });

  it("retry after cancel works from the UI", async () => {
    const { user } = setupApp({ scenario: "passwordRetry" });
    await openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
    await user.click(within(await screen.findByRole("dialog", { name: "Password" })).getByRole("button", { name: "Cancel" }));
    await user.click(await screen.findByRole("button", { name: "Try again" }));
    await user.type(await screen.findByLabelText("Password", { selector: "input" }), MOCK_SECRETS.password);
    await user.click(screen.getByRole("button", { name: "Connect" }));
    await waitFor(() => expect(Object.values(useStore.getState().sessionsById)[0]?.state.phase).toBe("connected"));
  });

  it("session_close with a pending prompt emits promptClosed{cancelled} and state closed", async () => {
    const backend = new MockBackend({ scenario: "passwordRetry" });
    const events: SessionEvent[] = [];
    const id = await backend.openSession({ target: sshConfigTarget("prod-web-01"), probeOnly: false }, (e) => events.push(e));
    await until(() => events.some((e) => e.type === "prompt"));
    await backend.sessionClose(id, null);
    expect(events.some((e) => e.type === "promptClosed" && e.reason === "cancelled")).toBe(true);
    expect(events.at(-1)).toEqual({ type: "state", state: { phase: "closed" } });
  });

  it("probeOnly: connected is followed by closed", async () => {
    const backend = new MockBackend();
    const phases: string[] = [];
    await backend.openSession({ target: sshConfigTarget("prod-web-01"), probeOnly: true }, (e) => {
      if (e.type === "state") phases.push(e.state.phase);
    });
    await until(() => phases.at(-1) === "closed");
    expect(phases.slice(-2)).toEqual(["connected", "closed"]);
  });

  it("prompt timeout: the dialog disappears and the error panel offers a retry", async () => {
    setupApp({ scenario: "passwordRetry", promptTimeoutMs: 60 });
    await openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
    await screen.findByRole("dialog", { name: "Password" });
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Password" })).toBeNull());
    expect((await screen.findByRole("alert")).textContent).toContain("The prompt timed out.");
    expect(screen.getByRole("button", { name: "Try again" })).toBeTruthy();
  });

  it("connection lost during a prompt closes it, re-dials once, and a second loss is reported", async () => {
    const { backend } = setupApp({ scenario: "passwordRetry" });
    await openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
    await screen.findByRole("dialog", { name: "Password" });
    const id = useStore.getState().sessionOrder[0] ?? "";
    act(() => backend.dropPendingPrompt(id, "connectionLost"));
    await waitFor(() => expect(useStore.getState().prompts[id]?.id).toBe("prompt-2")); // re-dial: asked again
    act(() => backend.dropPendingPrompt(id, "connectionLost"));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Password" })).toBeNull());
    expect((await screen.findByRole("alert")).textContent).toContain("The connection was lost.");
  });

  it("drafts and quick connects never offer the keychain", async () => {
    setupApp({ scenario: "passwordRetry" });
    await openSession(quickTarget("alice@example.com:22"), "example.com");
    await screen.findByRole("dialog", { name: "Password" });
    expect(screen.queryByRole("checkbox", { name: "Save in system keychain" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Use saved password" })).toBeNull();
  });
});

describe("Retrying a failed Connect after a save does not duplicate", () => {
  it("switches to edit mode with the saved bookmark and group ids", async () => {
    const { user, backend } = setupApp();
    const open = vi.spyOn(backend, "openSession").mockRejectedValueOnce(new IpcError({ code: "internal", detail: "boom" }));
    const save = vi.spyOn(backend, "bookmarkSave");
    const saveGroup = vi.spyOn(backend, "groupSave");
    act(() => useStore.getState().openDialog({ kind: "bookmark", bookmarkId: null }));
    const dialog = await screen.findByRole("dialog", { name: "New connection" });
    await user.type(within(dialog).getByLabelText("Name (optional)", { selector: "input" }), "dup");
    await user.type(within(dialog).getByLabelText("Host", { selector: "input" }), "dup.example.com");
    await user.selectOptions(within(dialog).getByLabelText("Group"), "New group…");
    await user.type(within(dialog).getByLabelText("New group name"), "G");
    await user.click(within(dialog).getByRole("button", { name: "Save & connect" }));
    await waitFor(() => expect(within(dialog).getByText("An internal error occurred.")).toBeTruthy());

    await user.click(within(dialog).getByRole("button", { name: "Save & connect" }));
    await waitFor(() => expect(open).toHaveBeenCalledTimes(2));
    expect(save).toHaveBeenCalledTimes(2);
    expect(save.mock.calls[1]?.[0].id).toBe(save.mock.results[0] ? (await save.mock.results[0].value).id : null);
    expect(saveGroup).toHaveBeenCalledTimes(1);
    expect(useStore.getState().bookmarks?.bookmarks.filter((b) => b.name === "dup")).toHaveLength(1);
    expect(useStore.getState().bookmarks?.groups.filter((g) => g.name === "G")).toHaveLength(1);
  });
});

describe("Background prompts never take focus; stray keys never become answers", () => {
  it("a prompt of a background session waits behind a tab badge; typing does not reach it", async () => {
    const { user, backend } = setupApp({ scenario: "passwordRetry" });
    const answer = vi.spyOn(backend, "sessionAnswerPrompt");
    const pending = openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
    await pending;
    act(() => useStore.getState().setActiveSession(null)); // the user moved on (Home) before the prompt arrived
    await waitFor(() => expect(Object.keys(useStore.getState().prompts)).toHaveLength(1));

    expect(screen.queryByRole("dialog", { name: "Password" })).toBeNull();
    const tab = screen.getByRole("tab", { name: /prod-web-01/ });
    expect(within(tab).getByRole("img", { name: "Waiting for your answer" })).toBeTruthy();

    await user.keyboard("hunter2{Enter}");
    expect(answer).not.toHaveBeenCalled();

    await user.click(tab);
    const dialog = await screen.findByRole("dialog", { name: "Password" });
    expect(within(dialog).getByTestId("prompt-origin").textContent).toBe("Requested by tabprod-web-01");
    expect(within(tab).queryByRole("img", { name: "Waiting for your answer" })).toBeNull();
  });

  it("a prompt for the active session ignores the keyboard for ~500 ms after it opens", async () => {
    setPromptInputGuardMs(500);
    const { user, backend } = setupApp({ scenario: "passwordRetry" });
    const answer = vi.spyOn(backend, "sessionAnswerPrompt");
    await openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
    const dialog = await screen.findByRole("dialog", { name: "Password" });
    const input = within(dialog).getByLabelText("Password", { selector: "input" }) as HTMLInputElement;

    await user.keyboard("typed-for-another-window{Enter}");
    expect(input.value).toBe("");
    expect(answer).not.toHaveBeenCalled();

    await sleep(600);
    await user.type(input, "ok");
    expect(input.value).toBe("ok");
  });

  it("a keyboard-interactive title keeps a fixed app-authored prefix around the server's name", async () => {
    setupApp();
    useStore.getState().addSession("s1", quickTarget("alice@example.com:22"), "example.com");
    act(() =>
      useStore.getState().applySessionEvent("s1", {
        type: "prompt",
        prompt: { kind: "keyboardInteractive", id: "k1", hop, name: "[sudo] password for alice", instructions: "", fields: [{ label: "x", echo: false }] },
      }),
    );
    expect(await screen.findByRole("dialog", { name: "Server prompt from example.com: [sudo] password for alice" })).toBeTruthy();
    expect(screen.getByTestId("prompt-origin").textContent).toContain("hop 1");
  });

  it("probe prompts do not pop up by themselves", async () => {
    const { user } = setupApp({ scenario: "hostKeyUnknown" });
    act(() => useStore.getState().openDialog({ kind: "bookmark", bookmarkId: null }));
    const dialog = await screen.findByRole("dialog", { name: "New connection" });
    await user.type(within(dialog).getByLabelText("Name (optional)", { selector: "input" }), "p");
    await user.type(within(dialog).getByLabelText("Host", { selector: "input" }), "p.example.com");
    await user.click(within(dialog).getByRole("button", { name: "Test connection" }));
    await within(dialog).findByText("The server is waiting for your answer.");
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });
});

describe("The host key dialog never defaults to trust", () => {
  it("focus starts on Cancel and Enter rejects", async () => {
    const { user, backend } = setupApp({ scenario: "hostKeyUnknown" });
    const answer = vi.spyOn(backend, "sessionAnswerPrompt");
    await openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
    const dialog = await screen.findByRole("alertdialog");
    expect(document.activeElement).toBe(within(dialog).getByRole("button", { name: "Cancel" }));
    await user.keyboard("{Enter}");
    await waitFor(() => expect(answer).toHaveBeenCalled());
    expect(answer.mock.calls[0]?.[2]).toEqual({ kind: "hostKey", accept: false, remember: false });
  });
});

describe("Remote names are escaped everywhere they are displayed", () => {
  const evil = `invoice${RLO}txt.exe`;
  const shown = "invoice⟨U+202E⟩txt.exe";

  function connectedSession() {
    const store = useStore.getState();
    store.addSession("s1", quickTarget("alice@example.com:22"), "example.com");
    const info: SessionInfo = {
      sessionId: "s1", label: "example.com", hops: [], auth: { method: "publickey", keyType: "ed25519", keySource: null },
      sftpAvailable: true, homeDir: "/home/alice", startDir: "/srv", remotePathStyle: "posix", remoteIdentity: null,
      limitOverrides: { uploadKibps: null, downloadKibps: null },
      color: null,
    };
    act(() => useStore.getState().applySessionEvent("s1", { type: "state", state: { phase: "connected", info } }));
    store.ensureWorkspace("s1");
  }

  it("file tree rows", async () => {
    setupApp();
    connectedSession();
    act(() => {
      useStore.getState().setTreeRoot("s1", "/srv");
      useStore.getState().setListing(
        "s1",
        "/srv",
        {
          path: "/srv",
          entries: [
            {
              name: evil,
              path: `/srv/${evil}`,
              kind: "file",
              linkTargetKind: null,
              size: 1,
              mtime: 1,
              permissions: 0,
              uid: 0,
              gid: 0,
              owner: null,
              group: null,
              lossyName: false,
            },
          ],
          truncated: false,
        },
        Date.now(),
      );
    });
    const row = await screen.findByRole("treeitem", { name: new RegExp(shown.replace(/[.+()]/g, "\\$&")) });
    expect(row.textContent).not.toContain(RLO);
  });

  it("editor tabs, close buttons and breadcrumbs", async () => {
    setupApp();
    connectedSession();
    const path = `/srv/${evil}`;
    act(() => {
      useStore.getState().addBuffer({
        id: "s1:b", sessionId: "s1", path, load: "loading", dirty: false, encoding: "utf-8", eol: "lf", etag: null,
        savedAtMs: null,
      });
      useStore.getState().openEditorTab("s1", "s1:b");
    });
    const tab = await screen.findByRole("tab", { name: new RegExp("invoice") });
    expect(tab.textContent).toContain(shown);
    expect(tab.textContent).not.toContain(RLO);
    expect(screen.getByRole("button", { name: `Close ${shown}` })).toBeTruthy();
    expect(document.querySelector(".breadcrumbs")?.textContent).toContain(shown);
    expect(document.querySelector(".breadcrumbs")?.textContent).not.toContain(RLO);
  });

  it("palette files mode", async () => {
    const { user } = setupApp();
    connectedSession();
    act(() => {
      useStore.getState().addBuffer({
        id: "s1:b", sessionId: "s1", path: `/srv/${evil}`, load: "loading", dirty: false, encoding: "utf-8", eol: "lf", etag: null,
        savedAtMs: null,
      });
      useStore.getState().openEditorTab("s1", "s1:b");
    });
    await user.click(screen.getByRole("button", { name: "Search" }));
    const palette = await screen.findByRole("dialog", { name: "Command palette" });
    expect(palette.textContent).toContain(shown);
    expect(palette.textContent).not.toContain(RLO);
  });
});
