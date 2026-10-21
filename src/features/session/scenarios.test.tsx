import { screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import { MOCK_SECRETS } from "../../ipc/mock/scenarios";
import { useStore } from "../../state/store";
import { firstPhase, firstSessionId, setupApp } from "../../test/harness";
import { openSession } from "./sessionActions";
import { sshConfigTarget } from "../../lib/targets";

afterEach(() => setBackend(null));

const connect = (alias = "prod-web-01") => openSession(sshConfigTarget(alias), alias);
const phase = () => firstPhase();

describe("Host key prompts (§5.3)", () => {
  it("hostKeyUnknown: shows the fingerprint, 'Trust and connect' connects with a known_hosts entry", async () => {
    const { user, backend } = setupApp({ scenario: "hostKeyUnknown" });
    const answer = vi.spyOn(backend, "sessionAnswerPrompt");
    await connect();
    const dialog = await screen.findByRole("alertdialog", { name: "First connection to 10.0.4.12" });
    expect(within(dialog).getByText("SHA256:nThbg6kXUpJWGl7E1IGOCspRomTxdCARLviKw6E5SY8")).toBeTruthy();
    expect(within(dialog).getByText("ssh-ed25519")).toBeTruthy();
    expect(within(dialog).getByText(/ssh-keygen -lf \/etc\/ssh\/ssh_host_ed25519_key\.pub/)).toBeTruthy();
    const remember = within(dialog).getByRole("checkbox", { name: "Add to ~/.ssh/known_hosts" }) as HTMLInputElement;
    expect(remember.checked).toBe(true);

    await user.click(within(dialog).getByRole("button", { name: "Trust and connect" }));
    await waitFor(() => expect(phase()).toBe("connected"));
    expect(answer).toHaveBeenCalledWith(expect.any(String), expect.any(String), { kind: "hostKey", accept: true, remember: true });
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("hostKeyUnknown: unchecking 'Add to known_hosts' trusts once; Cancel rejects and shows the error panel", async () => {
    const { user, backend } = setupApp({ scenario: "hostKeyUnknown" });
    const answer = vi.spyOn(backend, "sessionAnswerPrompt");
    await connect();
    let dialog = await screen.findByRole("alertdialog");
    await user.click(within(dialog).getByRole("checkbox"));
    await user.click(within(dialog).getByRole("button", { name: "Trust and connect" }));
    await waitFor(() => expect(phase()).toBe("connected"));
    expect(answer).toHaveBeenLastCalledWith(expect.any(String), expect.any(String), { kind: "hostKey", accept: true, remember: false });

    // A single click selects a bookmark row; a double click (or Enter) connects.
    await user.dblClick(screen.getByRole("button", { name: "Connect to prod-web-02" }));
    dialog = await screen.findByRole("alertdialog");
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("You declined the host key of 10.0.4.13:22"));
  });

  it("Escape rejects the host key (never accepts)", async () => {
    const { user, backend } = setupApp({ scenario: "hostKeyUnknown" });
    const answer = vi.spyOn(backend, "sessionAnswerPrompt");
    await connect();
    await screen.findByRole("alertdialog");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(answer).toHaveBeenCalled());
    expect(answer.mock.calls[0]?.[2]).toEqual({ kind: "hostKey", accept: false, remember: false });
  });

  it("hostKeyChanged: hard failure panel with file:line, fingerprint and the backend-built, shell-labelled removal hint; no override", async () => {
    const { user } = setupApp({ scenario: "hostKeyChanged" });
    await connect();
    const alert = await screen.findByRole("alert");
    await waitFor(() => expect(alert.textContent).toContain("has changed"));
    expect(alert.textContent).toContain("~/.ssh/known_hosts:7");
    expect(alert.textContent).toContain("SHA256:nThbg6kXUpJWGl7E1IGOCspRomTxdCARLviKw6E5SY8");
    expect(alert.textContent).toContain("remove the old entry from that file");
    expect(within(alert).getByText("Run in PowerShell:")).toBeTruthy();
    expect(alert.querySelector("code")?.textContent).toBe("ssh-keygen -R '10.0.4.12' -f '~/.ssh/known_hosts'");
    expect(within(alert).queryByRole("button", { name: "Trust and connect" })).toBeNull();
    expect(within(alert).getByRole("button", { name: "Try again" })).toBeTruthy(); // after fixing known_hosts
    expect(within(alert).getByText("Offered by server")).toBeTruthy(); // the SHA256 row says whose key it is
    await user.click(within(alert).getByRole("button", { name: "Close tab" }));
    expect(useStore.getState().sessionOrder).toHaveLength(0);
  });

  it("hostKeyChangedNoHint: removeHint null -> only the file:line instruction, no command", async () => {
    setupApp({ scenario: "hostKeyChangedNoHint" });
    await connect();
    const alert = await screen.findByRole("alert");
    await waitFor(() => expect(alert.textContent).toContain("has changed"));
    expect(alert.textContent).toContain("~/.ssh/known_hosts:7");
    expect(alert.textContent).toContain("remove the old entry from that file");
    expect(alert.querySelector("code")).toBeNull();
    expect(within(alert).queryByRole("button", { name: "Copy" })).toBeNull();
  });

  it("hostKeyCaManaged: fails closed without a prompt", async () => {
    setupApp({ scenario: "hostKeyCaManaged" });
    await connect();
    await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("certificate authority"));
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });
});

describe("Secret prompts (§5.6)", () => {
  it("passphraseRetry: wrong passphrase shows the plural 'attempts left' error, the right one connects", async () => {
    const { user } = setupApp({ scenario: "passphraseRetry" });
    await connect();
    let dialog = await screen.findByRole("dialog", { name: "Key passphrase" });
    expect(within(dialog).getByText(String.raw`~\.ssh\id_ed25519`)).toBeTruthy();
    await user.type(within(dialog).getByLabelText("Passphrase"), "wrong");
    await user.click(within(dialog).getByRole("button", { name: "Unlock" }));
    await waitFor(() => expect(screen.getByRole("alert").textContent).toBe("Incorrect passphrase. Try again (2 attempts left)."));
    dialog = screen.getByRole("dialog", { name: "Key passphrase" });
    const input = within(dialog).getByLabelText("Passphrase") as HTMLInputElement;
    expect(input.value).toBe(""); // the secret does not survive a retry
    await user.type(input, "wrong2");
    await user.click(within(dialog).getByRole("button", { name: "Unlock" }));
    await waitFor(() => expect(screen.getByRole("alert").textContent).toBe("Incorrect passphrase. Try again (1 attempt left)."));
    await user.type(screen.getByLabelText("Passphrase"), MOCK_SECRETS.passphrase);
    await user.click(screen.getByRole("button", { name: "Unlock" }));
    await waitFor(() => expect(phase()).toBe("connected"));
  });

  it("passphraseRetry: 'Skip this key' continues (here: authentication fails), Cancel aborts", async () => {
    const { user } = setupApp({ scenario: "passphraseRetry" });
    await connect();
    await screen.findByRole("dialog", { name: "Key passphrase" });
    await user.click(screen.getByRole("button", { name: "Skip this key" }));
    await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("Authentication failed for"));
    expect(screen.getByRole("button", { name: "Try again" })).toBeTruthy(); // canRetry

    // retry runs the script again; now cancel
    await user.click(screen.getByRole("button", { name: "Try again" }));
    const passphrase = await screen.findByRole("dialog", { name: "Key passphrase" });
    await user.click(within(passphrase).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(phase()).toBe("disconnected"));
  });

  it("passphraseRetry: 'Add key to ssh-agent' is forwarded and the agent warning becomes a notice", async () => {
    const { user, backend } = setupApp({ scenario: "passphraseRetry" });
    const answer = vi.spyOn(backend, "sessionAnswerPrompt");
    await connect();
    await screen.findByRole("dialog", { name: "Key passphrase" });
    await user.click(screen.getByRole("checkbox", { name: "Add key to ssh-agent for this session" }));
    await user.type(screen.getByLabelText("Passphrase"), MOCK_SECRETS.passphrase);
    await user.click(screen.getByRole("button", { name: "Unlock" }));
    await waitFor(() => expect(phase()).toBe("connected"));
    expect(answer.mock.calls[0]?.[2]).toEqual({ kind: "passphrase", passphrase: MOCK_SECRETS.passphrase, addToAgent: true });
    expect(screen.getByRole("region", { name: "Notifications" }).textContent).toContain("doesn't support temporary keys");
  });

  it("passwordRetry: wrong password, then 'Use saved password'", async () => {
    const { user, backend } = setupApp({ scenario: "passwordRetry" });
    const answer = vi.spyOn(backend, "sessionAnswerPrompt");
    await connect();
    const dialog = await screen.findByRole("dialog", { name: "Password" });
    expect(within(dialog).getByText("deploy@10.0.4.12:22")).toBeTruthy();
    await user.type(within(dialog).getByLabelText("Password"), "nope");
    await user.click(within(dialog).getByRole("button", { name: "Connect" }));
    await waitFor(() => expect(screen.getByRole("alert").textContent).toBe("Incorrect password. Try again (2 attempts left)."));
    await user.click(screen.getByRole("button", { name: "Use saved password" }));
    await waitFor(() => expect(phase()).toBe("connected"));
    expect(answer).toHaveBeenLastCalledWith(expect.any(String), expect.any(String), { kind: "useSavedPassword" });
  });

  it("passwordRetry: three wrong passwords end in authFailed with a retry option", async () => {
    const { user } = setupApp({ scenario: "passwordRetry" });
    await connect();
    for (let i = 0; i < 3; i += 1) {
      const input = await screen.findByLabelText("Password", { selector: "input" });
      await user.type(input, `bad${i}`);
      await user.click(screen.getByRole("button", { name: "Connect" }));
      await waitFor(() => expect((screen.queryByLabelText("Password", { selector: "input" }) as HTMLInputElement | null)?.value ?? "").toBe(""));
    }
    await waitFor(() => expect(phase()).toBe("disconnected"));
    expect(screen.getByRole("alert").textContent).toContain("Authentication failed");
  });

  it("password prompt: 'save in keychain' only when allowed; Escape cancels", async () => {
    const { user, backend } = setupApp({ scenario: "passwordRetry" });
    const answer = vi.spyOn(backend, "sessionAnswerPrompt");
    await connect();
    await screen.findByRole("dialog", { name: "Password" });
    expect(screen.getByRole("checkbox", { name: "Save in system keychain" })).toBeTruthy();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(answer.mock.calls[0]?.[2]).toEqual({ kind: "cancel" }));
    await waitFor(() => expect(phase()).toBe("disconnected"));
  });

  it("kbdInteractive2fa: password round, then verification code (server text shown sanitised)", async () => {
    const { user } = setupApp({ scenario: "kbdInteractive2fa" });
    await connect();
    await user.type(await screen.findByLabelText("Password", { selector: "input" }), MOCK_SECRETS.password);
    await user.click(screen.getByRole("button", { name: "Connect" }));
    const dialog = await screen.findByRole("dialog", { name: "Server prompt from 10.0.4.12: Two-factor authentication" });
    expect(within(dialog).getByText("Enter the code from your authenticator app.")).toBeTruthy();
    await user.type(within(dialog).getByLabelText("Verification code:"), "000000");
    await user.click(within(dialog).getByRole("button", { name: "Continue" }));
    const again = await screen.findByLabelText("Verification code:");
    expect((again as HTMLInputElement).value).toBe("");
    await user.type(again, MOCK_SECRETS.verificationCode);
    await user.click(screen.getByRole("button", { name: "Continue" }));
    await waitFor(() => expect(phase()).toBe("connected"));
  });

  it("agentUntrusted: warning notice, then normal password authentication", async () => {
    const { user } = setupApp({ scenario: "agentUntrusted" });
    await connect();
    await user.type(await screen.findByLabelText("Password", { selector: "input" }), MOCK_SECRETS.password);
    await user.click(screen.getByRole("button", { name: "Connect" }));
    await waitFor(() => expect(phase()).toBe("connected"));
    expect(screen.getByRole("region", { name: "Notifications" }).textContent).toContain("not owned by you");
  });
});

describe("Auto-reconnect and fatal reconnect failures (§5.8)", () => {
  it("dropAfter10s: overlay walks waiting -> connecting -> authenticating, keeps unsaved note, recovers", async () => {
    const { user } = setupApp({ scenario: "dropAfter10s", dropAfterMs: 5 });
    await connect();
    await waitFor(() => expect(phase()).toBe("connected"));
    const id = firstSessionId();
    // a dirty buffer in this session -> "stays in the editor" note
    useStore.getState().addBuffer({
      id: `${id}:/srv/app/docker-compose.yml`, sessionId: id, path: "/srv/app/docker-compose.yml", load: "ready", dirty: true,
      encoding: "utf-8", eol: "lf", etag: null, savedAtMs: null,
    });
    useStore.getState().openEditorTab(id, `${id}:/srv/app/docker-compose.yml`);

    const stages: string[] = [];
    const unsubscribe = useStore.subscribe((s) => {
      const st = s.sessionsById[id]?.state;
      if (st?.phase === "reconnecting" && stages.at(-1) !== st.stage) stages.push(st.stage);
    });
    const overlay = await screen.findByRole("group", { name: "Connection to prod-web-01 lost" });
    expect(within(overlay).getByText(/Reconnecting in \d+ s|Connecting to|Authenticating/)).toBeTruthy();
    expect(within(overlay).getByText(/Attempt \d of 5/)).toBeTruthy();
    expect(within(overlay).getByText(/docker-compose\.yml stay in the editor/)).toBeTruthy();
    await user.click(within(overlay).getByRole("button", { name: "Reconnect now" }));
    await waitFor(() => expect(phase()).toBe("connected"), { timeout: 3000 });
    unsubscribe();
    expect(stages).toContain("connecting");
    expect(stages).toContain("authenticating");
    expect(screen.queryByRole("group", { name: "Connection to prod-web-01 lost" })).toBeNull();
  });

  it("reconnectHostKeyMismatch: fatal, no retry button, explains the mismatch", async () => {
    setupApp({ scenario: "reconnectHostKeyMismatch", dropAfterMs: 5 });
    await connect();
    await waitFor(() => expect(phase()).toBe("disconnected"), { timeout: 3000 });
    const overlay = await screen.findByRole("group", { name: "Connection to prod-web-01 lost" });
    expect(overlay.textContent).toContain("changed since you connected");
    expect(within(overlay).queryByRole("button", { name: "Reconnect now" })).toBeNull();
    expect(within(overlay).getByRole("button", { name: "Close tab" })).toBeTruthy();
  });
});

describe("Banner, status bar", () => {
  it("shows a server banner as sanitised text and lets the user dismiss it", async () => {
    const { user } = setupApp();
    await connect();
    await waitFor(() => expect(phase()).toBe("connected"));
    const id = firstSessionId();
    useStore.getState().applySessionEvent(id, {
      type: "banner",
      hop: { index: 0, label: "x", user: "u", host: "h", port: 22 },
      text: `Welcome${String.fromCharCode(27)}[2J${String.fromCodePoint(0x202e)}evil`,
    });
    const banner = await screen.findByRole("status", { name: "" });
    expect(banner.textContent).toContain("Welcome");
    expect(banner.textContent).not.toContain(String.fromCharCode(27));
    expect(banner.textContent).not.toContain(String.fromCodePoint(0x202e));
    await user.click(screen.getByRole("button", { name: "Dismiss server message" }));
    expect(screen.queryByText(/Welcome/)).toBeNull();
  });

  it("status bar shows the unverified host key chip only for auto-accepted keys", async () => {
    setupApp();
    await connect();
    await waitFor(() => expect(phase()).toBe("connected"));
    expect(screen.queryByText("Host key not verified")).toBeNull();
    const id = firstSessionId();
    const entry = useStore.getState().sessionsById[id];
    const info = entry?.info;
    if (!info) throw new Error("no info");
    useStore.getState().applySessionEvent(id, {
      type: "state",
      state: {
        phase: "connected",
        info: { ...info, hops: info.hops.map((h) => ({ ...h, hostKey: { ...h.hostKey, verification: "autoAccepted" as const } })) },
      },
    });
    expect(await screen.findByText("Host key not verified")).toBeTruthy();
  });
});
