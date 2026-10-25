// WCAG contrast of the light theme and of the accents, computed from the real src/styles/tokens.css.
// Text pairs need 4.5:1; icons, rings and focus indicators 3:1 (light theme tokens).
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { ACCENTS, ACCENT_VALUES, contrastRatio } from "../lib/theme";
import { cmTheme } from "./cmTheme";

// Read from disk: the test config switches CSS processing off, which empties `?raw` CSS imports too.
const tokensCss = readFileSync("src/styles/tokens.css", "utf8").replace(/\r\n/g, "\n");

/** `--name: value` pairs of the first (or, with `last`, the last) rule whose selector text contains `selector`. */
function block(selector: string, last = false): Record<string, string> {
  const start = last ? tokensCss.lastIndexOf(selector) : tokensCss.indexOf(selector);
  if (start < 0) throw new Error(`selector not found: ${selector}`);
  const open = tokensCss.indexOf("{", start);
  const close = tokensCss.indexOf("}", open);
  const vars: Record<string, string> = {};
  for (const m of tokensCss.slice(open + 1, close).matchAll(/(--[a-z0-9-]+)\s*:\s*([^;]+);/g)) {
    vars[m[1] ?? ""] = (m[2] ?? "").trim();
  }
  return vars;
}

const dark = block(":root[data-term-tone=\"dark\"] .terminal-panel");
const light = { ...block(":root[data-term-tone=\"light\"] .terminal-panel"), ...block(":root[data-theme=\"light\"] {") };

/** A colour value as #RRGGBB, blending `rgba(r, g, b, a)` over `over`. */
function solid(value: string, over: string): string {
  const rgba = /^rgba\(\s*(\d+),\s*(\d+),\s*(\d+),\s*([\d.]+)\)$/.exec(value);
  if (!rgba) return value.toUpperCase();
  const alpha = Number(rgba[4]);
  const base = [1, 3, 5].map((i) => parseInt(over.slice(i, i + 2), 16));
  const mixed = [1, 2, 3].map((i, k) => Math.round(Number(rgba[i]) * alpha + (base[k] ?? 0) * (1 - alpha)));
  return `#${mixed.map((v) => v.toString(16).padStart(2, "0")).join("").toUpperCase()}`;
}

const L = (name: string, over = light["--bg"] ?? "#FFFFFF"): string => {
  const v = light[name];
  if (v === undefined) throw new Error(`missing light token ${name}`);
  return solid(v, over);
};

/** The dark set plus the root-only tokens (default teal accent). */
const rootDark = { ...dark, ...block(":root {") };
const D = (name: string, over = dark["--bg"] ?? "#14161B"): string => {
  const v = rootDark[name];
  if (v === undefined) throw new Error(`missing dark token ${name}`);
  return solid(v, over);
};

describe("light theme tokens: text pairs reach 4.5:1", () => {
  const surfaces = ["--bg", "--panel", "--chrome"] as const;
  const onSurfaces = ["--text", "--text-bright", "--text-muted", "--text-secondary", "--link", "--ok", "--danger", "--code-fg", "--accent"];
  for (const surface of surfaces) {
    for (const fg of onSurfaces) {
      it(`${fg} on ${surface}`, () => {
        expect(contrastRatio(L(fg), L(surface))).toBeGreaterThanOrEqual(4.5);
      });
    }
  }

  it("--warn on the surfaces and on its tint", () => {
    for (const bg of [L("--bg"), L("--panel"), L("--warn-bg")]) expect(contrastRatio(L("--warn"), bg)).toBeGreaterThanOrEqual(4.5);
  });

  it("text on the tinted notice backgrounds", () => {
    for (const bg of ["--warn-bg", "--danger-bg", "--accent-bg"]) expect(contrastRatio(L("--text-bright"), L(bg))).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(L("--danger"), L("--danger-bg"))).toBeGreaterThanOrEqual(4.5);
  });

  it("gutter, comments, hover and link hover", () => {
    expect(contrastRatio(L("--text-gutter"), L("--bg"))).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(L("--link-hover"), L("--bg"))).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(L("--text"), L("--active-line"))).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(L("--text-muted"), L("--hover"))).toBeGreaterThanOrEqual(4.5);
  });

  it("bookmark row text over the state layers", () => {
    const panel = L("--panel");
    const hover = solid(light["--layer-hover"] ?? "", panel);
    const blur = solid(light["--layer-sel-blur"] ?? "", panel);
    const focus = solid(light["--layer-sel-focus"] ?? "", panel);
    expect(contrastRatio(L("--row-addr-hover"), hover)).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(L("--row-addr-selected"), blur)).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(L("--row-addr-selected"), focus)).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(L("--text-bright"), focus)).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(L("--text-muted"), hover)).toBeGreaterThanOrEqual(4.5);
  });

  it("the approved brief values are in place", () => {
    expect(light["--bg"]).toBe("#FFFFFF");
    expect(light["--panel"]).toBe("#F6F7F9");
    expect(light["--chrome"]).toBe("#EEF0F3");
    expect(light["--hover"]).toBe("#ECEEF1");
    expect(light["--selected"]).toBe("#DDDEE0");
    expect(light["--layer-hover"]).toBe("rgba(0, 0, 0, .04)");
    expect(light["--layer-sel-blur"]).toBe("rgba(0, 0, 0, .06)");
    expect(light["--layer-sel-focus"]).toBe("rgba(0, 0, 0, .10)");
  });
});

describe("light theme tokens: icons, rings and focus indicators reach 3:1", () => {
  it("dim icons, the hollow status ring and the folder icon", () => {
    for (const token of ["--text-dim", "--status-ring", "--folder"]) {
      for (const surface of ["--bg", "--panel"]) expect(contrastRatio(L(token), L(surface)), `${token} on ${surface}`).toBeGreaterThanOrEqual(3);
    }
  });

  it("the focus ring (accent) on every surface", () => {
    for (const surface of ["--bg", "--panel", "--chrome"]) expect(contrastRatio(L("--accent"), L(surface))).toBeGreaterThanOrEqual(3);
  });

  it("bookmark colours on the surfaces", () => {
    for (const name of ["red", "orange", "yellow", "green", "teal", "blue", "purple", "gray"]) {
      expect(contrastRatio(L(`--bm-${name}`), L("--bg")), `--bm-${name}`).toBeGreaterThanOrEqual(3);
    }
  });
});

describe("accents", () => {
  it("TypeScript values mirror tokens.css (teal is the default rule)", () => {
    for (const accent of ACCENTS) {
      const darkRule = accent === "teal" ? dark : block(`:root[data-accent="${accent}"]`);
      const lightRule = accent === "teal" ? block(":root[data-theme=\"light\"] {") : block(`:root[data-theme="light"][data-accent="${accent}"]`);
      expect(darkRule["--accent"]?.toUpperCase() ?? tokensCss.match(/--accent:\s*(#[0-9A-F]{6})/i)?.[1]?.toUpperCase(), `${accent} dark`).toBe(
        ACCENT_VALUES[accent].dark,
      );
      expect(lightRule["--accent"]?.toUpperCase(), `${accent} light`).toBe(ACCENT_VALUES[accent].light);
    }
  });

  it("light accents reach 4.5:1 on white, the panel and the chrome colour, and carry white text at 4.5:1", () => {
    for (const accent of ACCENTS) {
      const value = ACCENT_VALUES[accent].light;
      for (const surface of ["#FFFFFF", L("--panel"), L("--chrome")]) {
        expect(contrastRatio(value, surface), `${accent} on ${surface}`).toBeGreaterThanOrEqual(4.5);
      }
      expect(contrastRatio("#FFFFFF", value), `${accent} with white text`).toBeGreaterThanOrEqual(4.5);
    }
  });

  it("dark accents stay readable on the dark surfaces and carry the dark foreground", () => {
    for (const accent of ACCENTS) {
      const value = ACCENT_VALUES[accent].dark;
      for (const surface of [D("--bg"), D("--panel"), D("--chrome")]) expect(contrastRatio(value, surface)).toBeGreaterThanOrEqual(4.5);
      expect(contrastRatio(rootDark["--accent-fg"] ?? "", value)).toBeGreaterThanOrEqual(4.5);
    }
  });

  it("each accent is readable on its own tint (active toggles, selected chips), in both themes", () => {
    for (const accent of ACCENTS) {
      const darkRule = accent === "teal" ? rootDark : block(`:root[data-accent="${accent}"]`);
      const lightRule = accent === "teal" ? block(":root[data-theme=\"light\"] {") : block(`:root[data-theme="light"][data-accent="${accent}"]`);
      expect(contrastRatio(ACCENT_VALUES[accent].dark, darkRule["--accent-bg"] ?? ""), `${accent} dark`).toBeGreaterThanOrEqual(4.5);
      expect(contrastRatio(ACCENT_VALUES[accent].light, lightRule["--accent-bg"] ?? ""), `${accent} light`).toBeGreaterThanOrEqual(4.5);
      expect(contrastRatio(L("--text-bright"), lightRule["--accent-bg"] ?? ""), `text on ${accent} light tint`).toBeGreaterThanOrEqual(4.5);
      expect(contrastRatio(D("--text-bright"), darkRule["--accent-bg"] ?? ""), `text on ${accent} dark tint`).toBeGreaterThanOrEqual(4.5);
    }
  });

  it("light accents carry the light accent foreground from tokens.css", () => {
    const fg = block(":root[data-theme=\"light\"] {")["--accent-fg"] ?? "";
    for (const accent of ACCENTS) expect(contrastRatio(fg, ACCENT_VALUES[accent].light), accent).toBeGreaterThanOrEqual(4.5);
  });
});

/** Terminal background swatches of the settings (brief): dark ones get the dark tone set, light ones the light set. */
const DARK_TERMINAL_BACKGROUNDS = ["#0F1115", "#000000", "#14171D"];
const LIGHT_TERMINAL_BACKGROUNDS = ["#FFFFFF", "#FBF9F4"];

describe("terminal panel on a custom background: the accent follows the background's tone", () => {
  for (const tone of ["dark", "light"] as const) {
    it(`${tone} tone: every accent reaches 3:1 on the backgrounds and 4.5:1 on its tint`, () => {
      const backgrounds = tone === "dark" ? DARK_TERMINAL_BACKGROUNDS : LIGHT_TERMINAL_BACKGROUNDS;
      // The last rule with this exact selector is the accent rule (the first one is the tone's colour set).
      const base = block(`:root[data-term-tone="${tone}"] .terminal-panel {`, true);
      for (const accent of ACCENTS) {
        const rule = accent === "teal" ? base : { ...base, ...block(`:root[data-term-tone="${tone}"][data-accent="${accent}"] .terminal-panel`) };
        const value = rule["--accent"] ?? "";
        expect(value.toUpperCase(), `${tone} ${accent} mirrors the theme value`).toBe(ACCENT_VALUES[accent][tone]);
        for (const bg of backgrounds) expect(contrastRatio(value, bg), `${tone} ${accent} on ${bg}`).toBeGreaterThanOrEqual(3);
        expect(contrastRatio(value, rule["--accent-bg"] ?? ""), `${tone} ${accent} on its tint`).toBeGreaterThanOrEqual(4.5);
      }
    });
  }
});

describe("dark theme tokens: text pairs reach 4.5:1", () => {
  const onSurfaces = ["--text", "--text-bright", "--text-muted", "--text-secondary", "--link", "--ok", "--warn", "--danger", "--code-fg", "--accent"];
  for (const surface of ["--bg", "--panel", "--chrome"] as const) {
    it(`text tokens on ${surface}`, () => {
      for (const fg of onSurfaces) expect(contrastRatio(D(fg), D(surface)), `${fg} on ${surface}`).toBeGreaterThanOrEqual(4.5);
    });
  }

  it("syntax colours and the gutter on the editor background", () => {
    for (const token of ["--syn-keyword", "--syn-string", "--syn-number", "--syn-comment", "--syn-purple", "--text-gutter"]) {
      expect(contrastRatio(D(token), D("--bg")), token).toBeGreaterThanOrEqual(4.5);
    }
  });
});

type Accent = (typeof ACCENTS)[number];
type Theme = "dark" | "light";

/** The rule that carries an accent's tokens in a theme (teal is the default rule of each theme). */
function accentRule(theme: Theme, a: Accent): Record<string, string> {
  if (theme === "dark") return a === "teal" ? block(":root {") : block(`:root[data-accent="${a}"]`);
  return a === "teal" ? block(':root[data-theme="light"] {') : block(`:root[data-theme="light"][data-accent="${a}"]`);
}

const RGBA = /^rgba\(\s*(\d+),\s*(\d+),\s*(\d+),\s*([\d.]+)\)$/;

/** Alpha of an accent's own translucent token; fails unless it is `rgba(<that accent's rgb>, alpha)`. */
function accentAlpha(theme: Theme, a: Accent, name: string): number {
  const value = accentRule(theme, a)[name];
  if (value === undefined) throw new Error(`${theme} ${a}: ${name} is missing`);
  const m = RGBA.exec(value);
  if (!m) throw new Error(`${theme} ${a}: ${name} is not rgba(): ${value}`);
  const hex = ACCENT_VALUES[a][theme];
  const channels = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
  expect([m[1], m[2], m[3]].map(Number), `${theme} ${a} ${name} is the accent colour`).toEqual(channels);
  return Number(m[4]);
}

/** An accent's `--selection-bg` as the solid colour it gives over `bg`. */
function selectionOver(theme: Theme, a: Accent, bg: string): string {
  accentAlpha(theme, a, "--selection-bg");
  return solid(accentRule(theme, a)["--selection-bg"] ?? "", bg);
}

const EDITOR_SETS: Record<Theme, { tokens: Record<string, string>; color: (name: string, over?: string) => string }> = {
  dark: { tokens: rootDark, color: D },
  light: { tokens: light, color: L },
};

/** Declarations of the editor theme rule whose selector (one of a comma list) ends with `selector`. */
function cmRule(selector: string): Record<string, string> {
  const rules = EditorState.create({ extensions: cmTheme })
    .facet(EditorView.styleModule)
    .flatMap((m) => m.getRules().split("\n"));
  const found = rules.find((line) => (line.split("{")[0] ?? "").split(",").some((s) => s.trim().endsWith(selector)));
  if (!found) throw new Error(`no editor theme rule for ${selector}`);
  const out: Record<string, string> = {};
  for (const m of found.slice(found.indexOf("{") + 1).matchAll(/([a-z-]+)\s*:\s*([^;]+);/g)) out[m[1] ?? ""] = (m[2] ?? "").trim();
  return out;
}

/** `under` with a match decoration's background (a token, or none) painted on top. */
function withFill(fill: string | undefined, tokens: Record<string, string>, under: string): string {
  if (fill === undefined || fill === "transparent") return under;
  const token = /^var\((--[a-z0-9-]+)\)$/.exec(fill)?.[1];
  const value = token === undefined ? undefined : tokens[token];
  if (value === undefined) throw new Error(`unsupported fill ${fill}`);
  return solid(value, under);
}

describe("syntax colours: every --syn-* token reaches 4.5:1 on the background, the selection and the active line", () => {
  for (const [theme, { tokens, color }] of Object.entries(EDITOR_SETS) as [Theme, (typeof EDITOR_SETS)[Theme]][]) {
    it(`${theme} theme`, () => {
      const names = [...Object.keys(tokens).filter((name) => name.startsWith("--syn-")), "--text"];
      expect(names.length).toBeGreaterThanOrEqual(6);
      const bg = color("--bg");
      for (const name of names) {
        for (const surface of ["--bg", "--active-line"]) {
          expect(contrastRatio(color(name), color(surface)), `${theme} ${name} on ${surface}`).toBeGreaterThanOrEqual(4.5);
        }
        // The selection is drawn behind the lines; on the cursor line the translucent active line lies on top of it.
        for (const a of ACCENTS) {
          const selection = selectionOver(theme, a, bg);
          const selectionOnActiveLine = solid(tokens["--active-line"] ?? "", selection);
          expect(contrastRatio(color(name), selection), `${theme} ${name} on the ${a} selection`).toBeGreaterThanOrEqual(4.5);
          expect(contrastRatio(color(name), selectionOnActiveLine), `${theme} ${name} on the ${a} selection under the active line`).toBeGreaterThanOrEqual(
            4.5,
          );
        }
      }
    });
  }
});

/** Strength of the selection tint: the share of the accent in `--selection-bg` (the maintainer's call for #40). */
const SELECTION_ALPHA: Record<Theme, number> = { dark: 0.22, light: 0.2 };

describe("editor selection and active line (#40)", () => {
  it("tokens.css uses no color-mix() (macOS 11 WebKit lacks it; a failed token would hide the selection)", () => {
    expect(tokensCss.replace(/\/\*[\s\S]*?\*\//g, "")).not.toMatch(/color-mix\(/);
  });

  for (const [theme, { tokens, color }] of Object.entries(EDITOR_SETS) as [Theme, (typeof EDITOR_SETS)[Theme]][]) {
    it(`${theme}: every accent has its own selection (${SELECTION_ALPHA[theme] * 100} %) and match outline (45 %) as rgba()`, () => {
      for (const a of ACCENTS) {
        expect(accentAlpha(theme, a, "--selection-bg"), `${theme} ${a} selection`).toBe(SELECTION_ALPHA[theme]);
        expect(accentAlpha(theme, a, "--selection-match-outline"), `${theme} ${a} match outline`).toBe(0.45);
      }
    });

    it(`${theme}: match highlights add no fill on the selection (search match on the cursor line keeps 4.5:1)`, () => {
      // The current search match is selected, so it sits on the selection under the active line; other occurrences of
      // the selected text can sit on the active line. Whatever fill these rules paint is stacked on top.
      const names = [...Object.keys(tokens).filter((name) => name.startsWith("--syn-")), "--text"];
      const bg = color("--bg");
      for (const selector of [".cm-searchMatch-selected", ".cm-selectionMatch"]) {
        const fill = cmRule(selector)["background-color"];
        for (const a of ACCENTS) {
          const under = solid(tokens["--active-line"] ?? "", selectionOver(theme, a, bg));
          const top = withFill(fill, { ...tokens, ...accentRule(theme, a) }, under);
          for (const name of names) {
            expect(contrastRatio(color(name), top), `${theme} ${name} in ${selector} on the ${a} selection`).toBeGreaterThanOrEqual(4.5);
          }
        }
      }
    });

    it(`${theme}: the active line is translucent, so the selection layer behind the lines shows through`, () => {
      const alpha = /^rgba\(\s*\d+,\s*\d+,\s*\d+,\s*([\d.]+)\)$/.exec(tokens["--active-line"] ?? "")?.[1];
      expect(alpha, `${theme} --active-line must be rgba()`).toBeDefined();
      expect(Number(alpha)).toBeGreaterThan(0);
      expect(Number(alpha)).toBeLessThanOrEqual(0.15);
    });

    it(`${theme}: the selection of every accent clearly stands out from the background and from the active line`, () => {
      // 20-25 % of the accent (the maintainer's call for #40); the weakest accent still differs by 1.3:1 in luminance
      // alone, on top of its hue (the former grey selection was 1.35:1 but hidden on the cursor line).
      const bg = color("--bg");
      const activeLine = color("--active-line");
      for (const a of ACCENTS) {
        const selection = selectionOver(theme, a, bg);
        const selectionOnActiveLine = solid(tokens["--active-line"] ?? "", selection);
        expect(contrastRatio(selection, bg), `${theme} ${a} selection on the background`).toBeGreaterThanOrEqual(1.3);
        expect(contrastRatio(selectionOnActiveLine, activeLine), `${theme} ${a} selection on the active line`).toBeGreaterThanOrEqual(1.3);
      }
    });

    it(`${theme}: comments stay muted next to normal text`, () => {
      // Lightened (dark) or darkened (light) for the selection, but still clearly apart from --text.
      expect(contrastRatio(color("--syn-comment"), color("--text")), `${theme} comment vs text`).toBeGreaterThanOrEqual(1.5);
    });

    it(`${theme}: whitespace markers are faint (visible, but far below text contrast)`, () => {
      const marker = /^rgba\(\s*\d+,\s*\d+,\s*\d+,\s*([\d.]+)\)$/.exec(tokens["--whitespace-marker"] ?? "")?.[1];
      expect(Number(marker), `${theme} --whitespace-marker is semi-transparent`).toBeLessThan(1);
      for (const surface of ["--bg", "--active-line"]) {
        const under = color(surface);
        const ratio = contrastRatio(color("--whitespace-marker", under), under);
        expect(ratio, `${theme} marker on ${surface}`).toBeGreaterThanOrEqual(1.3);
        expect(ratio, `${theme} marker on ${surface}`).toBeLessThan(2.5);
      }
    });
  }
});

describe("focus indicators", () => {
  it("never rely on the translucent --accent-ring alone (it is about 1.3:1)", () => {
    const root = "src";
    const files = (readdirSync(root, { recursive: true }) as string[]).filter((f) => f.endsWith(".css"));
    const offenders: string[] = [];
    for (const file of files) {
      const css = readFileSync(join(root, file), "utf8");
      for (const rule of css.split("}")) {
        if (/:focus(-visible|-within)?\b/.test(rule) && rule.includes("var(--accent-ring)") && !/border(-color)?:[^;]*var\(--accent\)/.test(rule)) {
          offenders.push(`${file}: ${rule.trim().split("{")[0]?.trim() ?? ""}`);
        }
      }
    }
    expect(offenders).toEqual([]);
  });
});
