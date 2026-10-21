import { act, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { setBackend } from "../../ipc";
import type { Prompt } from "../../ipc/types";
import { useStore } from "../../state/store";
import { setupApp } from "../../test/harness";

afterEach(() => setBackend(null));

const ESC = String.fromCharCode(27);
const RLO = String.fromCodePoint(0x202e);
const hop = { index: 0, label: "x", user: "alice", host: "example.com", port: 22 };

function showPrompt(prompt: Prompt) {
  useStore.getState().addSession("s1", { kind: "quick", spec: "alice@example.com:22" }, "example.com");
  act(() => useStore.getState().applySessionEvent("s1", { type: "prompt", prompt }));
}

describe("§9.5: prompt text from the server is displayed sanitised", () => {
  it("keyboard-interactive name, instructions and labels", async () => {
    setupApp();
    showPrompt({
      kind: "keyboardInteractive",
      id: "p1",
      hop,
      name: `Auth${ESC}[2J`,
      instructions: `Type the code${RLO}drowssap`,
      fields: [{ label: `Code${ESC}:`, echo: false }],
    });
    const dialog = await screen.findByRole("dialog");
    expect(dialog.textContent).not.toContain(ESC);
    expect(dialog.textContent).not.toContain(RLO);
    expect(dialog.textContent).toContain("Auth\\x1b[2J");
    expect(dialog.textContent).toContain("⟨U+202E⟩");
    expect(within(dialog).getByLabelText(/Code\\x1b:/, { selector: "input" }).getAttribute("type")).toBe("password");
  });

  it("host key prompt: host name and file path", async () => {
    setupApp();
    showPrompt({
      kind: "hostKeyUnknown",
      id: "p2",
      hop: { ...hop, host: `evil${RLO}.example.com` },
      knownHostsName: `[evil${ESC}host]:2222`,
      keyType: "ssh-ed25519",
      fingerprint: "SHA256:abc",
      knownHostsFile: null,
      otherKeyTypesKnown: ["ssh-rsa"],
      canRemember: false,
    });
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog.textContent).not.toContain(RLO);
    expect(dialog.textContent).not.toContain(ESC);
    expect(within(dialog).getByText(/other key types for this host \(ssh-rsa\)/)).toBeTruthy();
    expect(within(dialog).queryByRole("checkbox")).toBeNull();
    expect(within(dialog).getByText(/trusted for this session only/)).toBeTruthy();
  });

  it("password prompt: serverLabel is server text", async () => {
    setupApp();
    showPrompt({
      kind: "password",
      id: "p3",
      hop,
      user: "alice",
      host: "example.com",
      attemptsLeft: 3,
      lastError: null,
      canRemember: false,
      savedPasswordAvailable: false,
      serverLabel: `Password for alice${ESC}`,
    });
    const dialog = await screen.findByRole("dialog", { name: "Password" });
    expect(dialog.textContent).not.toContain(ESC);
    expect(dialog.textContent).toContain("Password for alice\\x1b");
    expect(within(dialog).queryByRole("button", { name: "Use saved password" })).toBeNull();
    expect(within(dialog).queryByRole("checkbox", { name: "Save in system keychain" })).toBeNull();
  });

  it("passphrase prompt: 'add to agent' is disabled with an explanation when the agent keeps keys permanently", async () => {
    setupApp();
    showPrompt({
      kind: "passphrase",
      id: "p4",
      hop,
      keyPath: "~/.ssh/id_example",
      attemptsLeft: 3,
      lastError: null,
      agentAvailable: true,
      addToAgentSupported: false,
      addToAgentDefault: false,
    });
    const box = (await screen.findByRole("checkbox", { name: "Add key to ssh-agent for this session" })) as HTMLInputElement;
    expect(box.disabled).toBe(true);
    expect(box.closest("label")?.getAttribute("title")).toContain("keeps keys permanently");
  });
});
