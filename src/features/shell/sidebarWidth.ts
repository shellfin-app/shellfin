// Which sidebar widths are remembered, and which width is shown. Pure, so it can be tested without layout.
import { clampSidebarWidth } from "./layoutLimits";

/**
 * The width the sidebar must have in a window of `windowWidth` px when it now has `currentPx`: the user's width
 * clamped to the window, or `null` when it already has it. The panel library keeps percentages across a group resize,
 * so after a one-step widen (maximize, Snap) it shows a share of the old window instead; the shell resizes it to this.
 */
export function sidebarCorrection(currentPx: number, userWidth: number, windowWidth: number): number | null {
  const want = clampSidebarWidth(userWidth, windowWidth);
  return Number.isFinite(currentPx) && Math.abs(currentPx - want) < 1 ? null : want;
}

/** A resize this soon after a key press on the separator counts as a keyboard resize (ms). */
export const KEY_RESIZE_WINDOW_MS = 500;

export interface SidebarWidthTracker {
  /** The width the user chose last (stored or dragged); the shell clamps it to the window when it renders. */
  userWidth(): number;
  /** A pointer press on the sidebar separator (drag or double click). */
  pointerDown(): void;
  /** The pointer was released or cancelled anywhere. */
  pointerUp(): void;
  /** A key press on the focused sidebar separator (arrow keys, Home/End resize it). */
  key(): void;
  /**
   * A size reported by the panel. Returns the width to store when the user is resizing, `null` when the change
   * comes from the layout (window narrower than the stored width, restore after a widening): those are not the user's
   * choice and must not overwrite it.
   */
  onResize(px: number): number | null;
}

/**
 * A drag still counts this long after the pointer is released (ms): react-resizable-panels reports the final size
 * after `pointerup` when the release comes in the same frame as the last move. A few frames, far below any human
 * follow-up action.
 */
export const DRAG_RELEASE_GRACE_MS = 100;

export function createSidebarWidthTracker(stored: number, now: () => number = () => performance.now()): SidebarWidthTracker {
  let user = stored;
  let dragging = false;
  let releasedAt = Number.NEGATIVE_INFINITY;
  let keyAt = Number.NEGATIVE_INFINITY;
  return {
    userWidth: () => user,
    pointerDown: () => {
      dragging = true;
    },
    pointerUp: () => {
      if (dragging) releasedAt = now();
      dragging = false;
    },
    key: () => {
      keyAt = now();
    },
    onResize: (px) => {
      if (!Number.isFinite(px) || px <= 0) return null;
      const t = now();
      if (!dragging && t - releasedAt > DRAG_RELEASE_GRACE_MS && t - keyAt > KEY_RESIZE_WINDOW_MS) return null;
      user = Math.round(px);
      return user;
    },
  };
}
