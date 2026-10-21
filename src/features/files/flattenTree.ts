import type { DirListing, FileEntry } from "../../ipc/types";
import { basename } from "../../lib/remotePath";
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

function sortedChildren(listing: DirListing, showHidden: boolean, foldersFirst: boolean, locale: string): FileEntry[] {
  const key = `${showHidden ? 1 : 0}${foldersFirst ? 1 : 0}|${locale}`;
  let byKey = sortedCache.get(listing);
  if (!byKey) {
    byKey = new Map();
    sortedCache.set(listing, byKey);
  }
  const hit = byKey.get(key);
  if (hit) return hit;
  sortStats.sorts += 1;
  const collator = collatorFor(locale);
  const sorted = listing.entries
    .filter((c) => showHidden || !c.name.startsWith("."))
    .sort((a, b) => {
      const aDir = foldersFirst && dirLike(a) ? 0 : 1;
      const bDir = foldersFirst && dirLike(b) ? 0 : 1;
      return aDir - bDir || collator.compare(a.name, b.name);
    });
  byKey.set(key, sorted);
  return sorted;
}

export interface FlattenOptions {
  showHidden: boolean;
  /** Directories before files (default true); off = one natural-sorted list. */
  foldersFirst?: boolean;
  locale?: string;
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
    const entries = sortedChildren(cached.listing, options.showHidden, options.foldersFirst ?? true, locale);
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
