import { beforeEach, describe, expect, it } from "vitest";
import {
  DEFAULT_APPEARANCE,
  applyAppearanceToElement,
  cacheAppearance,
  clampUiFontSize,
  contrastRatio,
  isHexColor,
  readCachedAppearance,
  relativeLuminance,
  resolveReduceMotion,
  resolveTheme,
  toneOfBackground,
} from "./theme";
import { appearanceInvalidFields, parseHexColor, type AppearanceForm } from "./settingsFormAppearance";

describe("resolveTheme / resolveReduceMotion", () => {
  it("dark and light ignore the OS; system follows it", () => {
    expect(resolveTheme("dark", false)).toBe("dark");
    expect(resolveTheme("light", true)).toBe("light");
    expect(resolveTheme("system", true)).toBe("dark");
    expect(resolveTheme("system", false)).toBe("light");
  });

  it("reduce motion: on / off are absolute, system follows the OS", () => {
    expect(resolveReduceMotion("on", false)).toBe(true);
    expect(resolveReduceMotion("off", true)).toBe(false);
    expect(resolveReduceMotion("system", true)).toBe(true);
    expect(resolveReduceMotion("system", false)).toBe(false);
  });
});

describe("applyAppearanceToElement", () => {
  let root: HTMLElement;
  beforeEach(() => {
    root = document.createElement("html");
  });
  const inputs = { appearance: DEFAULT_APPEARANCE, terminalBackground: null, systemPrefersDark: true, systemPrefersReducedMotion: false };

  it("sets theme, accent, motion and the UI scale", () => {
    const applied = applyAppearanceToElement(root, {
      ...inputs,
      appearance: { theme: "light", accent: "purple", uiFontSize: 16, reduceMotion: "on" },
    });
    expect(applied).toEqual({ theme: "light", reduceMotion: true });
    expect(root.dataset.theme).toBe("light");
    expect(root.dataset.accent).toBe("purple");
    expect(root.dataset.motion).toBe("reduce");
    expect(Number(root.style.getPropertyValue("--ui-scale"))).toBeCloseTo(16 / 13);
  });

  it("system follows the OS preference and the default scale is 1", () => {
    applyAppearanceToElement(root, { ...inputs, appearance: { ...DEFAULT_APPEARANCE, theme: "system" }, systemPrefersDark: false });
    expect(root.dataset.theme).toBe("light");
    applyAppearanceToElement(root, { ...inputs, appearance: { ...DEFAULT_APPEARANCE, theme: "system" }, systemPrefersDark: true });
    expect(root.dataset.theme).toBe("dark");
    expect(root.style.getPropertyValue("--ui-scale")).toBe("1");
    expect(root.dataset.motion).toBe("full");
  });

  it("system reduce-motion follows prefers-reduced-motion", () => {
    applyAppearanceToElement(root, { ...inputs, systemPrefersReducedMotion: true });
    expect(root.dataset.motion).toBe("reduce");
  });

  it("a custom terminal background sets --term-bg and the tone; null removes them", () => {
    applyAppearanceToElement(root, { ...inputs, terminalBackground: "#FBF9F4" });
    expect(root.style.getPropertyValue("--term-bg")).toBe("#FBF9F4");
    expect(root.dataset.termTone).toBe("light");
    applyAppearanceToElement(root, { ...inputs, terminalBackground: "#000000" });
    expect(root.dataset.termTone).toBe("dark");
    applyAppearanceToElement(root, inputs);
    expect(root.style.getPropertyValue("--term-bg")).toBe("");
    expect(root.dataset.termTone).toBeUndefined();
  });

  it("an invalid background string is ignored", () => {
    applyAppearanceToElement(root, { ...inputs, terminalBackground: "red; background: url(x)" });
    expect(root.style.getPropertyValue("--term-bg")).toBe("");
  });
});

describe("colour helpers", () => {
  it("clamps the UI font size to 12..16", () => {
    expect(clampUiFontSize(5)).toBe(12);
    expect(clampUiFontSize(99)).toBe(16);
    expect(clampUiFontSize(14.4)).toBe(14);
    expect(clampUiFontSize(Number.NaN)).toBe(13);
  });

  it("validates exactly #RRGGBB", () => {
    expect(isHexColor("#0F1115")).toBe(true);
    expect(isHexColor("#abcdef")).toBe(true);
    for (const bad of ["0F1115", "#0F111", "#0F11155", "#GGGGGG", "#FFF", "", " #0F1115", "#0F1115 "]) expect(isHexColor(bad)).toBe(false);
  });

  it("luminance and contrast follow WCAG", () => {
    expect(relativeLuminance("#000000")).toBe(0);
    expect(relativeLuminance("#FFFFFF")).toBeCloseTo(1);
    expect(contrastRatio("#000000", "#FFFFFF")).toBeCloseTo(21);
    expect(contrastRatio("#FFFFFF", "#FFFFFF")).toBeCloseTo(1);
  });

  it("the light/dark switch is at relative luminance 0.16", () => {
    expect(toneOfBackground("#FFFFFF")).toBe("light");
    expect(toneOfBackground("#FBF9F4")).toBe("light");
    expect(toneOfBackground("#0F1115")).toBe("dark");
    expect(toneOfBackground("#14171D")).toBe("dark");
    // #808080 has luminance ~0.216 (light palette); #5A5A5A ~0.102 (dark palette).
    expect(toneOfBackground("#808080")).toBe("light");
    expect(toneOfBackground("#5A5A5A")).toBe("dark");
  });

  it("mid-grey backgrounds get the palette whose text reads better", () => {
    for (const bg of ["#5A5A5A", "#6E6E6E", "#808080", "#949494", "#B0B0B0"]) {
      const light = contrastRatio(bg, "#1F2329");
      const dark = contrastRatio(bg, "#C9CED8");
      expect(toneOfBackground(bg)).toBe(light >= dark ? "light" : "dark");
    }
  });
});

describe("pre-paint cache", () => {
  beforeEach(() => localStorage.clear());

  it("round-trips the appearance", () => {
    cacheAppearance({ appearance: { theme: "light", accent: "pink", uiFontSize: 15, reduceMotion: "off" }, terminalBackground: "#FFFFFF" });
    expect(readCachedAppearance()).toEqual({
      appearance: { theme: "light", accent: "pink", uiFontSize: 15, reduceMotion: "off" },
      terminalBackground: "#FFFFFF",
    });
  });

  it("falls back to defaults for garbage", () => {
    localStorage.setItem("shellfin.appearance", JSON.stringify({ appearance: { theme: "neon", accent: 3, uiFontSize: "x" }, terminalBackground: "url(x)" }));
    expect(readCachedAppearance()).toEqual({ appearance: DEFAULT_APPEARANCE, terminalBackground: null });
    localStorage.setItem("shellfin.appearance", "{nope");
    expect(readCachedAppearance()).toBeNull();
  });
});

describe("appearance form", () => {
  const form = (over: Partial<AppearanceForm>): AppearanceForm => ({
    appearance: DEFAULT_APPEARANCE,
    terminalBackground: null,
    terminalBackgroundText: "",
    ...over,
  });

  it("parseHexColor accepts #RRGGBB (with or without #) and upper-cases", () => {
    expect(parseHexColor("#1a1b26")).toBe("#1A1B26");
    expect(parseHexColor(" 1a1b26 ")).toBe("#1A1B26");
    expect(parseHexColor("#1a1b2")).toBeNull();
    expect(parseHexColor("#1a1b26ff")).toBeNull();
  });

  it("flags typed text that is not a colour", () => {
    expect(appearanceInvalidFields(form({}))).toEqual([]);
    expect(appearanceInvalidFields(form({ terminalBackgroundText: "#12" }))).toEqual(["terminal.background"]);
    expect(appearanceInvalidFields(form({ terminalBackground: "#112233", terminalBackgroundText: "#112233" }))).toEqual([]);
  });
});
