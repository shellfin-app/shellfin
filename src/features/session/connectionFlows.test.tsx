import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import type { AppError } from "../../ipc/types";
import { quickTarget } from "../../lib/targets";
import { useStore } from "../../state/store";
import { setupApp, setupConnected, settleApp } from "../../test/harness";
import { saveTerminalText } from "../terminal/saveOutput";

afterEach(async () => {
  await settleApp();
  setBackend(null);
  vi.restoreAllMocks();
});

function connectingSession(phase: "resolving" | "connecting" | "authenticating" | "openingChannels") {
  const store = useStore.getState();
  store.addSession("s1", quickTarget("alice@example.com:2200"), "example.com");
  const hop = { index: 0, label: "example.com", user: "alice", host: "example.com", port: 2200 };
  const state = phase === "resolving" ? { phase } : { phase, hop };
  act(() => useStore.getState().applySessionEvent("s1", { type: "state", state }));
}

describe("connecting card", () => {
  it("shows the target, the steps with the running one marked, elapsed time, and Cancel closes the tab", async () => {
    const { user, backend } = setupApp();
    const close = vi.spyOn(backend, "sessionClose");
    connectingSession("authenticating");
    const card = await screen.findByRole("status");
    expect(within(card).getByText("example.com", { selector: "h2" })).toBeTruthy();
    expect(card.textContent).toContain("alice@example.com:2200");
    const steps = within(card).getAllByRole("listitem");
    expect(steps).toHaveLength(5);
    expect(steps.map((s) => s.getAttribute("aria-current"))).toEqual([null, null, null, "step", null]);
    expect(card.textContent).toContain("0 s elapsed");
    await user.click(within(card).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(useStore.getState().sessionsById["s1"]).toBeUndefined());
    expect(close).toHaveBeenCalled();
  });

  it("Esc cancels while nothing modal is open", async () => {
    setupApp();
    connectingSession("connecting");
    await screen.findByRole("status");
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(useStore.getState().sessionsById["s1"]).toBeUndefined());
  });

  it("a pending host key prompt moves the running step to 'Verifying host key'", async () => {
    setupApp();
    connectingSession("connecting");
    act(() =>
      useStore.getState().applySessionEvent("s1", {
        type: "prompt",
        prompt: {
          kind: "hostKeyUnknown", id: "p1", hop: { index: 0, label: "example.com", user: "alice", host: "example.com", port: 2200 },
          knownHostsName: "example.com", keyType: "ssh-ed25519", fingerprint: "SHA256:abc", knownHostsFile: null,
          otherKeyTypesKnown: [], canRemember: true,
        },
      }),
    );
    const item = (await screen.findAllByRole("listitem")).find((li) => li.getAttribute("aria-current") === "step");
    expect(item?.textContent).toContain("Verifying host key");
  });
});

describe("connection failed panel", () => {
  const authFailed: AppError = {
    code: "authFailed",
    params: { user: "root", host: "95.216.0.17", tried: ["agent", "password"], serverMethods: ["publickey", "keyboardInteractive"] },
  };
  function fail(error: AppError, canRetry = true) {
    useStore.getState().addSession("s1", quickTarget("root@95.216.0.17:22"), "95.216.0.17");
    act(() => useStore.getState().applySessionEvent("s1", { type: "state", state: { phase: "disconnected", error, canRetry } }));
  }

  it("lists the methods tried and accepted, a one-line fix and the actions", async () => {
    setupApp();
    fail(authFailed);
    const panel = await screen.findByRole("alert");
    expect(panel.textContent).toContain("Authentication failed for root@95.216.0.17.");
    expect(panel.textContent).toContain("ssh-agent and Password");
    expect(panel.textContent).toContain("Public key and Keyboard-interactive");
    expect(panel.textContent).toContain("Check the user name and the key or password");
    for (const name of ["Close tab", "Copy details", "Edit connection…", "Try again"]) {
      expect(within(panel).getByRole("button", { name })).toBeTruthy();
    }
  });

  it("gives a remedy for refused connections with the port", async () => {
    setupApp();
    fail({ code: "connectRefused", params: { host: "h", port: 2200 } });
    expect((await screen.findByRole("alert")).textContent).toContain("Is sshd running on port 2200?");
  });

  it("Edit connection… opens the new-connection form prefilled from a quick connect and replaces the failed tab", async () => {
    const { user, backend } = setupApp();
    fail(authFailed);
    await user.click(await screen.findByRole("button", { name: "Edit connection…" }));
    const dialog = await screen.findByRole("dialog", { name: "New connection" });
    expect((within(dialog).getByLabelText("Host", { selector: "input" }) as HTMLInputElement).value).toBe("95.216.0.17");
    expect((within(dialog).getByLabelText("User", { selector: "input" }) as HTMLInputElement).value).toBe("root");
    expect((within(dialog).getByLabelText("Port", { selector: "input" }) as HTMLInputElement).value).toBe("22");
    const open = vi.spyOn(backend, "openSession");
    await user.click(within(dialog).getByRole("button", { name: "Connect" }));
    await waitFor(() => expect(open).toHaveBeenCalled());
    expect(useStore.getState().sessionsById["s1"]).toBeUndefined(); // the failed tab was replaced
  });

  it("Copy details puts the error, code and method lists on the clipboard", async () => {
    const { user } = setupApp();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    fail(authFailed);
    await user.click(await screen.findByRole("button", { name: "Copy details" }));
    await waitFor(() => expect(writeText).toHaveBeenCalled());
    const text = String(writeText.mock.calls[0]?.[0]);
    expect(text).toContain("code: authFailed");
    expect(text).toContain("root@95.216.0.17:22");
    expect(text).toContain("tried:");
  });

  it("the reconnect host-key mismatch shows both fingerprints", async () => {
    setupApp();
    fail({ code: "hostKeyMismatchOnReconnect", params: { host: "h", port: 22, expected: "SHA256:old", actual: "SHA256:new" } }, false);
    const panel = await screen.findByRole("alert");
    expect(panel.textContent).toContain("SHA256:old");
    expect(panel.textContent).toContain("SHA256:new");
  });
});

describe("new connection dialog", () => {
  async function open(user: ReturnType<typeof setupApp>["user"]) {
    await user.click(screen.getByRole("button", { name: "Add bookmark" }));
    return screen.findByRole("dialog", { name: "New connection" });
  }

  it("orders Host, User, Port, Name; shows placeholders, the Automatic help and a close button", async () => {
    const { user } = setupApp();
    const dialog = await open(user);
    const fields = ["Host", "User", "Port", "Name (optional)"].map((l) => within(dialog).getByLabelText(l, { selector: "input" }));
    for (let i = 1; i < fields.length; i += 1) {
      expect((fields[i - 1] as HTMLElement).compareDocumentPosition(fields[i] as HTMLElement) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    }
    expect((within(dialog).getByLabelText("Port", { selector: "input" }) as HTMLInputElement).placeholder).toBe("22");
    expect(dialog.textContent).toContain("Tries ssh-agent, then your default keys");
    expect(within(dialog).getByRole("button", { name: "Test connection" })).toBeTruthy();
    await user.click(within(dialog).getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog", { name: "New connection" })).toBeNull();
  });

  it("splits a pasted user@host:port and the name defaults to the host", async () => {
    const { user, backend } = setupApp();
    const save = vi.spyOn(backend, "bookmarkSave");
    const dialog = await open(user);
    const host = within(dialog).getByLabelText("Host", { selector: "input" });
    fireEvent.paste(host, { clipboardData: { getData: () => "deploy@10.0.4.12:2201" } });
    await waitFor(() => expect((host as HTMLInputElement).value).toBe("10.0.4.12"));
    expect((within(dialog).getByLabelText("User", { selector: "input" }) as HTMLInputElement).value).toBe("deploy");
    expect((within(dialog).getByLabelText("Port", { selector: "input" }) as HTMLInputElement).value).toBe("2201");
    expect((within(dialog).getByLabelText("Name (optional)", { selector: "input" }) as HTMLInputElement).placeholder).toBe("10.0.4.12");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(save).toHaveBeenCalled());
    expect(save.mock.calls[0]?.[0]).toMatchObject({ name: "10.0.4.12", host: "10.0.4.12", port: 2201, user: "deploy" });
  });

  it("Connect does not keep a bookmark; Save & connect does", async () => {
    const { user, backend } = setupApp();
    const save = vi.spyOn(backend, "bookmarkSave");
    let dialog = await open(user);
    await user.type(within(dialog).getByLabelText("Host", { selector: "input" }), "a.example.com");
    await user.click(within(dialog).getByRole("button", { name: "Connect" }));
    await waitFor(() => expect(useStore.getState().sessionOrder).toHaveLength(1));
    expect(save).not.toHaveBeenCalled();

    dialog = await open(user);
    await user.type(within(dialog).getByLabelText("Host", { selector: "input" }), "b.example.com");
    await user.click(within(dialog).getByRole("button", { name: "Save & connect" }));
    await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(useStore.getState().sessionOrder).toHaveLength(2));
  });

  it("per-bookmark transfer speed: a limit is sent in KiB/s, 'global' leaves the field out", async () => {
    const { user, backend } = setupApp();
    const save = vi.spyOn(backend, "bookmarkSave");
    const dialog = await open(user);
    await user.type(within(dialog).getByLabelText("Host", { selector: "input" }), "slow.example.com");
    await user.selectOptions(within(dialog).getByLabelText("Upload limit", { selector: "select" }), "limit");
    const value = within(dialog).getByLabelText("Upload limit", { selector: "input" });
    await user.clear(value);
    await user.type(value, "2");
    await user.selectOptions(within(dialog).getAllByLabelText("Unit")[0] as HTMLElement, "MiB");
    await user.selectOptions(within(dialog).getByLabelText("Download limit", { selector: "select" }), "unlimited");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(save).toHaveBeenCalled());
    const input = save.mock.calls[0]?.[0];
    expect(input?.uploadLimitKiBps).toBe(2048);
    expect(input?.downloadLimitKiBps).toBe(0);
  });
});

describe("password prompt", () => {
  it("the password field has a show/hide toggle", async () => {
    const { user } = setupApp({ scenario: "passwordRetry" });
    const { openSession } = await import("./sessionActions");
    await openSession(quickTarget("alice@example.com:22"), "example.com");
    const dialog = await screen.findByRole("dialog", { name: "Password" });
    const field = within(dialog).getByLabelText("Password", { selector: "input" }) as HTMLInputElement;
    expect(field.type).toBe("password");
    await user.click(within(dialog).getByRole("button", { name: "Show" }));
    expect(field.type).toBe("text");
    await user.click(within(dialog).getByRole("button", { name: "Hide" }));
    expect(field.type).toBe("password");
  });
});

describe("Save output…", () => {
  it("sends the text to the native save command and reports saved / cancelled", async () => {
    const { backend } = setupApp();
    const save = vi.spyOn(backend, "appSaveTextFile");
    await saveTerminalText("line 1\nline 2");
    expect(save).toHaveBeenCalledWith("terminal-output.txt", "line 1\nline 2\n", "Save terminal output");
    expect(useStore.getState().notices.some((n) => n.info?.kind === "outputSaved")).toBe(true);

    save.mockResolvedValueOnce({ saved: false, fileName: null });
    await saveTerminalText("x");
    expect(useStore.getState().notices.some((n) => n.info?.kind === "outputCancelled")).toBe(true);
  });
});

describe("editor status, palette and transfers on a live session", () => {
  it("the command palette finds files in all mode and shows shortcut chips", async () => {
    const { user } = await setupConnected();
    act(() => useStore.getState().openPalette("all"));
    const palette = await screen.findByRole("dialog", { name: "Command palette" });
    expect(within(palette).getAllByText("Ctrl+B").length).toBeGreaterThan(0);
    await user.type(within(palette).getByRole("combobox"), "deploy");
    expect(within(palette).getByText("deploy.sh")).toBeTruthy();
    await user.clear(within(palette).getByRole("combobox"));
    await user.type(within(palette).getByRole("combobox"), ">deploy");
    expect(within(palette).queryByText("deploy.sh")).toBeNull();
  });
});
