// Drag-to-reorder with POINTER events. HTML5 drag and drop does not work on Windows in this app (Tauri replaces the
// WebView2 drop target), so a drag is: pointerdown -> move past a threshold -> drop indicator -> pointerup (drop) or
// Escape / pointercancel (cancel). The list auto-scrolls near its edges.
import { SEL_DROP_GROUP_SECTION, SEL_DROP_HEADER, SEL_DROP_SECTION, SEL_HOST_ROW } from "../../lib/domIds";
import { useCallback, useEffect, useRef, useState, type PointerEvent as ReactPointerEvent, type RefObject } from "react";

/** Pointer travel (px) before a press becomes a drag; below it the press stays a click. */
export const DRAG_THRESHOLD = 5;
/** Distance from the list edge where auto-scroll starts, and its maximum speed (px per tick). */
export const SCROLL_ZONE = 28;
export const SCROLL_MAX_STEP = 14;
const SCROLL_TICK_MS = 16;

/** Scroll step for a pointer at `y` in a list spanning [top, bottom]: negative = up, 0 = none. */
export function autoScrollDelta(y: number, top: number, bottom: number): number {
  if (y < top + SCROLL_ZONE) return -Math.ceil((SCROLL_MAX_STEP * Math.min(SCROLL_ZONE, top + SCROLL_ZONE - y)) / SCROLL_ZONE);
  if (y > bottom - SCROLL_ZONE) return Math.ceil((SCROLL_MAX_STEP * Math.min(SCROLL_ZONE, y - (bottom - SCROLL_ZONE))) / SCROLL_ZONE);
  return 0;
}

// ---- geometry (pure; the DOM measuring lives in `measureSections`) ----

export interface RowGeom {
  key: string;
  top: number;
  bottom: number;
}
export interface SectionGeom {
  /** `null` = the ungrouped list; for a group its id. Sections that accept no drop are not passed in. */
  groupId: string | null;
  headerTop: number;
  headerBottom: number;
  sectionBottom: number;
  collapsed: boolean;
  rows: RowGeom[];
}
export interface BookmarkDrop {
  groupId: string | null;
  /** The gap before item `slot` of the list that still contains the dragged item. */
  slot: number;
  /** Where the indicator line is drawn (client coordinates). */
  lineY: number;
}

/** Which gap of which list a pointer at `y` points to (null = not over a list that accepts the drop). */
export function computeBookmarkDrop(y: number, sections: readonly SectionGeom[]): BookmarkDrop | null {
  for (const s of sections) {
    if (y < s.headerTop || y >= s.sectionBottom) continue;
    if (y < s.headerBottom) return { groupId: s.groupId, slot: s.collapsed ? s.rows.length : 0, lineY: s.headerBottom };
    if (s.collapsed || s.rows.length === 0) return { groupId: s.groupId, slot: 0, lineY: s.headerBottom };
    for (let i = 0; i < s.rows.length; i += 1) {
      const row = s.rows[i];
      if (!row) continue;
      const next = s.rows[i + 1];
      const spanBottom = next ? next.top : s.sectionBottom;
      if (y >= spanBottom) continue;
      return (row.top + row.bottom) / 2 > y ? { groupId: s.groupId, slot: i, lineY: row.top } : { groupId: s.groupId, slot: i + 1, lineY: row.bottom };
    }
  }
  return null;
}

export interface GroupGeom {
  id: string;
  top: number;
  bottom: number;
}
export interface GroupDrop {
  slot: number;
  lineY: number;
}

/** The gap among the groups a pointer at `y` points to; above the first group is slot 0, below the last the end. */
export function computeGroupDrop(y: number, groups: readonly GroupGeom[]): GroupDrop | null {
  const first = groups[0];
  const last = groups[groups.length - 1];
  if (!first || !last) return null;
  if (y < first.top) return { slot: 0, lineY: first.top };
  for (let i = 0; i < groups.length; i += 1) {
    const g = groups[i];
    if (!g) continue;
    const next = groups[i + 1];
    const spanBottom = next ? next.top : Number.POSITIVE_INFINITY;
    if (y >= spanBottom) continue;
    return (g.top + g.bottom) / 2 > y ? { slot: i, lineY: g.top } : { slot: i + 1, lineY: g.bottom };
  }
  return { slot: groups.length, lineY: last.bottom };
}

// ---- DOM measuring (rects of the rendered list) ----

/** Reads the geometry of the draggable sections: `[data-drop-section]` with `data-drop-group` ("" = ungrouped). */
export function measureSections(list: HTMLElement): SectionGeom[] {
  const out: SectionGeom[] = [];
  for (const el of list.querySelectorAll<HTMLElement>(SEL_DROP_SECTION)) {
    const header = el.querySelector<HTMLElement>(SEL_DROP_HEADER);
    if (!header) continue;
    const r = el.getBoundingClientRect();
    const h = header.getBoundingClientRect();
    const group = el.dataset["dropGroup"] ?? "";
    out.push({
      groupId: group === "" ? null : group,
      headerTop: h.top,
      headerBottom: h.bottom,
      sectionBottom: r.bottom,
      collapsed: el.dataset["collapsed"] === "true",
      rows: [...el.querySelectorAll<HTMLElement>(SEL_HOST_ROW)].map((row) => {
        const rr = row.getBoundingClientRect();
        return { key: row.dataset["hostKey"] ?? "", top: rr.top, bottom: rr.bottom };
      }),
    });
  }
  return out;
}

export function measureGroups(list: HTMLElement): GroupGeom[] {
  return [...list.querySelectorAll<HTMLElement>(SEL_DROP_GROUP_SECTION)].map((el) => {
    const r = el.getBoundingClientRect();
    return { id: el.dataset["dropGroup"] ?? "", top: r.top, bottom: r.bottom };
  });
}

// ---- the hook ----

export interface DragItem {
  kind: "bookmark" | "group";
  /** Bookmark id or group id. */
  id: string;
  /** Text of the floating label. */
  label: string;
}

export type DropTarget = ({ kind: "bookmark" } & BookmarkDrop) | ({ kind: "group" } & GroupDrop);

export interface DragView {
  item: DragItem;
  x: number;
  y: number;
  drop: DropTarget | null;
  /** Vertical position of the indicator line inside the scrolling list (null = nothing to draw). */
  lineTop: number | null;
}

export interface PointerReorder {
  /** Current drag (null until the threshold is passed). */
  drag: DragView | null;
  /** Wire to `onPointerDown` of a draggable element. */
  start(e: ReactPointerEvent<HTMLElement>, item: DragItem): void;
  /** True right after a drag ended: the click that follows the pointerup must be ignored. */
  consumeClick(): boolean;
}

export function usePointerReorder(options: {
  listRef: RefObject<HTMLElement | null>;
  enabled: boolean;
  resolve(item: DragItem, y: number, list: HTMLElement): DropTarget | null;
  onDrop(item: DragItem, drop: DropTarget): void;
}): PointerReorder {
  const [drag, setDrag] = useState<DragView | null>(null);
  const opts = useRef(options);
  useEffect(() => {
    opts.current = options;
  });
  const clickSuppressed = useRef(false);
  const cleanup = useRef<(() => void) | null>(null);

  useEffect(() => () => cleanup.current?.(), []);

  const start = useCallback((e: ReactPointerEvent<HTMLElement>, item: DragItem) => {
    if (e.button !== 0 || !opts.current.enabled || cleanup.current) return;
    const x0 = e.clientX;
    const y0 = e.clientY;
    let active = false;
    let pointer = { x: x0, y: y0 };
    let timer: ReturnType<typeof setInterval> | null = null;

    const resolveNow = (): DropTarget | null => {
      const list = opts.current.listRef.current;
      return list ? opts.current.resolve(item, pointer.y, list) : null;
    };
    const refresh = () => {
      const drop = resolveNow();
      const list = opts.current.listRef.current;
      const lineTop = drop && list ? drop.lineY - list.getBoundingClientRect().top + list.scrollTop : null;
      setDrag({ item, x: pointer.x, y: pointer.y, drop, lineTop });
    };

    const end = () => {
      window.removeEventListener("pointermove", onMove, true);
      window.removeEventListener("pointerup", onUp, true);
      window.removeEventListener("pointercancel", onCancel, true);
      window.removeEventListener("keydown", onKey, true);
      if (timer !== null) clearInterval(timer);
      cleanup.current = null;
      setDrag(null);
    };
    const swallowClick = () => {
      clickSuppressed.current = true;
      setTimeout(() => {
        clickSuppressed.current = false;
      }, 0);
    };
    function onMove(ev: PointerEvent) {
      pointer = { x: ev.clientX, y: ev.clientY };
      if (!active) {
        if (Math.hypot(pointer.x - x0, pointer.y - y0) < DRAG_THRESHOLD) return;
        active = true;
        timer = setInterval(() => {
          const list = opts.current.listRef.current;
          if (!list) return;
          const r = list.getBoundingClientRect();
          const delta = autoScrollDelta(pointer.y, r.top, r.bottom);
          if (delta === 0) return;
          const before = list.scrollTop;
          list.scrollTop += delta;
          if (list.scrollTop !== before) refresh();
        }, SCROLL_TICK_MS);
      }
      refresh();
    }
    function onUp(ev: PointerEvent) {
      pointer = { x: ev.clientX, y: ev.clientY };
      const drop = active ? resolveNow() : null;
      const wasActive = active;
      end();
      if (wasActive) {
        swallowClick();
        if (drop) opts.current.onDrop(item, drop);
      }
    }
    function onCancel() {
      const wasActive = active;
      end();
      if (wasActive) swallowClick();
    }
    function onKey(ev: KeyboardEvent) {
      if (ev.key !== "Escape" || !active) return;
      ev.preventDefault();
      ev.stopPropagation();
      onCancel();
    }

    window.addEventListener("pointermove", onMove, true);
    window.addEventListener("pointerup", onUp, true);
    window.addEventListener("pointercancel", onCancel, true);
    window.addEventListener("keydown", onKey, true);
    cleanup.current = end;
  }, []);

  const consumeClick = useCallback(() => {
    if (!clickSuppressed.current) return false;
    clickSuppressed.current = false;
    return true;
  }, []);

  return { drag, start, consumeClick };
}
