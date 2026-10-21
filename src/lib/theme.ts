// Theme, accent, UI font size and motion: pure helpers plus the DOM application (docs/ARCHITECTURE.md §8.5).
// The wiring to the store and to the OS media queries is in features/settings/appearance.ts.
import type { AccentColor, AppearanceSettings, ReduceMotion, ThemeMode } from "../ipc/types";

export type ResolvedTheme = "dark" | "light";

export const THEME_MODES: readonly ThemeMode[] = ["dark", "light", "system"];
export const ACCENTS: readonly AccentColor[] = ["teal", "blue", "purple", "orange", "green", "pink"];
export const REDUCE_MOTION_MODES: readonly ReduceMotion[] = ["system", "on", "off"];

export const UI_FONT_SIZE = { min: 12, max: 16, default: 13 } as const;

/**
 * Accent colours per theme. These mirror `styles/tokens.css` (a unit test compares both); canvas renderers and the
 * swatches cannot read CSS variables. Light values reach 4.5:1 on white, the panel and the chrome colour.
 */
export const ACCENT_VALUES: Record<AccentColor, { dark: string; light: string }> = {
  teal: { dark: "#4FD1C5", light: "#0B7A70" },
  blue: { dark: "#8AB4F8", light: "#1F5FBF" },
  purple: { dark: "#C792EA", light: "#7E3FB5" },
  orange: { dark: "#F7A072", light: "#A94A0B" },
  green: { dark: "#4ADE80", light: "#13763A" },
  pink: { dark: "#F29BC4", light: "#BE185D" },
};

export const DEFAULT_APPEARANCE: AppearanceSettings = { theme: "dark", accent: "teal", uiFontSize: 13, reduceMotion: "system" };

/** `system` follows the OS; anything else is itself. */
export function resolveTheme(mode: ThemeMode, systemPrefersDark: boolean): ResolvedTheme {
  if (mode === "system") return systemPrefersDark ? "dark" : "light";
  return mode;
}

/** `on` always reduces, `off` never, `system` follows `prefers-reduced-motion`. */
export function resolveReduceMotion(mode: ReduceMotion, systemPrefersReduced: boolean): boolean {
  if (mode === "system") return systemPrefersReduced;
  return mode === "on";
}

export function clampUiFontSize(value: number): number {
  if (!Number.isFinite(value)) return UI_FONT_SIZE.default;
  return Math.min(UI_FONT_SIZE.max, Math.max(UI_FONT_SIZE.min, Math.round(value)));
}

/** Strict `#RRGGBB` (exactly 7 characters, hex digits in either case), like the backend. */
export function isHexColor(value: string): boolean {
  return /^#[0-9A-Fa-f]{6}$/.test(value);
}

/** WCAG relative luminance (0 black .. 1 white) of a `#RRGGBB` colour. */
export function relativeLuminance(hex: string): number {
  const channel = (i: number) => {
    const v = parseInt(hex.slice(i, i + 2), 16) / 255;
    return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5);
}

/** WCAG contrast ratio (1..21) of two `#RRGGBB` colours. */
export function contrastRatio(a: string, b: string): number {
  const la = relativeLuminance(a);
  const lb = relativeLuminance(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

/**
 * Above this relative luminance a terminal background counts as light and gets the light (dark text) palette. 0.16 is
 * where the light palette's foreground (#1F2329) and the dark palette's (#C9CED8) have equal contrast, so mid greys
 * get whichever text reads better.
 */
export const LIGHT_BACKGROUND_LUMINANCE = 0.16;

export function toneOfBackground(hex: string): ResolvedTheme {
  return relativeLuminance(hex) > LIGHT_BACKGROUND_LUMINANCE ? "light" : "dark";
}

export interface AppearanceInputs {
  appearance: AppearanceSettings;
  /** `terminal.background`: `null` follows the theme. */
  terminalBackground: string | null;
  systemPrefersDark: boolean;
  systemPrefersReducedMotion: boolean;
}

export interface AppliedAppearance {
  theme: ResolvedTheme;
  reduceMotion: boolean;
}

/** Writes the appearance onto `<html>`: `data-theme`, `data-accent`, `data-motion`, `--ui-scale`, `--term-bg`. */
export function applyAppearanceToElement(root: HTMLElement, inputs: AppearanceInputs): AppliedAppearance {
  const { appearance, terminalBackground } = inputs;
  const theme = resolveTheme(appearance.theme, inputs.systemPrefersDark);
  const reduceMotion = resolveReduceMotion(appearance.reduceMotion, inputs.systemPrefersReducedMotion);
  root.dataset.theme = theme;
  root.dataset.accent = appearance.accent;
  root.dataset.motion = reduceMotion ? "reduce" : "full";
  root.style.setProperty("--ui-scale", String(clampUiFontSize(appearance.uiFontSize) / UI_FONT_SIZE.default));
  if (terminalBackground !== null && isHexColor(terminalBackground)) {
    root.style.setProperty("--term-bg", terminalBackground);
    root.dataset.termTone = toneOfBackground(terminalBackground);
  } else {
    root.style.removeProperty("--term-bg");
    delete root.dataset.termTone;
  }
  return { theme, reduceMotion };
}

// ---- pre-paint cache ----
// index.html may not carry inline scripts (CSP), so the last applied appearance is cached in localStorage and applied
// synchronously when the entry module starts, before React renders and before the settings arrive from the backend.

const CACHE_KEY = "shellfin.appearance";

export function cacheAppearance(inputs: Pick<AppearanceInputs, "appearance" | "terminalBackground">): void {
  try {
    localStorage.setItem(CACHE_KEY, JSON.stringify(inputs));
  } catch {
    // Storage may be unavailable (private window, blocked); the next start just paints dark first.
  }
}

export function readCachedAppearance(): Pick<AppearanceInputs, "appearance" | "terminalBackground"> | null {
  try {
    const raw = localStorage.getItem(CACHE_KEY);
    if (raw === null) return null;
    const parsed = JSON.parse(raw) as Partial<{ appearance: Partial<AppearanceSettings>; terminalBackground: unknown }>;
    const a = parsed.appearance ?? {};
    const appearance: AppearanceSettings = {
      theme: THEME_MODES.includes(a.theme as ThemeMode) ? (a.theme as ThemeMode) : DEFAULT_APPEARANCE.theme,
      accent: ACCENTS.includes(a.accent as AccentColor) ? (a.accent as AccentColor) : DEFAULT_APPEARANCE.accent,
      uiFontSize: clampUiFontSize(Number(a.uiFontSize ?? DEFAULT_APPEARANCE.uiFontSize)),
      reduceMotion: REDUCE_MOTION_MODES.includes(a.reduceMotion as ReduceMotion)
        ? (a.reduceMotion as ReduceMotion)
        : DEFAULT_APPEARANCE.reduceMotion,
    };
    const bg = typeof parsed.terminalBackground === "string" && isHexColor(parsed.terminalBackground) ? parsed.terminalBackground : null;
    return { appearance, terminalBackground: bg };
  } catch {
    return null;
  }
}

/** The theme currently shown (`dark` before anything was applied). */
export function currentResolvedTheme(): ResolvedTheme {
  return typeof document !== "undefined" && document.documentElement.dataset.theme === "light" ? "light" : "dark";
}
