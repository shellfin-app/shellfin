// Size limits of the resizable shell, as pure functions (the panels library does the dragging; these decide the bounds).

export const SIDEBAR_MIN = 200;
/** The sidebar may take up to this share of the window width. */
export const SIDEBAR_MAX_SHARE = 0.8;
/** The main area (editor + terminal) never gets narrower than this. */
export const MAIN_MIN = 320;
/** Width of the activity bar next to the resizable group (`--w-activity`). */
export const ACTIVITY_BAR_PX = 52;
/** The vertical separator between the sidebar and the main area. */
export const SEPARATOR_PX = 1;
/**
 * The narrowest window in which the open sidebar and the main area both keep their minimum. `minWidth` in
 * `src-tauri/tauri.conf.json` must not go below it (`layoutLimits.test.ts`).
 */
export const SHELL_MIN_WIDTH = ACTIVITY_BAR_PX + SIDEBAR_MIN + SEPARATOR_PX + MAIN_MIN;

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

/** Below this window width the title bar shows one menu button instead of the six menu titles (`TitleBar.tsx`). */
export const COMPACT_TITLEBAR_BELOW = 960;
/** Below this window width the status bar drops the transfer rates and the key type (`shell.css`, same number). */
export const COMPACT_STATUSBAR_BELOW = 760;

/** Whether a window of `windowWidth` px uses the compact title bar. */
export function isCompactTitleBar(windowWidth: number): boolean {
  return windowWidth < COMPACT_TITLEBAR_BELOW;
}
