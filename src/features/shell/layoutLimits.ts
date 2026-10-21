// Size limits of the resizable shell, as pure functions (the panels library does the dragging; these decide the bounds).

export const SIDEBAR_MIN = 200;
/** The sidebar may take up to this share of the window width. */
export const SIDEBAR_MAX_SHARE = 0.8;
/** The main area (editor + terminal) never gets narrower than this. */
export const MAIN_MIN = 320;
/** Width of the activity bar next to the resizable group (`--w-activity`), approximately. */
export const ACTIVITY_BAR_PX = 48;

/** Largest sidebar width for a window of `windowWidth` px: 80% of it, but the main area keeps `MAIN_MIN`. */
export function sidebarMaxPx(windowWidth: number): number {
  const group = Math.max(0, windowWidth - ACTIVITY_BAR_PX);
  const byShare = Math.floor(windowWidth * SIDEBAR_MAX_SHARE);
  const byMain = group - MAIN_MIN;
  return Math.max(SIDEBAR_MIN, Math.min(byShare, byMain));
}

/** A stored or dragged sidebar width brought into [SIDEBAR_MIN, sidebarMaxPx(windowWidth)]. */
export function clampSidebarWidth(width: number, windowWidth: number): number {
  if (!Number.isFinite(width)) return SIDEBAR_MIN;
  return Math.round(Math.min(sidebarMaxPx(windowWidth), Math.max(SIDEBAR_MIN, width)));
}
