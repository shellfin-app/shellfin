import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import { useStore } from "../../state/store";
import { settleApp, setupConnected } from "../../test/harness";
import { removeHint } from "./fileActions";

afterEach(async () => {
  await settleApp();
  setBackend(null);
  vi.restoreAllMocks();
});

/** Makes the loaded root listing look like a root-owned, read-only-for-us directory with a root-owned file. */
function lockRootListing(sessionId: string, dirMode: number) {
  const tree = useStore.getState().workspaces[sessionId]?.tree;
  const root = tree?.root ?? "";
  const cached = tree?.listings[root];
  if (!cached) throw new Error("no root listing");
  const entries = cached.listing.entries.map((e) =>
    e.name === "README.md" ? { ...e, uid: 0, gid: 0, permissions: 0o644 } : { ...e, uid: 1000, gid: 1000, permissions: e.kind === "dir" ? 0o755 : 0o644 },
  );
  act(() => useStore.getState().setListing(sessionId, root, { ...cached.listing, entries }, Date.now()));
  // the root directory itself is only known through its parent's listing (or not at all): store a parent listing
  const parent = root.slice(0, root.lastIndexOf("/")) || "/";
  const name = root.slice(root.lastIndexOf("/") + 1);
  act(() =>
    useStore.getState().setListing(
      sessionId,
      parent,
      {
        ...cached.listing,
        entries: [
          {
            name,
            path: root,
            kind: "dir",
            linkTargetKind: null,
            size: null,
            mtime: 1,
            permissions: dirMode,
            uid: 0,
            gid: 0,
            owner: null,
            group: null,
            lossyName: false,
          },
        ],
      },
      Date.now(),
    ),
  );
}

describe("Permission hints", () => {
  it("dims files the mode bits say we cannot write, and keeps everything else normal", async () => {
    const { sessionId } = await setupConnected();
    lockRootListing(sessionId, 0o755);
    const readme = await screen.findByRole("treeitem", { name: /README\.md/ });
    await waitFor(() => expect(readme.className).toContain("file-row--readonly"));
    expect(readme.getAttribute("title")).toContain("You probably can't change this file");
    expect(screen.getByRole("treeitem", { name: /docker-compose\.yml/ }).className).not.toContain("file-row--readonly");
  });

  it("Rename, Move and Delete stay enabled but warn when the parent folder is probably not writable", async () => {
    const { sessionId } = await setupConnected();
    lockRootListing(sessionId, 0o755);
    fireEvent.contextMenu(await screen.findByRole("treeitem", { name: /docker-compose\.yml/ }), { clientX: 10, clientY: 10 });
    const menu = await screen.findByRole("menu");
    const rename = within(menu).getByRole("menuitem", { name: /Rename/ });
    const del = within(menu).getByRole("menuitem", { name: /Delete/ });
    expect(rename.getAttribute("aria-disabled")).not.toBe("true");
    expect(del.getAttribute("aria-disabled")).not.toBe("true");
    expect(within(menu).getByRole("menuitem", { name: /Move/ }).getAttribute("aria-disabled")).not.toBe("true");
    expect(rename.getAttribute("title")).toContain("You probably can't change items in this folder");
  });

  it("The delete confirmation says it will probably fail, and the user can still try", async () => {
    const { user, sessionId } = await setupConnected();
    lockRootListing(sessionId, 0o755);
    fireEvent.contextMenu(await screen.findByRole("treeitem", { name: /docker-compose\.yml/ }), { clientX: 10, clientY: 10 });
    await user.click(within(await screen.findByRole("menu")).getByRole("menuitem", { name: /Delete/ }));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText(/will probably fail/)).toBeTruthy();
    expect(within(dialog).getByRole("button", { name: "Delete" }).hasAttribute("disabled")).toBe(false);
  });

  it("A folder we cannot enter gets the dim/lock hint", async () => {
    const { sessionId } = await setupConnected();
    const tree = useStore.getState().workspaces[sessionId]?.tree;
    const root = tree?.root ?? "";
    const cached = tree?.listings[root];
    if (!cached) throw new Error("no root listing");
    const dirName = cached.listing.entries.find((e) => e.kind === "dir")?.name;
    const entries = cached.listing.entries.map((e) =>
      e.name === dirName ? { ...e, uid: 0, gid: 0, permissions: 0o700 } : { ...e, uid: 1000, gid: 1000, permissions: e.kind === "dir" ? 0o755 : 0o644 },
    );
    act(() => useStore.getState().setListing(sessionId, root, { ...cached.listing, entries }, Date.now()));
    const row = await screen.findByRole("treeitem", { name: new RegExp(dirName ?? "") });
    await waitFor(() => expect(row.className).toContain("file-row--readonly"));
    expect(row.getAttribute("title")).toContain("can't open this folder");
  });

  it("The root's parent listing is fetched in the background so the hint has the root's mode bits", async () => {
    const { sessionId } = await setupConnected();
    const root = useStore.getState().workspaces[sessionId]?.tree.root ?? "";
    const parent = root.slice(0, root.lastIndexOf("/")) || "/";
    await waitFor(() => expect(useStore.getState().workspaces[sessionId]?.tree.listings[parent]).toBeDefined());
    // The root folder itself now has real mode bits, so the children get a real answer.
    expect(removeHint(sessionId, `${root}/README.md`)).not.toBe("unknown");
  });

  it("shows no hint when the backend did not report the user's ids", async () => {
    const { sessionId } = await setupConnected();
    act(() => {
      const entry = useStore.getState().sessionsById[sessionId];
      if (entry?.info) useStore.setState((s) => void (s.sessionsById[sessionId]!.info!.remoteIdentity = null));
    });
    lockRootListing(sessionId, 0o755);
    const readme = await screen.findByRole("treeitem", { name: /README\.md/ });
    expect(readme.className).not.toContain("file-row--readonly");
  });

  it("the editor shows a read-only banner for such a file, but saving stays possible", async () => {
    const { user, sessionId } = await setupConnected();
    lockRootListing(sessionId, 0o755);
    await user.dblClick(await screen.findByRole("treeitem", { name: /README\.md/ }));
    const banner = await screen.findByText(/You probably can't save this file/);
    expect(banner).toBeTruthy();
  });
});
