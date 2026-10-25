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

/**
 * Below this window width the title bar always shows one menu button instead of the six menu titles (`TitleBar.tsx`).
 * At or above it the titles stay unless they do not fit (`isCompactTitleBar` with a measurement): the lower bound of
 * the measured switch, so English keeps the behaviour of #14.
 */
export const COMPACT_TITLEBAR_BELOW = 960;
/** Below this window width the status bar drops the transfer rates and the key type (`shell.css`, same number). */
export const COMPACT_STATUSBAR_BELOW = 760;
/** Below this window width the palette uses its short placeholder (`CommandPalette.tsx`). */
export const SHORT_PALETTE_PLACEHOLDER_BELOW = 960;
/** What the title bar keeps for the connection tab strip (`.conn-tabs-wrap` min-width in `shell.css`). */
export const TITLEBAR_TABS_MIN = 140;
/** What the title bar keeps for the empty drag area (`.title-bar__drag` min-width in `shell.css`). */
export const TITLEBAR_DRAG_MIN = 24;

/** Measured widths of the title bar (CSS px), see `useCompactTitleBar`. */
export interface TitleBarFit {
  /** Inner width of `.title-bar__content` (everything left of the window controls). */
  contentWidth: number;
  /** Natural width of the six menu titles in the current language and UI scale. */
  menuWidth: number;
  /** Width of the other fixed parts in the row: logo, search button, the gaps between the items. */
  othersWidth: number;
}

/**
 * Whether a window of `windowWidth` px uses the compact title bar: always below `COMPACT_TITLEBAR_BELOW`, and above
 * it when the measured menu titles plus the fixed parts and the minimum tab strip and drag area do not fit. Without
 * a usable measurement (unknown yet, or no layout as in jsdom) only the width bound decides.
 */
export function isCompactTitleBar(windowWidth: number, fit?: TitleBarFit | null): boolean {
  if (windowWidth < COMPACT_TITLEBAR_BELOW) return true;
  if (!fit || !(fit.contentWidth > 0) || !(fit.menuWidth > 0) || !Number.isFinite(fit.othersWidth)) return false;
  return fit.menuWidth + Math.max(0, fit.othersWidth) + TITLEBAR_TABS_MIN + TITLEBAR_DRAG_MIN > fit.contentWidth;
}
