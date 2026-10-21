// Keeps <html> in sync with `settings.appearance` and `settings.terminal.background` (docs/ARCHITECTURE.md §8.5):
// data-theme / data-accent / data-motion / --ui-scale / --term-bg, "system" follows the OS live.
import type { AppearanceSettings } from "../../ipc/types";
import {
  DEFAULT_APPEARANCE,
  applyAppearanceToElement,
  cacheAppearance,
  readCachedAppearance,
  type AppearanceInputs,
  type AppliedAppearance,
} from "../../lib/theme";
import { useStore } from "../../state/store";

const DARK_QUERY = "(prefers-color-scheme: dark)";
const MOTION_QUERY = "(prefers-reduced-motion: reduce)";

function media(query: string): MediaQueryList | null {
  return typeof window !== "undefined" && typeof window.matchMedia === "function" ? window.matchMedia(query) : null;
}

function systemFlags(): Pick<AppearanceInputs, "systemPrefersDark" | "systemPrefersReducedMotion"> {
  // Without a media query (very old webview, tests) "system" means dark, the app's default.
  return {
    systemPrefersDark: media(DARK_QUERY)?.matches ?? true,
    systemPrefersReducedMotion: media(MOTION_QUERY)?.matches ?? false,
  };
}

/** Whether the OS asks for a dark theme (`true` when it cannot be asked). */
export function systemPrefersDark(): boolean {
  return systemFlags().systemPrefersDark;
}

type Listener =(applied: AppliedAppearance) => void;
const listeners = new Set<Listener>();

/** Called after every application (theme change, system change, terminal background change). Returns the unsubscribe. */
export function subscribeAppearance(listener: Listener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

type Inputs = Pick<AppearanceInputs, "appearance" | "terminalBackground">;

/** The inputs shown on screen right now (saved settings, a dialog preview or the pre-paint cache). */
let shown: Inputs | null = null;
/** Unsaved values of the open settings dialog; while set, settings and OS changes re-apply them, not the saved ones. */
let preview: Inputs | null = null;

function apply(inputs: Inputs): void {
  shown = inputs;
  const applied = applyAppearanceToElement(document.documentElement, { ...inputs, ...systemFlags() });
  for (const listener of listeners) listener(applied);
}

/**
 * Theme, accent and terminal background currently on screen, including an unsaved preview (the terminal theme is
 * built from these so the xterm canvas matches the panel around it). `null` before anything was applied.
 */
export function shownAppearance(): Inputs | null {
  return shown;
}

function savedInputs(): Inputs | null {
  const settings = useStore.getState().settings;
  if (!settings) return null;
  return { appearance: settings.appearance, terminalBackground: settings.terminal.background };
}

/** Applies the saved settings (used on every settings or media-query change). */
export function applySavedAppearance(): void {
  const inputs = savedInputs();
  if (!inputs) return;
  apply(inputs);
  cacheAppearance(inputs);
}

/** A settings or OS change: keeps an open preview on screen (the OS part is re-read), else shows the saved values. */
function refreshAppearance(): void {
  if (preview) apply(preview);
  else applySavedAppearance();
}

/** Applies unsaved values while the settings dialog is open; `endAppearancePreview` reverts them. */
export function previewAppearance(appearance: AppearanceSettings, terminalBackground: string | null): void {
  preview = { appearance, terminalBackground };
  apply(preview);
}

/** Closes the preview (dialog saved or cancelled) and shows the saved settings again. */
export function endAppearancePreview(): void {
  preview = null;
  applySavedAppearance();
}

/** Applies the cached appearance of the last run before React renders (no flash of the wrong theme). */
export function applyCachedAppearance(): void {
  const cached = readCachedAppearance();
  apply(cached ?? { appearance: DEFAULT_APPEARANCE, terminalBackground: null });
}

let installed = false;

/** Subscribes to the store and to the OS media queries. Idempotent. */
export function installAppearance(): void {
  if (installed) return;
  installed = true;
  useStore.subscribe((state, previous) => {
    if (state.settings?.appearance === previous.settings?.appearance && state.settings?.terminal.background === previous.settings?.terminal.background) {
      return;
    }
    refreshAppearance();
  });
  for (const query of [DARK_QUERY, MOTION_QUERY]) media(query)?.addEventListener?.("change", refreshAppearance);
  applySavedAppearance();
}
