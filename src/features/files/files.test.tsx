import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import { useStore } from "../../state/store";
import { settleApp, setupConnected } from "../../test/harness";

afterEach(async () => {
  await settleApp();
  setBackend(null);
  vi.restoreAllMocks();
});

const row = (name: RegExp | string) => screen.getByRole("treeitem", { name });

async function menuFor(target: HTMLElement) {
  fireEvent.contextMenu(target, { clientX: 20, clientY: 20 });
  return screen.findByRole("menu");
}

describe("file tree", () => {
  it("is virtualized: a crowded folder renders only a window of rows, and a truncated listing ends with a notice (largeFile)", async () => {
    const { user } = await setupConnected({ scenario: "largeFile" });
    await user.click(row("many"));
    await screen.findByRole("treeitem", { name: /item-00000\.log/ });
    expect(screen.getAllByRole("treeitem").length).toBeLessThan(200);
    // the notice is the last row of that folder: scroll the list to the end
    const tree = screen.getByRole("tree", { name: "Remote files" });
    act(() => {
      tree.scrollTop = 1_000_000;
      fireEvent.scroll(tree);
    });
    await screen.findByText("The list is incomplete: only the first 3000 entries are shown.");
  });

  it("sftpUnavailable: the tree is replaced by a calm note and terminals still open", async () => {
    await setupConnected({ scenario: "sftpUnavailable", tree: false });
    expect(await screen.findByText("SFTP is not available on this server.")).toBeTruthy();
    expect(screen.queryByRole("tree")).toBeNull();
    expect(screen.queryByRole("toolbar", { name: "File actions" })).toBeNull();
    const { sessionId } = useStore.getState().sessionOrder.length ? { sessionId: useStore.getState().sessionOrder[0] } : { sessionId: "" };
    await waitFor(() => expect(Object.values(useStore.getState().terminals).filter((t) => t.sessionId === sessionId)).toHaveLength(1));
  });

  it("expands folders lazily and shows the loading state, then the entries", async () => {
    const { user } = await setupConnected();
    await user.click(row("nginx"));
    await screen.findByRole("treeitem", { name: /nginx\.conf/ });
    expect(row("nginx").getAttribute("aria-expanded")).toBe("true");
    await user.click(row("nginx"));
    expect(screen.queryByRole("treeitem", { name: /nginx\.conf/ })).toBeNull();
  });

  it("keyboard: arrows move the selection, right/left expand and collapse, Enter opens a file", async () => {
    const { user } = await setupConnected();
    const tree = screen.getByRole("tree", { name: "Remote files" });
    act(() => tree.focus());
    await user.keyboard("{ArrowDown}"); // root is selected on focus, first child next
    const selected = () => screen.getAllByRole("treeitem").find((el) => el.getAttribute("aria-selected") === "true");
    expect(selected()?.textContent).toContain("certs");
    await user.keyboard("{ArrowDown}{ArrowDown}"); // logs, nginx
    expect(selected()?.textContent).toContain("nginx");
    await user.keyboard("{ArrowRight}");
    await screen.findByRole("treeitem", { name: /nginx\.conf/ });
    await user.keyboard("{ArrowRight}"); // into the folder
    expect(selected()?.textContent).toContain("mime.types");
    await user.keyboard("{ArrowLeft}"); // back to the parent
    expect(selected()?.textContent).toContain("nginx");
    await user.keyboard("{ArrowLeft}"); // collapse
    expect(row("nginx").getAttribute("aria-expanded")).toBe("false");
    await user.keyboard("{End}{Enter}");
    await waitFor(() => expect(Object.keys(useStore.getState().buffers).length).toBe(1));
  });

  it("New file: the menu shows an input row, Enter creates the file, selects it and opens it", async () => {
    const { user, sessionId } = await setupConnected();
    const menu = await menuFor(row("app"));
    await user.click(within(menu).getByRole("menuitem", { name: "New file…" }));
    const input = await screen.findByRole("textbox", { name: "New file name" });
    await user.type(input, "todo.txt{Enter}");
    await screen.findByRole("treeitem", { name: /todo\.txt/ });
    await waitFor(() => expect(useStore.getState().buffers[`${sessionId}:/srv/app/todo.txt`]?.load).toBe("ready"));
    expect(useStore.getState().workspaces[sessionId]?.tree.selection).toBe("/srv/app/todo.txt");
  });

  it("validates names and keeps the input open when the name exists", async () => {
    const { user } = await setupConnected();
    const menu = await menuFor(row("app"));
    await user.click(within(menu).getByRole("menuitem", { name: "New folder…" }));
    const input = await screen.findByRole("textbox", { name: "New folder name" });
    await user.type(input, "a/b{Enter}");
    expect(await screen.findByText("A name can't contain a slash.")).toBeTruthy();
    await user.clear(input);
    await user.type(input, "certs{Enter}"); // exists
    await waitFor(() => expect(useStore.getState().notices.some((n) => n.error?.code === "remoteExists")).toBe(true));
    expect(screen.getByRole("textbox", { name: "New folder name" })).toBeTruthy();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("textbox", { name: "New folder name" })).toBeNull();
  });

  it("Rename: the open buffer follows the new path and keeps its text", async () => {
    const { user, sessionId } = await setupConnected();
    await user.click(row("README.md"));
    await waitFor(() => expect(useStore.getState().buffers[`${sessionId}:/srv/app/README.md`]?.load).toBe("ready"));
    const tree = screen.getByRole("tree", { name: "Remote files" });
    act(() => tree.focus());
    await user.keyboard("{F2}");
    const input = await screen.findByRole("textbox", { name: "New name" });
    await user.clear(input);
    await user.type(input, "NOTES.md{Enter}");
    await screen.findByRole("treeitem", { name: /NOTES\.md/ });
    expect(screen.queryByRole("treeitem", { name: /README\.md/ })).toBeNull();
    const buffers = useStore.getState().buffers;
    expect(Object.keys(buffers)).toEqual([`${sessionId}:/srv/app/NOTES.md`]);
    expect(useStore.getState().workspaces[sessionId]?.activeBuffer).toBe(`${sessionId}:/srv/app/NOTES.md`);
  });

  it("Move…: asks for a destination folder and moves the entry there", async () => {
    const { user } = await setupConnected();
    const menu = await menuFor(row("deploy.sh"));
    await user.click(within(menu).getByRole("menuitem", { name: "Move…" }));
    const dialog = await screen.findByRole("dialog", { name: "Move deploy.sh" });
    const input = within(dialog).getByRole("textbox", { name: "Destination folder" });
    await user.clear(input);
    await user.type(input, "/srv/app/logs");
    await user.click(within(dialog).getByRole("button", { name: "Move" }));
    await waitFor(() => expect(screen.queryByRole("treeitem", { name: /deploy\.sh/ })).toBeNull());
    await user.click(row("logs"));
    await screen.findByRole("treeitem", { name: /deploy\.sh/ });
  });

  it("Delete asks first; a non-empty folder is explained inside the dialog", async () => {
    const { user, backend } = await setupConnected();
    const del = vi.spyOn(backend, "sftpDelete");
    const menu = await menuFor(row("nginx"));
    await user.click(within(menu).getByRole("menuitem", { name: /^Delete/ }));
    const dialog = await screen.findByRole("alertdialog", { name: "Delete the folder nginx?" });
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));
    await within(dialog).findByText("/srv/app/nginx is not empty.");
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));

    const fileMenu = await menuFor(row("README.md"));
    await user.click(within(fileMenu).getByRole("menuitem", { name: /^Delete/ }));
    const confirm = await screen.findByRole("alertdialog", { name: "Delete README.md?" });
    await user.click(within(confirm).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(screen.queryByRole("treeitem", { name: /README\.md/ })).toBeNull());
    expect(del).toHaveBeenLastCalledWith(expect.any(String), "/srv/app/README.md");
  });

  it("Delete key opens the confirmation for the selected row; Escape cancels without deleting", async () => {
    const { user, backend } = await setupConnected();
    const del = vi.spyOn(backend, "sftpDelete");
    await user.click(row("README.md"));
    const tree = screen.getByRole("tree", { name: "Remote files" });
    act(() => tree.focus());
    await user.keyboard("{Delete}");
    await screen.findByRole("alertdialog", { name: "Delete README.md?" });
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(del).not.toHaveBeenCalled();
  });

  it("Copy path puts the remote path on the clipboard; Refresh reloads the folder", async () => {
    const { user, backend } = await setupConnected();
    const write = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText: write }, configurable: true });
    const list = vi.spyOn(backend, "sftpList");
    const menu = await menuFor(row("deploy.sh"));
    await user.click(within(menu).getByRole("menuitem", { name: "Copy path" }));
    expect(write).toHaveBeenCalledWith("/srv/app/deploy.sh");

    const menu2 = await menuFor(row("deploy.sh"));
    await user.click(within(menu2).getByRole("menuitem", { name: "Refresh" }));
    await waitFor(() => expect(list).toHaveBeenCalledWith(expect.any(String), "/srv/app"));
  });

  it("Open terminal here starts a terminal in that folder (a file uses its folder)", async () => {
    const { user, backend } = await setupConnected();
    await waitFor(() => expect(Object.keys(useStore.getState().terminals)).toHaveLength(1));
    const open = vi.spyOn(backend, "openTerminal");
    const menu = await menuFor(row("nginx"));
    await user.click(within(menu).getByRole("menuitem", { name: "Open terminal here" }));
    await waitFor(() => expect(open).toHaveBeenCalledOnce());
    expect(open.mock.calls[0]?.[2]).toBe("/srv/app/nginx");

    const menu2 = await menuFor(row("deploy.sh"));
    await user.click(within(menu2).getByRole("menuitem", { name: "Open terminal here" }));
    await waitFor(() => expect(open).toHaveBeenCalledTimes(2));
    expect(open.mock.calls[1]?.[2]).toBe("/srv/app");
  });

  it("names with U+FFFD are shown but every operation on them is disabled", async () => {
    const { backend, user, sessionId } = await setupConnected();
    const fs = (backend as unknown as { fs: { addLossyName(dir: string, name: string): void } }).fs;
    fs.addLossyName("/srv/app", "bad�name.txt");
    await user.click(screen.getByRole("button", { name: "Refresh" }));
    const lossy = await screen.findByRole("treeitem", { name: /bad.*name\.txt/ });
    expect(lossy.getAttribute("aria-disabled")).toBe("true");
    const open = vi.spyOn(backend, "sftpReadText");
    await user.click(lossy);
    expect(open).not.toHaveBeenCalled();
    const menu = await menuFor(lossy);
    for (const name of [/^Open$/, /^Rename/, /^Move/, /^Delete/, /^Copy path/, /^Open terminal here/, /^Refresh/]) {
      expect(within(menu).getByRole("menuitem", { name }).getAttribute("aria-disabled")).toBe("true");
    }
    expect(Object.keys(useStore.getState().buffers)).toHaveLength(0);
    expect(sessionId).toBeTruthy();
  });

  it("a lost connection keeps the cached listing, disables operations, and refreshes the expanded folders after the reconnect", async () => {
    const { user, backend, sessionId } = await setupConnected();
    await user.click(row("logs"));
    await screen.findByText("This folder is empty.");
    backend.loseConnection(sessionId);
    await screen.findByText("Connection lost. Showing the last known listing.");
    expect(screen.getByRole("treeitem", { name: /docker-compose\.yml/ })).toBeTruthy();
    expect(screen.getByRole("button", { name: "New file…" }).hasAttribute("disabled")).toBe(true);

    // a file appears in the expanded folder while we are away
    const fs = (backend as unknown as { fs: { create(p: string, k: "file" | "dir"): unknown } }).fs;
    fs.create("/srv/app/logs/app.log", "file");
    backend.restoreConnection(sessionId);
    await screen.findByRole("treeitem", { name: /app\.log/ });
    expect(screen.queryByText("Connection lost. Showing the last known listing.")).toBeNull();
  });

  it("the toolbar creates a folder inside the selected folder", async () => {
    const { user } = await setupConnected();
    await user.click(row("certs"));
    await user.click(screen.getByRole("button", { name: "New folder…" }));
    const input = await screen.findByRole("textbox", { name: "New folder name" });
    await user.type(input, "private{Enter}");
    await screen.findByRole("treeitem", { name: /private/ });
    expect(row("private").getAttribute("aria-level")).toBe("3");
  });

  describe("navigate above the start folder", () => {
    const rootOf = (sessionId: string) => useStore.getState().workspaces[sessionId]?.tree.root;

    it("Up re-roots at the parent and is disabled at /", async () => {
      const { user, sessionId } = await setupConnected();
      expect(rootOf(sessionId)).toBe("/srv/app");
      await user.click(screen.getByRole("button", { name: "Up one level" }));
      await waitFor(() => expect(rootOf(sessionId)).toBe("/srv"));
      await user.click(screen.getByRole("button", { name: "Up one level" }));
      await waitFor(() => expect(rootOf(sessionId)).toBe("/"));
      expect((screen.getByRole("button", { name: "Up one level" }) as HTMLButtonElement).disabled).toBe(true);
    });

    it("breadcrumb segments re-root the tree", async () => {
      const { user, sessionId } = await setupConnected();
      const nav = screen.getByRole("navigation", { name: "Current folder" });
      expect(within(nav).getAllByRole("button").map((b) => b.textContent)).toEqual(["/", "srv", "app"]);
      await user.click(within(nav).getByRole("button", { name: "srv" }));
      await waitFor(() => expect(rootOf(sessionId)).toBe("/srv"));
      await user.click(within(screen.getByRole("navigation", { name: "Current folder" })).getByRole("button", { name: "/" }));
      await waitFor(() => expect(rootOf(sessionId)).toBe("/"));
    });

    it("Ctrl+L in the tree and the context menu open Go to folder; ~ and absolute paths work, bad ones are explained", async () => {
      const { user, sessionId } = await setupConnected();
      const tree = screen.getByRole("tree", { name: "Remote files" });
      act(() => tree.focus());
      await user.keyboard("{Control>}l{/Control}");
      const input = await screen.findByRole("textbox", { name: "Folder path" });
      await user.clear(input);
      await user.type(input, "relative/path{Enter}");
      expect(await screen.findByText(/Invalid value/)).toBeTruthy();
      expect(rootOf(sessionId)).toBe("/srv/app");
      await user.clear(input);
      await user.type(input, "/srv/../home/{Enter}");
      await waitFor(() => expect(rootOf(sessionId)).toBe("/home"));
      expect(screen.queryByRole("textbox", { name: "Folder path" })).toBeNull();

      const menu = await menuFor(screen.getAllByRole("treeitem")[0] as HTMLElement);
      await user.click(within(menu).getByRole("menuitem", { name: /Go to folder/ }));
      const again = await screen.findByRole("textbox", { name: "Folder path" });
      await user.clear(again);
      await user.type(again, "~{Enter}");
      await waitFor(() => expect(rootOf(sessionId)).toBe("/home/deploy"));
    });

    it("a server error becomes a notice and leaves the root alone", async () => {
      const { user, sessionId } = await setupConnected();
      act(() => useStore.getState().openDialog({ kind: "goToFolder", sessionId }));
      const input = await screen.findByRole("textbox", { name: "Folder path" });
      await user.clear(input);
      await user.type(input, "/does/not/exist{Enter}");
      await waitFor(() => expect(useStore.getState().notices.some((n) => n.error?.code === "remoteNotFound")).toBe(true));
      expect(rootOf(sessionId)).toBe("/srv/app");
    });
  });
});
