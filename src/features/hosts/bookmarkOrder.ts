// Pure helpers for moving bookmarks and groups (drag and drop, "Move up/down", "Move to group…").
// Positions follow the backend contract (ARCHITECTURE §4.2): `index` is the FINAL position in the target list.
import type { Bookmark, BookmarksDto, Group } from "../../ipc/types";

/** Bookmarks of one list (`null` = ungrouped; a group that no longer exists counts as ungrouped), in order. */
export function bookmarksOfGroup(dto: BookmarksDto, groupId: string | null): Bookmark[] {
  const known = new Set(dto.groups.map((g) => g.id));
  return dto.bookmarks
    .filter((b) => (b.groupId !== null && known.has(b.groupId) ? b.groupId : null) === groupId)
    .sort((a, b) => a.order - b.order || a.name.localeCompare(b.name));
}

export function sortedGroups(dto: BookmarksDto): Group[] {
  return [...dto.groups].sort((a, b) => a.order - b.order);
}

/**
 * The final index when the item currently at `from` is dropped into the slot `slot` of the SAME list
 * (slot = the gap before item `slot`, counted in the list that still contains the dragged item).
 */
export function finalIndexInSameList(from: number, slot: number): number {
  return slot > from ? slot - 1 : slot;
}

export interface MoveTarget {
  groupId: string | null;
  index: number;
}

/** "Move up" / "Move down": the target inside the same list, or null at the edge. */
export function stepTarget(dto: BookmarksDto, bookmarkId: string, delta: -1 | 1): MoveTarget | null {
  const bookmark = dto.bookmarks.find((b) => b.id === bookmarkId);
  if (!bookmark) return null;
  const list = bookmarksOfGroup(dto, bookmark.groupId);
  const from = list.findIndex((b) => b.id === bookmarkId);
  const to = from + delta;
  if (from < 0 || to < 0 || to >= list.length) return null;
  return { groupId: bookmark.groupId, index: to };
}

export function stepGroupTarget(dto: BookmarksDto, groupId: string, delta: -1 | 1): number | null {
  const list = sortedGroups(dto);
  const from = list.findIndex((g) => g.id === groupId);
  const to = from + delta;
  if (from < 0 || to < 0 || to >= list.length) return null;
  return to;
}
