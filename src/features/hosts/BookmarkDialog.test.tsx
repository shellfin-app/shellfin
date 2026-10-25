import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import { useStore } from "../../state/store";
import { setupApp } from "../../test/harness";
import { editConnection } from "../session/editConnection";
import { refreshBookmarks } from "./hostActions";

afterEach(() => setBackend(null));

const SECRET = "p4ss-w0rd-for-tests";

async function openNewDialog(user: ReturnType<typeof setupApp>["user"]) {
  await user.click(screen.getByRole("button", { name: "Add bookmark" }));
  return screen.findByRole("dialog", { name: "New connection" });
}

async function fill(user: ReturnType<typeof setupApp>["user"], dialog: HTMLElement, values: Record<string, string>) {
  for (const [label, value] of Object.entries(values)) {
    const input = within(dialog).getByLabelText(label, { selector: "input" });
    await user.clear(input);
    await user.type(input, value);
  }
}

describe("BookmarkDialog", () => {
  it("validates before saving", async () => {
    const { user, backend } = setupApp();
    const save = vi.spyOn(backend, "bookmarkSave");
    const dialog = await openNewDialog(user);
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(within(dialog).getByText("Enter a host.")).toBeTruthy();
    await fill(user, dialog, { "Name (optional)": "x", Host: "h", Port: "99999" });
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(within(dialog).getByText("Port must be a number from 1 to 65535.")).toBeTruthy();
    expect(save).not.toHaveBeenCalled();
  });

  it("saves a key-based bookmark (all fields, browse, group, start dir) and closes", async () => {
    const { user, backend } = setupApp();
    vi.spyOn(backend, "dialogPickIdentityFile").mockResolvedValue("~/.ssh/id_example");
    const save = vi.spyOn(backend, "bookmarkSave");
    const dialog = await openNewDialog(user);
    await fill(user, dialog, { "Name (optional)": "nas2", Host: "nas2.local", Port: "2222", User: "admin", "Start directory": "/data" });
    await user.click(within(dialog).getByRole("radio", { name: "Key" }));
    await user.click(within(dialog).getByRole("button", { name: "Browse…" }));
    expect((within(dialog).getByLabelText("Private key", { selector: "input" }) as HTMLInputElement).value).toBe("~/.ssh/id_example");
    await user.selectOptions(within(dialog).getByLabelText("Via jump host (ProxyJump)"), "ssh:bastion");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));

    await waitFor(() => expect(screen.queryByRole("dialog", { name: "New connection" })).toBeNull());
    expect(save).toHaveBeenCalledWith({
      id: null, name: "nas2", host: "nas2.local", port: 2222, user: "admin",
      auth: { method: "key", identityFile: "~/.ssh/id_example" },
      proxyJump: { kind: "sshConfig", alias: "bastion" }, groupId: null, color: null, tags: [], startDir: "/data", useSshConfig: true,
      confirmConnect: false, startupCommand: null, notes: null,
    });
    expect(useStore.getState().bookmarks?.bookmarks.some((b) => b.name === "nas2")).toBe(true);
    expect(screen.getByRole("button", { name: "Connect to nas2" })).toBeTruthy();
  });

  it("offers the ProxyJump choices from ~/.ssh/config and shows agent keys in ssh-agent mode", async () => {
    const { user } = setupApp();
    const dialog = await openNewDialog(user);
    expect(within(dialog).getByRole("option", { name: "bastion (from ~/.ssh/config)" })).toBeTruthy();
    await user.click(within(dialog).getByRole("radio", { name: "ssh-agent" }));
    expect(within(dialog).getByText(/ssh-ed25519 SHA256:.* id_ed25519/)).toBeTruthy();
    await user.click(within(dialog).getByRole("radio", { name: "Password" }));
    expect(within(dialog).getByRole("checkbox", { name: /Save in system keychain/ })).toBeTruthy();
  });

  it("creates a new group inline and assigns the bookmark to it", async () => {
    const { user } = setupApp();
    const dialog = await openNewDialog(user);
    await fill(user, dialog, { "Name (optional)": "grouped", Host: "g.example.com" });
    await user.selectOptions(within(dialog).getByLabelText("Group"), "New group…");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(within(dialog).getByText("Enter a group name.")).toBeTruthy();
    await user.type(within(dialog).getByLabelText("New group name"), "Work");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "New connection" })).toBeNull());
    const { groups, bookmarks } = useStore.getState().bookmarks ?? { groups: [], bookmarks: [] };
    expect(groups.map((g) => g.name)).toEqual(["Work"]);
    expect(bookmarks.find((b) => b.name === "grouped")?.groupId).toBe(groups[0]?.id);
    expect(screen.getByRole("button", { name: /^Work\s*1$/ })).toBeTruthy();
  });

  it("password: stored in the keychain through bookmark_set_password, never in the store", async () => {
    const { user, backend } = setupApp();
    const setPassword = vi.spyOn(backend, "bookmarkSetPassword");
    const dialog = await openNewDialog(user);
    await fill(user, dialog, { "Name (optional)": "pw", Host: "pw.example.com", User: "alice" });
    await user.click(within(dialog).getByRole("radio", { name: "Password" }));
    await user.type(within(dialog).getByLabelText("Password", { selector: "input" }), SECRET);
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(setPassword).toHaveBeenCalledWith(expect.any(String), SECRET));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "New connection" })).toBeNull());
    expect(JSON.stringify(useStore.getState())).not.toContain(SECRET);
    const saved = useStore.getState().bookmarks?.bookmarks.find((b) => b.name === "pw");
    expect(saved?.auth).toEqual({ method: "password", passwordSaved: true });
  });

  it("connect without saving: draft request carries the password, the store only a quick target", async () => {
    const { user, backend } = setupApp();
    const open = vi.spyOn(backend, "openSession");
    const dialog = await openNewDialog(user);
    await fill(user, dialog, { "Name (optional)": "tmp", Host: "tmp.example.com", User: "bob" });
    await user.click(within(dialog).getByRole("radio", { name: "Password" }));
    await user.type(within(dialog).getByLabelText("Password", { selector: "input" }), SECRET);
    await user.click(within(dialog).getByRole("button", { name: "Connect" }));

    await waitFor(() => expect(open).toHaveBeenCalled());
    const request = open.mock.calls[0]?.[0];
    expect(request?.target).toMatchObject({ kind: "draft", password: SECRET, bookmark: { host: "tmp.example.com", user: "bob" } });
    await waitFor(() => expect(useStore.getState().sessionOrder).toHaveLength(1));
    const target = useStore.getState().sessionsById[useStore.getState().sessionOrder[0] ?? ""]?.target;
    expect(target).toEqual({ kind: "quick", spec: "bob@tmp.example.com:22" });
    expect(JSON.stringify(useStore.getState())).not.toContain(SECRET);
    expect(useStore.getState().bookmarks?.bookmarks.some((b) => b.name === "tmp")).toBe(false);
  });

  it("Test connection: success leaves no tab behind; a failing host shows the reason", async () => {
    const ok = setupApp();
    let dialog = await openNewDialog(ok.user);
    await fill(ok.user, dialog, { "Name (optional)": "t", Host: "t.example.com" });
    await ok.user.click(within(dialog).getByRole("button", { name: "Test connection" }));
    await waitFor(() => expect(within(dialog).getByText("Connection works.")).toBeTruthy());
    expect(useStore.getState().sessionOrder).toHaveLength(0);
    expect(Object.keys(useStore.getState().sessionsById)).toHaveLength(0);
    ok.unmount();

    const bad = setupApp({ scenario: "hostKeyChanged" });
    dialog = await openNewDialog(bad.user);
    await fill(bad.user, dialog, { "Name (optional)": "t", Host: "t.example.com" });
    await bad.user.click(within(dialog).getByRole("button", { name: "Test connection" }));
    await waitFor(() => expect(within(dialog).getByText("Connection test failed.")).toBeTruthy());
    expect(dialog.textContent).toContain("has changed");
    expect(useStore.getState().sessionOrder).toHaveLength(0);
  });

  it("Test connection shows the normal prompts of the hidden probe session", async () => {
    const { user } = setupApp({ scenario: "hostKeyUnknown" });
    const dialog = await openNewDialog(user);
    await fill(user, dialog, { "Name (optional)": "t", Host: "t.example.com" });
    await user.click(within(dialog).getByRole("button", { name: "Test connection" }));
    // A probe prompt never pops up by itself: the dialog offers an "Answer" button.
    const answerButton = await within(dialog).findByRole("button", { name: "Answer" });
    expect(screen.queryByRole("alertdialog")).toBeNull();
    await user.click(answerButton);
    const prompt = await screen.findByRole("alertdialog", { name: /First connection to t\.example\.com/ });
    await user.click(within(prompt).getByRole("button", { name: "Trust and connect" }));
    await waitFor(() => expect(within(dialog).getByText("Connection works.")).toBeTruthy());
  });

  it("editing a bookmark with a saved password warns before the password is forgotten", async () => {
    const { user, backend } = setupApp();
    const created = await backend.bookmarkSave({
      id: null, name: "keep", host: "k.example.com", port: null, user: "carol", auth: { method: "password" },
      proxyJump: null, groupId: null, tags: [], startDir: null, useSshConfig: true,
    });
    await backend.bookmarkSetPassword(created.id, SECRET);
    await refreshBookmarks();
    await user.click(screen.getByRole("button", { name: "Edit keep" }));
    const dialog = await screen.findByRole("dialog", { name: "Edit bookmark" });
    expect(within(dialog).queryByText(/removes the saved password/)).toBeNull();
    await user.clear(within(dialog).getByLabelText("Host", { selector: "input" }));
    await user.type(within(dialog).getByLabelText("Host", { selector: "input" }), "other.example.com");
    expect(within(dialog).getByText("Changing the host, port, user or jump host removes the saved password.")).toBeTruthy();
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Edit bookmark" })).toBeNull());
    const after = useStore.getState().bookmarks?.bookmarks.find((b) => b.id === created.id);
    expect(after?.host).toBe("other.example.com");
    expect(after?.auth).toEqual({ method: "password", passwordSaved: false });
  });

  it("deletes a bookmark after confirmation and copies an ssh-config host", async () => {
    const { user } = setupApp();
    await user.click(screen.getByRole("button", { name: "Copy bastion to my bookmarks" }));
    await waitFor(() => expect(screen.getAllByRole("button", { name: "Connect to bastion" })).toHaveLength(2));
    expect(useStore.getState().bookmarks?.bookmarks.some((b) => b.name === "bastion")).toBe(true);

    await user.click(screen.getByRole("button", { name: "Delete nas" }));
    const confirm = await screen.findByRole("alertdialog", { name: "Delete bookmark?" });
    await user.click(within(confirm).getByRole("button", { name: "Cancel" }));
    expect(useStore.getState().bookmarks?.bookmarks.some((b) => b.name === "nas")).toBe(true);

    await user.click(screen.getByRole("button", { name: "Delete nas" }));
    await user.click(within(await screen.findByRole("alertdialog", { name: "Delete bookmark?" })).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(useStore.getState().bookmarks?.bookmarks.some((b) => b.name === "nas")).toBe(false));
  });

  it("traps focus inside the dialog and Escape closes it", async () => {
    const { user } = setupApp();
    const dialog = await openNewDialog(user);
    expect(dialog.contains(document.activeElement)).toBe(true);
    for (let i = 0; i < 30; i += 1) {
      await user.tab();
      expect(dialog.contains(document.activeElement)).toBe(true);
    }
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: "New connection" })).toBeNull();
  });
});

describe("BookmarkDialog: User placeholder", () => {
  const DEFAULT = "Your local user name";
  const userInput = (dialog: HTMLElement) => within(dialog).getByLabelText("User", { selector: "input" }) as HTMLInputElement;

  it("shows the SSH config User when the host is an alias, typed by hand", async () => {
    const { user } = setupApp();
    const dialog = await openNewDialog(user);
    expect(userInput(dialog).placeholder).toBe(DEFAULT);
    await fill(user, dialog, { Host: "staging-db" });
    expect(userInput(dialog).placeholder).toBe("postgres (from SSH config)");
    await user.type(within(dialog).getByLabelText("Host", { selector: "input" }), "x");
    expect(userInput(dialog).placeholder).toBe(DEFAULT);
  });

  it("matches the alias ignoring surrounding spaces", async () => {
    const { user } = setupApp();
    const dialog = await openNewDialog(user);
    await fill(user, dialog, { Host: " staging-db " });
    expect(userInput(dialog).placeholder).toBe("postgres (from SSH config)");
  });

  it("matches the alias case-sensitively, like OpenSSH Host patterns", async () => {
    const { user } = setupApp();
    const dialog = await openNewDialog(user);
    await fill(user, dialog, { Host: "Staging-DB" });
    expect(userInput(dialog).placeholder).toBe("Your local user name");
    // ... and says why: a bookmark `Staging-DB` does not get the settings of `Host staging-db`.
    expect(
      within(dialog).getByText(
        "~/.ssh/config has “staging-db”. Host names there are case-sensitive, so its settings are not used for this host.",
      ),
    ).toBeTruthy();
  });

  it("shows no case hint for an exact alias or an unknown host", async () => {
    const { user } = setupApp();
    const dialog = await openNewDialog(user);
    await fill(user, dialog, { Host: "staging-db" });
    expect(within(dialog).queryByText(/case-sensitive/)).toBeNull();
    await fill(user, dialog, { Host: "elsewhere.example.com" });
    expect(within(dialog).queryByText(/case-sensitive/)).toBeNull();
  });

  it("shows it right away for an ssh-config prefill", async () => {
    setupApp();
    act(() => useStore.getState().addSession("cfg1", { kind: "sshConfig", alias: "bastion" }, "bastion"));
    act(() => editConnection("cfg1"));
    const dialog = await screen.findByRole("dialog", { name: "New connection" });
    expect(userInput(dialog).placeholder).toBe("jump (from SSH config)");
  });

  it("keeps the default for an alias without User", async () => {
    const { user } = setupApp();
    act(() => {
      const cfg = useStore.getState().sshConfig;
      if (!cfg) throw new Error("no ssh config");
      useStore.setState({ sshConfig: { ...cfg, hosts: cfg.hosts.map((h) => (h.alias === "bastion" ? { ...h, user: null } : h)) } });
    });
    const dialog = await openNewDialog(user);
    await fill(user, dialog, { Host: "bastion" });
    expect(userInput(dialog).placeholder).toBe(DEFAULT);
  });
});
