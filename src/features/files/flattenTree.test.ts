import { describe, expect, it } from "vitest";
import type { DirListing, FileEntry } from "../../ipc/types";
import type { TreeState } from "../../state/slices/workspaces";
import { entryRows, flattenTree, sortStats } from "./flattenTree";

function entry(path: string, kind: FileEntry["kind"], extra: Partial<FileEntry> = {}): FileEntry {
  const name = path.slice(path.lastIndexOf("/") + 1);
  return {
    name,
    path,
    kind,
    linkTargetKind: null,
    size: 1,
    mtime: 1,
    permissions: 0o644,
    uid: 0,
    gid: 0,
    owner: null,
    group: null,
    lossyName: false,
    ...extra,
  };
}

function listing(path: string, entries: FileEntry[], truncated = false): { listing: DirListing; fetchedAtMs: number; stale: boolean } {
  return { listing: { path, entries, truncated }, fetchedAtMs: 0, stale: false };
}

describe("flattenTree", () => {
  const tree: TreeState = {
    root: "/srv/app",
    expanded: { "/srv/app": true, "/srv/app/nginx": true },
    selection: null,
    edit: null,
    listings: {
      "/srv/app": listing("/srv/app", [
        entry("/srv/app/README.md", "file"),
        entry("/srv/app/.env", "file"),
        entry("/srv/app/nginx", "dir"),
        entry("/srv/app/certs", "dir"),
        entry("/srv/app/file10.txt", "file"),
        entry("/srv/app/file2.txt", "file"),
      ]),
      "/srv/app/nginx": listing("/srv/app/nginx", [entry("/srv/app/nginx/nginx.conf", "file")]),
    },
  };

  it("lists the root first, directories before files, natural order", () => {
    const rows = entryRows(flattenTree(tree, { showHidden: true }));
    expect(rows.map((r) => r.name)).toEqual(["app", "certs", "nginx", "nginx.conf", ".env", "file2.txt", "file10.txt", "README.md"]);
    expect(rows.map((r) => r.depth)).toEqual([0, 1, 1, 2, 1, 1, 1, 1]);
  });

  it("hides dotfiles when showHidden is false", () => {
    const rows = entryRows(flattenTree(tree, { showHidden: false }));
    expect(rows.some((r) => r.name === ".env")).toBe(false);
  });

  it("marks an expanded directory without a listing as loading and adds a loading note", () => {
    const rows = flattenTree({ ...tree, expanded: { ...tree.expanded, "/srv/app/certs": true } }, { showHidden: true });
    expect(entryRows(rows).find((r) => r.name === "certs")?.loading).toBe(true);
    expect(rows.some((r) => r.type === "note" && r.note === "loading" && r.path === "/srv/app/certs")).toBe(true);
  });

  it("returns nothing without a root", () => {
    expect(flattenTree({ ...tree, root: null }, { showHidden: true })).toEqual([]);
  });

  it("ends a truncated listing with a notice row that carries the entry count", () => {
    const crowded: TreeState = {
      ...tree,
      listings: { ...tree.listings, "/srv/app": listing("/srv/app", [entry("/srv/app/a.txt", "file"), entry("/srv/app/b.txt", "file")], true) },
    };
    const rows = flattenTree(crowded, { showHidden: true });
    const last = rows.filter((r) => r.type === "note" && r.path === "/srv/app").at(-1);
    expect(last).toMatchObject({ type: "note", note: "truncated", count: 2 });
  });

  it("shows an empty-folder note, and the naming input as the first row of a folder being filled", () => {
    const empty: TreeState = { ...tree, expanded: { ...tree.expanded, "/srv/app/certs": true }, listings: { ...tree.listings, "/srv/app/certs": listing("/srv/app/certs", []) } };
    expect(flattenTree(empty, { showHidden: true }).some((r) => r.type === "note" && r.note === "empty")).toBe(true);

    const naming = flattenTree({ ...empty, edit: { kind: "new", parent: "/srv/app/certs", entry: "file" } }, { showHidden: true });
    const index = naming.findIndex((r) => r.type === "new");
    expect(naming[index - 1]).toMatchObject({ type: "entry", path: "/srv/app/certs" });
    expect(naming.some((r) => r.type === "note" && r.note === "empty")).toBe(false);
  });

  it("treats a symlink to a directory as expandable, a symlink to a file as a leaf", () => {
    const links: TreeState = {
      ...tree,
      expanded: { "/srv/app": true },
      listings: {
        "/srv/app": listing("/srv/app", [
          entry("/srv/app/current", "symlink", { linkTargetKind: "dir" }),
          entry("/srv/app/latest.log", "symlink", { linkTargetKind: "file" }),
        ]),
      },
    };
    const rows = entryRows(flattenTree(links, { showHidden: true }));
    expect(rows.find((r) => r.name === "current")).toMatchObject({ dirLike: true });
    expect(rows.find((r) => r.name === "latest.log")).toMatchObject({ dirLike: false });
    expect(rows[1]?.name).toBe("current"); // directories (and links to them) first
  });

  it("flags names with U+FFFD as lossy and never expands them", () => {
    const lossy: TreeState = {
      ...tree,
      expanded: { "/srv/app": true, "/srv/app/bad�dir": true },
      listings: { "/srv/app": listing("/srv/app", [entry("/srv/app/bad�dir", "dir", { lossyName: true })]) },
    };
    const rows = flattenTree(lossy, { showHidden: true });
    expect(entryRows(rows).find((r) => r.lossy)).toMatchObject({ expanded: false, dirLike: true });
    expect(rows).toHaveLength(2);
  });

  it("numbers siblings for the accessibility tree", () => {
    const rows = entryRows(flattenTree(tree, { showHidden: true }));
    const nginx = rows.find((r) => r.name === "nginx");
    expect(nginx).toMatchObject({ posInSet: 2, setSize: 6 });
  });

  it("sorts a listing once: selection, edit and expansion changes reuse the sorted children", () => {
    const big: TreeState = {
      root: "/big",
      expanded: { "/big": true },
      selection: null,
      edit: null,
      listings: { "/big": listing("/big", Array.from({ length: 20000 }, (_, i) => entry(`/big/f${(i * 7919) % 20000}.txt`, "file"))) },
    };
    const before = sortStats.sorts;
    flattenTree(big, { showHidden: true });
    expect(sortStats.sorts).toBe(before + 1);
    flattenTree({ ...big, selection: "/big/f1.txt" }, { showHidden: true });
    flattenTree({ ...big, edit: { kind: "rename", path: "/big/f2.txt" } }, { showHidden: true });
    flattenTree({ ...big, edit: { kind: "new", parent: "/big", entry: "file" } }, { showHidden: true });
    expect(sortStats.sorts).toBe(before + 1);
    // a different filter or a new listing object is sorted again
    flattenTree(big, { showHidden: false });
    expect(sortStats.sorts).toBe(before + 2);
    flattenTree({ ...big, listings: { "/big": { ...big.listings["/big"]!, listing: { ...big.listings["/big"]!.listing } } } }, { showHidden: true });
    expect(sortStats.sorts).toBe(before + 3);
  });
});
