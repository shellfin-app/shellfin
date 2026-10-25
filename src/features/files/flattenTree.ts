import type { DirListing, FileEntry } from "../../ipc/types";
import { basename } from "../../lib/remotePath";
import { FILES_SORT_DEFAULT, type FilesSort } from "../../state/layoutPrefs";
import type { TreeState } from "../../state/slices/workspaces";

export interface EntryRow {
  type: "entry";
  path: string;
  name: string;
  kind: FileEntry["kind"];
  linkTargetKind: FileEntry["linkTargetKind"];
  depth: number;
  /** A folder (or a symlink to one) that can be expanded. */
  dirLike: boolean;
  expanded: boolean;
  /** Expanded but the listing has not arrived yet. */
  loading: boolean;
  /** The name contains U+FFFD: the real name is unknown, so every operation on it is disabled (§6.2). */
  lossy: boolean;
  isRoot: boolean;
  /** Position among its siblings (for the accessibility tree; rows are virtualized). */
  posInSet: number;
  setSize: number;
  /** This entry is being renamed (the row shows an input). */
  renaming: boolean;
}

/** A line of text inside a folder: loading, empty, or the truncation notice. */
export interface NoteRow {
  type: "note";
  key: string;
  note: "loading" | "empty" | "truncated";
  /** The folder the note belongs to. */
  path: string;
  depth: number;
  /** Entries that were listed (for the truncation notice). */
  count: number;
}

/** The input row for naming a new file or folder. */
export interface NewRow {
  type: "new";
  key: string;
  entry: "file" | "dir";
  parent: string;
  depth: number;
}

export type TreeRow = EntryRow | NoteRow | NewRow;

/** How many times a listing was filtered and sorted (tests assert that tree changes do not repeat it). */
export const sortStats = { sorts: 0 };

const collators = new Map<string, Intl.Collator>();
function collatorFor(locale: string): Intl.Collator {
  let c = collators.get(locale);
  if (!c) {
    c = new Intl.Collator(locale, { numeric: true, sensitivity: "base" });
    collators.set(locale, c);
  }
  return c;
}

/** Sorted, filtered children per listing OBJECT (the store keeps its identity while the tree changes around it). */
const sortedCache = new WeakMap<DirListing, Map<string, FileEntry[]>>();
const SORTED_PER_LISTING = 2;

/** The size a column shows and sorts by: files only (folders, symlinks and unknown kinds have none, §8.11). */
function sizeKey(e: FileEntry): number | null {
  return e.kind === "file" ? e.size : null;
}

/** Code-unit order: the final tiebreak, so the order is total and the same after every refresh. */
function rawCompare(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

/**
 * The file tree order (§6.2): folders first (when `foldersFirst`), then the sort key in its direction; a missing
 * key (no size, unknown mtime) goes last in both directions; ties by the natural name order, then by code units.
 * With `foldersFirst` and the size key, folders (which have no size) are ordered by name.
 */
export function compareEntries(a: FileEntry, b: FileEntry, sort: FilesSort, foldersFirst: boolean, collator: Intl.Collator): number {
  const aDir = dirLike(a);
  const bDir = dirLike(b);
  if (foldersFirst && aDir !== bDir) return aDir ? -1 : 1;
  const sign = sort.dir === "desc" ? -1 : 1;
  if (sort.key === "name") return sign * collator.compare(a.name, b.name) || rawCompare(a.name, b.name);
  if (!(sort.key === "size" && foldersFirst && aDir && bDir)) {
    const av = sort.key === "size" ? sizeKey(a) : a.mtime;
    const bv = sort.key === "size" ? sizeKey(b) : b.mtime;
    if (av === null || bv === null) {
      if (av !== bv) return av === null ? 1 : -1;
    } else if (av !== bv) {
      return av < bv ? -sign : sign;
    }
  }
  return collator.compare(a.name, b.name) || rawCompare(a.name, b.name);
}

function sortedChildren(listing: DirListing, showHidden: boolean, foldersFirst: boolean, locale: string, sort: FilesSort): FileEntry[] {
  const key = `${showHidden ? 1 : 0}${foldersFirst ? 1 : 0}|${sort.key}|${sort.dir}|${locale}`;
  let byKey = sortedCache.get(listing);
  if (!byKey) {
    byKey = new Map();
    sortedCache.set(listing, byKey);
  }
  const hit = byKey.get(key);
  if (hit) {
    // Most recently used last (Map order), so the eviction below drops the oldest.
    byKey.delete(key);
    byKey.set(key, hit);
    return hit;
  }
  sortStats.sorts += 1;
  const collator = collatorFor(locale);
  const sorted = listing.entries
    .filter((c) => showHidden || !c.name.startsWith("."))
    .sort((a, b) => compareEntries(a, b, sort, foldersFirst, collator));
  byKey.set(key, sorted);
  // A sorted copy per key would otherwise pile up (key × direction × filter × locale); keep the last two, which
  // covers flipping back and forth. Each copy is an array of references, ~0.8 MB for 100 000 entries.
  while (byKey.size > SORTED_PER_LISTING) {
    const oldest = byKey.keys().next().value;
    if (oldest === undefined) break;
    byKey.delete(oldest);
  }
  return sorted;
}

export interface FlattenOptions {
  showHidden: boolean;
  /** Directories before files (default true); off = one natural-sorted list. */
  foldersFirst?: boolean;
  locale?: string;
  /** Sort key and direction (default name, ascending). */
  sort?: FilesSort;
}

function dirLike(e: Pick<FileEntry, "kind" | "linkTargetKind">): boolean {
  return e.kind === "dir" || (e.kind === "symlink" && e.linkTargetKind === "dir");
}

/**
 * Flattens the expanded part of the tree into visible rows (root first, directories before files, natural
 * sort). Listings are keyed by directory path (§6.2, §8.3). A truncated listing ends with a notice row; the
 * naming input for a new entry is the first row inside its folder.
 */
export function flattenTree(tree: TreeState, options: FlattenOptions): TreeRow[] {
  const { root } = tree;
  if (root === null) return [];
  const locale = options.locale ?? "en";
  const sort = options.sort ?? FILES_SORT_DEFAULT;
  const rows: TreeRow[] = [];

  const visit = (e: FileEntry, depth: number, posInSet: number, setSize: number, isRoot: boolean) => {
    const isDir = dirLike(e);
    const expanded = isDir && !e.lossyName && tree.expanded[e.path] === true;
    const cached = tree.listings[e.path];
    rows.push({
      type: "entry",
      path: e.path,
      name: e.name,
      kind: e.kind,
      linkTargetKind: e.linkTargetKind,
      depth,
      dirLike: isDir,
      expanded,
      loading: expanded && !cached,
      lossy: e.lossyName,
      isRoot,
      posInSet,
      setSize,
      renaming: tree.edit?.kind === "rename" && tree.edit.path === e.path,
    });
    if (!expanded) return;
    if (tree.edit?.kind === "new" && tree.edit.parent === e.path) {
      rows.push({ type: "new", key: `new:${e.path}`, entry: tree.edit.entry, parent: e.path, depth: depth + 1 });
    }
    if (!cached) {
      rows.push({ type: "note", key: `loading:${e.path}`, note: "loading", path: e.path, depth: depth + 1, count: 0 });
      return;
    }
    const entries = sortedChildren(cached.listing, options.showHidden, options.foldersFirst ?? true, locale, sort);
    entries.forEach((c, i) => visit(c, depth + 1, i + 1, entries.length, false));
    if (entries.length === 0 && !(tree.edit?.kind === "new" && tree.edit.parent === e.path)) {
      rows.push({ type: "note", key: `empty:${e.path}`, note: "empty", path: e.path, depth: depth + 1, count: 0 });
    }
    if (cached.listing.truncated) {
      rows.push({ type: "note", key: `truncated:${e.path}`, note: "truncated", path: e.path, depth: depth + 1, count: cached.listing.entries.length });
    }
  };

  visit(
    {
      name: basename(root),
      path: root,
      kind: "dir",
      linkTargetKind: "dir",
      size: null,
      mtime: null,
      permissions: null,
      uid: null,
      gid: null,
      owner: null,
      group: null,
      lossyName: false,
    },
    0,
    1,
    1,
    true,
  );
  return rows;
}

/** Only the entry rows (keyboard navigation skips notes and the input row). */
export function entryRows(rows: readonly TreeRow[]): EntryRow[] {
  return rows.filter((r): r is EntryRow => r.type === "entry");
}
