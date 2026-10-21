// Locale key parity and plural readiness (docs/ARCHITECTURE.md §8.2.4). Runs with `npm run test`.
// Every non-English locale must have exactly the English keys (plural-aware: uk adds `_few` and `_many`), the same
// interpolation tokens, and every pluralized key must be called with `count` so those forms can be selected.
import { describe, expect, it } from "vitest";
import en from "./locales/en/translation.json";

type Bundle = Record<string, unknown>;

const PLURAL_SUFFIX = /_(zero|one|two|few|many|other)$/;

const bundles = import.meta.glob<Bundle>("./locales/*/translation.json", { eager: true, import: "default" });
// All app sources (new files are picked up when the test file itself changes; vitest caches the expanded glob).
const sources = import.meta.glob<string>(["/src/**/*.{ts,tsx}", "!/src/**/*.test.{ts,tsx}", "!/src/ipc/bindings/**"], {
  eager: true,
  query: "?raw",
  import: "default",
});

function flatten(node: unknown, prefix = "", out: Record<string, string> = {}): Record<string, string> {
  if (typeof node === "string") out[prefix] = node;
  else if (node && typeof node === "object") {
    for (const [k, v] of Object.entries(node)) flatten(v, prefix ? `${prefix}.${k}` : k, out);
  }
  return out;
}

function base(key: string): string {
  return key.replace(PLURAL_SUFFIX, "");
}

function forms(flat: Record<string, string>): Map<string, Set<string>> {
  const out = new Map<string, Set<string>>();
  for (const key of Object.keys(flat)) {
    const m = PLURAL_SUFFIX.exec(key);
    const b = base(key);
    const set = out.get(b) ?? new Set<string>();
    if (m?.[1]) set.add(m[1]);
    out.set(b, set);
  }
  return out;
}

function tokens(s: string): string[] {
  return [...new Set([...s.matchAll(/\{\{\s*([\w.]+)\s*(?:,[^}]*)?\}\}/g)].map((m) => m[1] ?? ""))].sort();
}

function tokensOf(flat: Record<string, string>, b: string): string[] {
  const all = Object.entries(flat).filter(([k]) => base(k) === b).flatMap(([, v]) => tokens(v));
  return [...new Set(all)].filter((t) => t !== "count").sort();
}

/** True when the source has `t("<key>", { ... count ... })`. */
function callsWithCount(code: string, key: string): boolean {
  const quoted = key.replaceAll(".", String.raw`\.`);
  return new RegExp(String.raw`["'` + "`" + String.raw`]${quoted}["'` + "`" + String.raw`]\s*,\s*\{[^}]*\bcount\b`).test(code);
}

const enFlat = flatten(en);
const enForms = forms(enFlat);
const locales = Object.entries(bundles)
  .map(([path, bundle]) => ({ lng: /locales\/([^/]+)\//.exec(path)?.[1] ?? "", flat: flatten(bundle) }))
  .filter((l) => l.lng !== "en");

describe("locale files", () => {
  it("English has plural forms `one` and `other` for every pluralized key", () => {
    const bad = [...enForms].filter(([, f]) => f.size > 0 && !(f.has("one") && f.has("other"))).map(([k]) => k);
    expect(bad).toEqual([]);
  });

  it("every pluralized key is called with `count`", () => {
    const code = Object.values(sources).join("\n");
    const dynamic = ["prompt.errors."]; // `prompt.errors.${lastError}` is called with `count: attemptsLeft`
    const missing = [...enForms]
      .filter(([k, f]) => f.size > 0 && !dynamic.some((d) => k.startsWith(d)))
      .map(([k]) => k)
      .filter((k) => !callsWithCount(code, k));
    expect(missing).toEqual([]);
  });

  it("has at least the English locale (uk is checked once its file exists)", () => {
    // The key-parity checks below only run for locale files that exist. They are skipped, not failed, while a
    // translation is still being written; a registered language without a file simply falls back to English.
    expect(Object.keys(bundles).some((p) => p.includes("/en/"))).toBe(true);
  });

  for (const { lng, flat } of locales) {
    describe(lng, () => {
      const lngForms = forms(flat);
      const needed = new Set(new Intl.PluralRules(lng).resolvedOptions().pluralCategories);

      it("has every English key and no extra keys (plural-aware)", () => {
        const missing = [...enForms.keys()].filter((k) => !lngForms.has(k));
        const extra = [...lngForms.keys()].filter((k) => !enForms.has(k));
        expect({ missing, extra }).toEqual({ missing: [], extra: [] });
      });

      it("has the plural forms of the language for every pluralized key", () => {
        const bad = [...enForms]
          .filter(([, f]) => f.size > 0)
          .map(([k]) => ({ k, have: lngForms.get(k) ?? new Set<string>() }))
          .filter(({ have }) => [...needed].some((c) => !have.has(c)) || [...have].some((c) => !needed.has(c as never)))
          .map(({ k, have }) => `${k}: ${[...have].join(",")}`);
        expect(bad).toEqual([]);
      });

      it("keeps the interpolation tokens of every key", () => {
        const bad = [...enForms.keys()]
          .filter((k) => lngForms.has(k))
          .filter((k) => tokensOf(enFlat, k).join() !== tokensOf(flat, k).join());
        expect(bad).toEqual([]);
      });

      it("has no empty strings", () => {
        expect(Object.entries(flat).filter(([, v]) => v.trim() === "").map(([k]) => k)).toEqual([]);
      });
    });
  }
});
