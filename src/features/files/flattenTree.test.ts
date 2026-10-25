import { describe, expect, it } from "vitest";
import type { DirListing, FileEntry } from "../../ipc/types";
import type { TreeState } from "../../state/slices/workspaces";
import { compareEntries, entryRows, flattenTree, sortStats, type FlattenOptions } from "./flattenTree";

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

describe("file tree sort (size and date columns, §6.2)", () => {
  const names = (t: TreeState, options: FlattenOptions) => entryRows(flattenTree(t, options)).slice(1).map((r) => r.name);
  const tree: TreeState = {
    root: "/d",
    expanded: { "/d": true },
    selection: null,
    edit: null,
    listings: {
      "/d": listing("/d", [
        entry("/d/b.txt", "file", { size: 300, mtime: 30 }),
        entry("/d/a.txt", "file", { size: 100, mtime: 50 }),
        entry("/d/c.txt", "file", { size: 200, mtime: null }),
        entry("/d/zdir", "dir", { size: 4096, mtime: 10 }),
        entry("/d/adir", "dir", { size: 4096, mtime: 40 }),
        entry("/d/link-to-dir", "symlink", { linkTargetKind: "dir", size: 7, mtime: 20 }),
        entry("/d/link-to-file", "symlink", { linkTargetKind: "file", size: 9, mtime: 60 }),
        entry("/d/same1.txt", "file", { size: 300, mtime: 30 }),
      ]),
    },
  };

  it("name: folders (and links to folders) first, natural order; descending flips the names only", () => {
    expect(names(tree, { showHidden: true, sort: { key: "name", dir: "asc" } })).toEqual([
      "adir", "link-to-dir", "zdir", "a.txt", "b.txt", "c.txt", "link-to-file", "same1.txt",
    ]);
    expect(names(tree, { showHidden: true, sort: { key: "name", dir: "desc" } })).toEqual([
      "zdir", "link-to-dir", "adir", "same1.txt", "link-to-file", "c.txt", "b.txt", "a.txt",
    ]);
  });

  it("size: folders by name on top, files by size; links and folders have no size and go last without foldersFirst", () => {
    expect(names(tree, { showHidden: true, sort: { key: "size", dir: "desc" } })).toEqual([
      "adir", "link-to-dir", "zdir", "b.txt", "same1.txt", "c.txt", "a.txt", "link-to-file",
    ]);
    expect(names(tree, { showHidden: true, sort: { key: "size", dir: "asc" } })).toEqual([
      "adir", "link-to-dir", "zdir", "a.txt", "c.txt", "b.txt", "same1.txt", "link-to-file",
    ]);
    expect(names(tree, { showHidden: true, foldersFirst: false, sort: { key: "size", dir: "asc" } })).toEqual([
      "a.txt", "c.txt", "b.txt", "same1.txt", "adir", "link-to-dir", "link-to-file", "zdir",
    ]);
    expect(names(tree, { showHidden: true, foldersFirst: false, sort: { key: "size", dir: "desc" } })).toEqual([
      "b.txt", "same1.txt", "c.txt", "a.txt", "adir", "link-to-dir", "link-to-file", "zdir",
    ]);
  });

  it("mtime: folders by date on top; an unknown date goes last in both directions; ties by name", () => {
    expect(names(tree, { showHidden: true, sort: { key: "mtime", dir: "desc" } })).toEqual([
      "adir", "link-to-dir", "zdir", "link-to-file", "a.txt", "b.txt", "same1.txt", "c.txt",
    ]);
    expect(names(tree, { showHidden: true, sort: { key: "mtime", dir: "asc" } })).toEqual([
      "zdir", "link-to-dir", "adir", "b.txt", "same1.txt", "a.txt", "link-to-file", "c.txt",
    ]);
    expect(names(tree, { showHidden: true, foldersFirst: false, sort: { key: "mtime", dir: "asc" } })).toEqual([
      "zdir", "link-to-dir", "b.txt", "same1.txt", "adir", "a.txt", "link-to-file", "c.txt",
    ]);
  });

  it("ties are total and do not depend on the server order (case-only differences by code units)", () => {
    const collator = new Intl.Collator("en", { numeric: true, sensitivity: "base" });
    const a = entry("/d/Read.me", "file", { size: 5 });
    const b = entry("/d/read.me", "file", { size: 5 });
    for (const sort of [{ key: "name", dir: "asc" }, { key: "name", dir: "desc" }, { key: "size", dir: "desc" }, { key: "mtime", dir: "asc" }] as const) {
      expect(compareEntries(a, b, sort, true, collator)).toBeLessThan(0);
      expect(compareEntries(b, a, sort, true, collator)).toBeGreaterThan(0);
      expect(compareEntries(a, a, sort, true, collator)).toBe(0);
    }
  });

  it("sorts each folder level on its own, keeping the hierarchy", () => {
    const nested: TreeState = {
      ...tree,
      expanded: { "/d": true, "/d/adir": true },
      listings: {
        ...tree.listings,
        "/d/adir": listing("/d/adir", [entry("/d/adir/small", "file", { size: 1 }), entry("/d/adir/big", "file", { size: 999 })]),
      },
    };
    const rows = entryRows(flattenTree(nested, { showHidden: true, sort: { key: "size", dir: "desc" } }));
    expect(rows.slice(1, 4).map((r) => [r.name, r.depth])).toEqual([["adir", 1], ["big", 2], ["small", 2]]);
  });

  it("a new sort key or direction sorts once more and is cached as well", () => {
    const fresh: TreeState = { ...tree, listings: { "/d": listing("/d", [...tree.listings["/d"]!.listing.entries]) } };
    const before = sortStats.sorts;
    flattenTree(fresh, { showHidden: true });
    flattenTree(fresh, { showHidden: true, sort: { key: "name", dir: "asc" } }); // the default
    expect(sortStats.sorts).toBe(before + 1);
    flattenTree(fresh, { showHidden: true, sort: { key: "size", dir: "desc" } });
    flattenTree(fresh, { showHidden: true, sort: { key: "size", dir: "desc" } });
    expect(sortStats.sorts).toBe(before + 2);
    flattenTree(fresh, { showHidden: true, sort: { key: "size", dir: "asc" } });
    expect(sortStats.sorts).toBe(before + 3);
    // only the last two orders are kept per listing: flipping back and forth is free, a third one evicts the oldest
    flattenTree(fresh, { showHidden: true, sort: { key: "size", dir: "desc" } });
    flattenTree(fresh, { showHidden: true, sort: { key: "size", dir: "asc" } });
    expect(sortStats.sorts).toBe(before + 3);
    flattenTree(fresh, { showHidden: true, sort: { key: "name", dir: "asc" } }); // evicted earlier: sorted again
    expect(sortStats.sorts).toBe(before + 4);
    flattenTree(fresh, { showHidden: true, sort: { key: "size", dir: "asc" } }); // still cached (most recent)
    expect(sortStats.sorts).toBe(before + 4);
    flattenTree(fresh, { showHidden: true, sort: { key: "size", dir: "desc" } }); // evicted by "name"
    expect(sortStats.sorts).toBe(before + 5);
  });

  it("sorts a 100 000-entry folder by each key within a loose time bound", () => {
    const entries = Array.from({ length: 100_000 }, (_, i) => {
      const n = (i * 7919) % 100_000;
      return entry(`/huge/file-${n}.log`, i % 50 === 0 ? "dir" : "file", { size: (n * 2654435761) % 1_000_000_007, mtime: n % 3 === 0 ? null : 1_700_000_000 + ((n * 31) % 86_400) });
    });
    const huge: TreeState = { root: "/huge", expanded: { "/huge": true }, selection: null, edit: null, listings: { "/huge": listing("/huge", entries) } };
    for (const sort of [{ key: "size", dir: "desc" }, { key: "mtime", dir: "desc" }, { key: "name", dir: "asc" }] as const) {
      const started = performance.now();
      const rows = flattenTree(huge, { showHidden: true, sort });
      const elapsed = performance.now() - started;
      expect(rows).toHaveLength(100_001);
      // jsdom on a slow CI runner: a loose bound that still catches an accidental O(n²) or a collator per compare.
      expect(elapsed).toBeLessThan(3000);
    }
  });
});
