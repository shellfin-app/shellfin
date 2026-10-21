import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import { useStore } from "../../state/store";
import { firstPhase, setupApp } from "../../test/harness";
import { openSession } from "../session/sessionActions";
import { sshConfigTarget } from "../../lib/targets";

afterEach(() => setBackend(null));

describe("Command palette (Mod+K)", () => {
  it("Ctrl+K opens it; typing filters hosts; Enter opens a session; Escape closes", async () => {
    const { user } = setupApp();
    await user.keyboard("{Control>}k{/Control}");
    const palette = await screen.findByRole("dialog", { name: "Command palette" });
    const input = within(palette).getByRole("combobox");
    expect(document.activeElement).toBe(input);
    await user.type(input, "staging");
    expect(within(palette).getByText("staging-db")).toBeTruthy();
    expect(within(palette).queryByText("prod-web-02")).toBeNull();
    await user.keyboard("{Enter}");
    await waitFor(() => expect(useStore.getState().sessionOrder).toHaveLength(1));
    expect(screen.queryByRole("dialog", { name: "Command palette" })).toBeNull();

    await user.keyboard("{Control>}k{/Control}");
    await screen.findByRole("dialog", { name: "Command palette" });
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: "Command palette" })).toBeNull();
  });

  it("lists commands (new connection, settings) and runs them", async () => {
    const { user } = setupApp();
    await user.click(screen.getByRole("button", { name: "Search hosts, files and commands" }));
    const palette = await screen.findByRole("dialog", { name: "Command palette" });
    await user.type(within(palette).getByRole("combobox"), "new conn");
    await user.keyboard("{Enter}");
    expect(await screen.findByRole("dialog", { name: "New connection" })).toBeTruthy();
  });

  it("closing the palette opened from the title-bar search does not reopen it (focus is restored to the trigger)", async () => {
    const { user } = setupApp();
    const trigger = screen.getByRole("button", { name: "Search hosts, files and commands" });
    await user.click(trigger);
    await screen.findByRole("dialog", { name: "Command palette" });
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Command palette" })).toBeNull());
    // Focus returning to the trigger must not open the palette again.
    act(() => trigger.focus());
    expect(screen.queryByRole("dialog", { name: "Command palette" })).toBeNull();
  });

  it("files mode (Search activity): open buffers and already-loaded tree entries", async () => {
    const { user } = setupApp();
    await openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
    await waitFor(() => expect(firstPhase()).toBe("connected"));
    await waitFor(() => expect(screen.getByRole("treeitem", { name: /docker-compose\.yml/ })).toBeTruthy());
    await user.click(screen.getByRole("button", { name: "Search" }));
    const palette = await screen.findByRole("dialog", { name: "Command palette" });
    await user.type(within(palette).getByRole("combobox"), "deploy");
    expect(within(palette).getByText("deploy.sh")).toBeTruthy();
    expect(within(palette).queryByText("README.md")).toBeNull();
    await user.keyboard("{Enter}");
    await waitFor(() => expect(screen.getByRole("tab", { name: /deploy\.sh/ })).toBeTruthy());
  });
});

describe("Settings and keys", () => {
  it("settings: rejects out-of-range values, saves valid ones through settings_update", async () => {
    const { user, backend } = setupApp();
    const update = vi.spyOn(backend, "settingsUpdate");
    await user.click(screen.getByRole("button", { name: "Settings" }));
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
    await user.click(within(dialog).getByRole("tab", { name: "Terminal" }));
    const fontSize =within(dialog).getAllByLabelText("Font size", { selector: "input" })[0] as HTMLInputElement;
    await user.clear(fontSize);
    await user.type(fontSize, "99");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(update).not.toHaveBeenCalled();
    expect(fontSize.getAttribute("aria-invalid")).toBe("true");

    await user.clear(fontSize);
    await user.type(fontSize, "15");
    await user.click(within(dialog).getByRole("tab", { name: "Files" }));
    await user.click(within(dialog).getByRole("checkbox", { name: "Show hidden files" }));
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(update).toHaveBeenCalled());
    expect(update.mock.calls[0]?.[0].terminal.fontSize).toBe(15);
    expect(update.mock.calls[0]?.[0].files.showHidden).toBe(false);
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Settings" })).toBeNull());
    expect(useStore.getState().settings?.terminal.fontSize).toBe(15);
  });

  it("settings: transfer speed limits are saved in KiB/s (MiB/s through the unit select)", async () => {
    const { user, backend } = setupApp();
    const update = vi.spyOn(backend, "settingsUpdate");
    await user.click(screen.getByRole("button", { name: "Settings" }));
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
    await user.click(within(dialog).getByRole("tab", { name: "Transfers" }));
    const upload =within(dialog).getByLabelText("Upload speed limit", { selector: "input" });
    await user.clear(upload);
    await user.type(upload, "512");
    const download = within(dialog).getByLabelText("Download speed limit", { selector: "input" });
    await user.clear(download);
    await user.type(download, "3");
    await user.selectOptions(within(dialog).getAllByLabelText("Unit")[1] as HTMLElement, "MiB");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(update).toHaveBeenCalled());
    expect(update.mock.calls[0]?.[0].transfers).toEqual({
      uploadLimitKiBps: 512,
      downloadLimitKiBps: 3072,
      maxConcurrent: 3,
      onConflict: "ask",
      preserveTimestamps: true,
      notifyOnComplete: false,
    });

    await user.click(screen.getByRole("button", { name: "Settings" }));
    const again = await screen.findByRole("dialog", { name: "Settings" });
    await user.click(within(again).getByRole("tab", { name: "Transfers" }));
    await user.clear(within(again).getByLabelText("Upload speed limit", { selector: "input" }));
    await user.type(within(again).getByLabelText("Upload speed limit", { selector: "input" }), "x");
    await user.click(within(again).getByRole("button", { name: "Save" }));
    expect(within(again).getAllByText(/Enter a whole number up to 1 GiB\/s/).length).toBeGreaterThan(0);
  });

  it("keys: lists local keys and agent identities", async () => {
    const { user } = setupApp();
    await user.click(screen.getByRole("button", { name: "Keys" }));
    const dialog = await screen.findByRole("dialog", { name: "Keys" });
    await waitFor(() => expect(within(dialog).getByText("~/.ssh/id_ed25519")).toBeTruthy());
    expect(within(dialog).getByText("Passphrase")).toBeTruthy();
    expect(within(dialog).getAllByText("In ssh-agent").length).toBe(2);
    expect(within(dialog).getAllByText("File").length).toBe(2);
    expect(within(dialog).getByText("~/.ssh/work_rsa")).toBeTruthy();
    expect(within(dialog).getAllByRole("button", { name: "Copy fingerprint" }).length).toBe(2);
    await user.click(within(dialog).getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog", { name: "Keys" })).toBeNull();
  });
});

describe("Welcome screen", () => {
  it("quick connect resolves aliases, bookmarks and user@host:port", async () => {
    const { user } = setupApp();
    const input = screen.getByRole("textbox", { name: "Quick connect" });
    await user.type(input, "pi-home{Enter}");
    await waitFor(() => expect(useStore.getState().sessionOrder).toHaveLength(1));
    expect(useStore.getState().sessionsById[useStore.getState().sessionOrder[0] ?? ""]?.target).toEqual({
      kind: "bookmark",
      id: expect.any(String),
    });
    await user.click(screen.getByRole("tab", { name: "Home" }));
    await user.type(screen.getByRole("textbox", { name: "Quick connect" }), "alice@example.com:2200{Enter}");
    await waitFor(() => expect(useStore.getState().sessionOrder).toHaveLength(2));
    expect(useStore.getState().sessionsById[useStore.getState().sessionOrder[1] ?? ""]?.target).toEqual({
      kind: "quick",
      spec: "alice@example.com:2200",
    });
  });

  it("recents follow recents:changed events", async () => {
    const { backend } = setupApp();
    await backend.listen("recents:changed", (r) => useStore.getState().setRecents(r));
    const before = screen.getAllByText(/terminals?$/).length;
    expect(before).toBeGreaterThan(0);
    await act(async () => {
      backend.emit("recents:changed", [
        { target: { kind: "quick", spec: "x@example.com:22" }, label: "example.com", userAtHost: "x@example.com", via: null, lastConnectedAtMs: Date.now(), lastDir: "/tmp", terminalCount: 2 },
      ]);
    });
    useStore.getState().setRecents(useStore.getState().recents);
    await waitFor(() => expect(screen.getByText("x@example.com")).toBeTruthy());
    expect(screen.getByText("2 terminals")).toBeTruthy();
  });

  it("New connection and Manage keys open their dialogs", async () => {
    const { user } = setupApp();
    await user.click(screen.getByRole("button", { name: "New connection", description: "" }).closest("button") as HTMLElement);
    expect(await screen.findByRole("dialog", { name: "New connection" })).toBeTruthy();
    await user.keyboard("{Escape}");
    await user.click(screen.getByRole("button", { name: "Manage keys" }));
    expect(await screen.findByRole("dialog", { name: "Keys" })).toBeTruthy();
  });
});

describe("Tabs and shortcuts", () => {
  it("Mod+1..9 switch connection tabs; Mod+N opens the new-connection dialog", async () => {
    const { user } = setupApp();
    await openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
    await openSession(sshConfigTarget("prod-web-02"), "prod-web-02");
    const [a, b] = useStore.getState().sessionOrder;
    expect(useStore.getState().activeSessionId).toBe(b);
    await user.keyboard("{Control>}1{/Control}");
    expect(useStore.getState().activeSessionId).toBe(a);
    await waitFor(() => {
      const { sessionsById } = useStore.getState();
      expect(Object.values(sessionsById).every((e) => e.state.phase === "connected")).toBe(true);
    });
    await waitFor(() => expect(document.querySelector(".terminal-host .xterm")).not.toBeNull());
    (document.activeElement as HTMLElement | null)?.blur(); // Mod+N is not active inside the terminal (§8.7)
    await user.keyboard("{Control>}n{/Control}");
    expect(await screen.findByRole("dialog", { name: "New connection" })).toBeTruthy();
  });
});
