// Where a context menu goes. Pure, so it can be tested without layout.

/** Gap kept to the viewport edges. */
export const MENU_EDGE = 6;
/** The Windows Snap Layouts overlay covers this top-right box (two caption buttons wide, the title bar high). */
export const CAPTION_ZONE_WIDTH = 92;
export const CAPTION_ZONE_HEIGHT = 44;

export interface MenuPlacementInput {
  /** Requested position (pointer or anchor). */
  x: number;
  y: number;
  /** Measured menu size. */
  width: number;
  height: number;
  viewportWidth: number;
  viewportHeight: number;
  /** Only Windows has the native overlay over the maximize button. */
  windows: boolean;
}

/**
 * The menu position: inside the viewport and, on Windows, never over the top-right caption zone (the native Snap
 * Layouts overlay would take the mouse there). A menu that would reach into that zone moves below the title bar;
 * a menu too tall for that moves left of the zone instead.
 */
export function placeMenu(input: MenuPlacementInput): { left: number; top: number } {
  const { x, y, width, height, viewportWidth, viewportHeight, windows } = input;
  let left = Math.max(MENU_EDGE, Math.min(x, viewportWidth - width - MENU_EDGE));
  let top = Math.max(MENU_EDGE, Math.min(y, viewportHeight - height - MENU_EDGE));
  const overlapsZone =
    windows &&
    left + width > viewportWidth - CAPTION_ZONE_WIDTH &&
    top < CAPTION_ZONE_HEIGHT;
  if (overlapsZone) {
    const below = CAPTION_ZONE_HEIGHT + MENU_EDGE;
    if (below + height + MENU_EDGE <= viewportHeight) {
      top = Math.max(top, below);
    } else {
      // Does not fit below the zone: keep the height, move left of the zone (never past the left edge).
      left = Math.max(MENU_EDGE, Math.min(left, viewportWidth - CAPTION_ZONE_WIDTH - width - MENU_EDGE));
    }
  }
  return { left, top };
}
