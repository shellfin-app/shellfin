import { beforeEach, describe, expect, it } from "vitest";
import type { DirListing } from "../ipc/types";
import { resetStore, useStore } from "./store";

const listing = (path: string): DirListing => ({ path, entries: [], truncated: false });

function session() {
  const s = useStore.getState();
  s.addSession("s1", { kind: "sshConfig", alias: "a" }, "a");
  s.ensureWorkspace("s1");
  s.setTreeRoot("s1", "/srv/app");
}

describe("tree state", () => {
  beforeEach(() => {
    resetStore();
    session();
  });

  it("markListingsStale flags every cached listing", () => {
    const s = useStore.getState();
    s.setListing("s1", "/srv/app", listing("/srv/app"), 1);
    s.setListing("s1", "/srv/app/x", listing("/srv/app/x"), 1);
    s.markListingsStale("s1");
    const listings = useStore.getState().workspaces["s1"]?.tree.listings ?? {};
    expect(Object.values(listings).every((l) => l.stale)).toBe(true);
    s.setListing("s1", "/srv/app", listing("/srv/app"), 2);
    expect(useStore.getState().workspaces["s1"]?.tree.listings["/srv/app"]?.stale).toBe(false);
  });

  it("moveTreePath re-keys expansion and selection and drops the listings below the old path", () => {
    const s = useStore.getState();
    s.setListing("s1", "/srv/app/a", listing("/srv/app/a"), 1);
    s.setListing("s1", "/srv/app/a/b", listing("/srv/app/a/b"), 1);
    s.setListing("s1", "/srv/app/ab", listing("/srv/app/ab"), 1);
    s.toggleExpanded("s1", "/srv/app/a");
    s.toggleExpanded("s1", "/srv/app/a/b");
    s.selectTreeEntry("s1", "/srv/app/a/b");
    s.moveTreePath("s1", "/srv/app/a", "/srv/app/z");
    const tree = useStore.getState().workspaces["s1"]?.tree;
    expect(Object.keys(tree?.expanded ?? {}).sort()).toEqual(["/srv/app", "/srv/app/z", "/srv/app/z/b"]);
    expect(Object.keys(tree?.listings ?? {})).toEqual(["/srv/app/ab"]); // "ab" is a sibling, not below "a"
    expect(tree?.selection).toBe("/srv/app/z/b");
  });

  it("forgetTreePath removes cache, expansion and selection of a deleted folder", () => {
    const s = useStore.getState();
    s.setListing("s1", "/srv/app/a", listing("/srv/app/a"), 1);
    s.toggleExpanded("s1", "/srv/app/a");
    s.selectTreeEntry("s1", "/srv/app/a");
    s.forgetTreePath("s1", "/srv/app/a");
    const tree = useStore.getState().workspaces["s1"]?.tree;
    expect(tree?.listings["/srv/app/a"]).toBeUndefined();
    expect(tree?.expanded["/srv/app/a"]).toBeUndefined();
    expect(tree?.selection).toBeNull();
  });

  it("starting to name a new entry expands its parent", () => {
    useStore.getState().setTreeEdit("s1", { kind: "new", parent: "/srv/app/c", entry: "dir" });
    expect(useStore.getState().workspaces["s1"]?.tree.expanded["/srv/app/c"]).toBe(true);
  });
});

describe("buffers and transfers", () => {
  beforeEach(() => {
    resetStore();
    session();
  });

  it("addBuffer fills the defaults; renameBuffer moves the tab and the active marker", () => {
    const s = useStore.getState();
    s.addBuffer({ id: "s1:/a", sessionId: "s1", path: "/a" });
    s.openEditorTab("s1", "s1:/a");
    expect(useStore.getState().buffers["s1:/a"]).toMatchObject({ load: "loading", dirty: false, pendingSave: false, conflict: null, rev: 0 });
    s.renameBuffer("s1:/a", "s1:/b", "/b");
    const state = useStore.getState();
    expect(state.buffers["s1:/a"]).toBeUndefined();
    expect(state.buffers["s1:/b"]?.path).toBe("/b");
    expect(state.workspaces["s1"]?.editorTabs).toEqual(["s1:/b"]);
    expect(state.workspaces["s1"]?.activeBuffer).toBe("s1:/b");
  });

  it("removing a session removes its transfers", () => {
    const s = useStore.getState();
    const base = { bytes: 0, total: 1, status: "running" as const, retryable: false, error: null, remoteDir: null, direction: "upload" as const };
    s.upsertTransfer({ ...base, transferId: "t1", sessionId: "s1", name: "a" });
    s.upsertTransfer({ ...base, transferId: "t2", sessionId: "other", name: "b" });
    s.removeSession("s1");
    expect(useStore.getState().transfers.map((t) => t.transferId)).toEqual(["t2"]);
  });

  it("info notices are de-duplicated like errors", () => {
    const s = useStore.getState();
    s.pushInfo({ kind: "dropNoTarget" });
    s.pushInfo({ kind: "dropNoTarget" });
    s.pushInfo({ kind: "foldersUnsupported" });
    expect(useStore.getState().notices.map((n) => n.info?.kind)).toEqual(["dropNoTarget", "foldersUnsupported"]);
  });
});
