// Local, per-machine layout choices (sidebar width, Bookmarks/Files split, view, status bar). They are UI state, not
// settings: no contract field exists for them, and losing them only resets the layout. Storage may be missing or
// throw (private window, blocked site data), so every access is guarded and the app works without it.
export type SidebarView = "bookmarks" | "files" | "both";

export interface LayoutPrefs {
  activity: SidebarView;
  sidebarOpen: boolean;
  statusBarOpen: boolean;
  /** Sidebar width in px (up to 80% of the window; the shell clamps it to the current window). */
  sidebarWidth: number;
  /** Share of the sidebar height taken by the Bookmarks pane when both panes are shown (percent). */
  bookmarksShare: number;
  bookmarksCollapsed: boolean;
  filesCollapsed: boolean;
  /** Terminal panel height in px; null = the value from settings. */
  terminalHeight: number | null;
}

export const LAYOUT_DEFAULTS: LayoutPrefs = {
  activity: "both",
  sidebarOpen: true,
  statusBarOpen: true,
  sidebarWidth: 280,
  bookmarksShare: 40,
  bookmarksCollapsed: false,
  filesCollapsed: false,
  terminalHeight: null,
};

const STORAGE_KEY = "shellfin.layout.v1";

function num(value: unknown, min: number, max: number, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? Math.min(max, Math.max(min, value)) : fallback;
}

function bool(value: unknown, fallback: boolean): boolean {
  return typeof value === "boolean" ? value : fallback;
}

/** Patches not yet written to storage (resize ticks arrive many times a second; writes are debounced). */
let pending: Partial<LayoutPrefs> | null = null;
let timer: ReturnType<typeof setTimeout> | null = null;
const SAVE_DELAY_MS = 250;

function readStored(): LayoutPrefs {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (raw === null) return { ...LAYOUT_DEFAULTS };
    const data = JSON.parse(raw) as Record<string, unknown>;
    const activity = data["activity"];
    return {
      activity: activity === "bookmarks" || activity === "files" || activity === "both" ? activity : LAYOUT_DEFAULTS.activity,
      sidebarOpen: bool(data["sidebarOpen"], LAYOUT_DEFAULTS.sidebarOpen),
      statusBarOpen: bool(data["statusBarOpen"], LAYOUT_DEFAULTS.statusBarOpen),
      sidebarWidth: num(data["sidebarWidth"], 160, 10000, LAYOUT_DEFAULTS.sidebarWidth),
      bookmarksShare: num(data["bookmarksShare"], 5, 95, LAYOUT_DEFAULTS.bookmarksShare),
      bookmarksCollapsed: bool(data["bookmarksCollapsed"], LAYOUT_DEFAULTS.bookmarksCollapsed),
      filesCollapsed: bool(data["filesCollapsed"], LAYOUT_DEFAULTS.filesCollapsed),
      terminalHeight: data["terminalHeight"] === null ? null : num(data["terminalHeight"], 80, 10000, 0) || null,
    };
  } catch {
    return { ...LAYOUT_DEFAULTS };
  }
}

/** The stored layout including patches still waiting for their debounced write (so a re-mount sees the last size). */
export function loadLayoutPrefs(): LayoutPrefs {
  return { ...readStored(), ...pending };
}

/** Writes pending patches now (also on page hide). */
export function flushLayoutPrefs(): void {
  if (timer !== null) {
    clearTimeout(timer);
    timer = null;
  }
  if (pending === null) return;
  const merged = { ...readStored(), ...pending };
  pending = null;
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(merged));
  } catch {
    // Storage unavailable: the layout simply is not remembered.
  }
}

/** Forgets every stored layout choice (and patches still waiting for their write): the defaults apply again. */
export function resetLayoutPrefs(): void {
  if (timer !== null) {
    clearTimeout(timer);
    timer = null;
  }
  pending = null;
  try {
    window.localStorage.removeItem(STORAGE_KEY);
  } catch {
    // Storage unavailable: nothing was remembered anyway.
  }
}

/** Remembers layout choices. `debounce` is for high-frequency sources (dragging a separator). */
export function saveLayoutPrefs(patch: Partial<LayoutPrefs>, options: { debounce?: boolean } = {}): void {
  pending = { ...pending, ...patch };
  if (options.debounce === true) {
    timer ??= setTimeout(flushLayoutPrefs, SAVE_DELAY_MS);
    return;
  }
  flushLayoutPrefs();
}

if (typeof window !== "undefined") window.addEventListener("pagehide", flushLayoutPrefs);

/** One-time flags ("the hint was shown") share the same guarded storage. */
const FLAG_PREFIX = "shellfin.flag.";

export function readFlag(name: string): boolean {
  try {
    return window.localStorage.getItem(FLAG_PREFIX + name) === "1";
  } catch {
    return false;
  }
}

export function writeFlag(name: string): void {
  try {
    window.localStorage.setItem(FLAG_PREFIX + name, "1");
  } catch {
    // Not remembered; the hint may show once more in the next run.
  }
}
