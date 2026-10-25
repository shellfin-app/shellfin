// Locale key parity and plural readiness (docs/ARCHITECTURE.md §8.2.4). Runs with `npm run test`.
// English is the one complete locale. Any other locale may leave keys out (i18next shows the English text; the missing
// keys are only reported here and by `npm run i18n:missing`), but it fails on: a key English does not have (including
// a plural form the language does not use, or a plural/non-plural mismatch), a plural key with only some of the
// language's forms (a partial set would show English for some numbers and the translation for others), a changed set
// of `{{tokens}}`, and an empty string. Every pluralized key must be called with `count` so the forms can be selected.
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

  it("has the English locale (the others are checked below for every file that exists)", () => {
    expect(Object.keys(bundles).some((p) => p.includes("/en/"))).toBe(true);
  });

  for (const { lng, flat } of locales) {
    describe(lng, () => {
      const lngForms = forms(flat);
      const needed = new Intl.PluralRules(lng).resolvedOptions().pluralCategories;
      // The leaf keys a translation may have: a plain English key as is, a plural one with the language's suffixes.
      const allowed = new Set(
        [...enForms].flatMap(([k, f]) => (f.size > 0 ? needed.map((c) => `${k}_${c}`) : [k])),
      );

      it("has no key that English does not have (plural-aware)", () => {
        expect(Object.keys(flat).filter((k) => !allowed.has(k))).toEqual([]);
      });

      it("has all plural forms of the language for every plural key it translates", () => {
        const bad = [...enForms]
          .filter(([k, f]) => f.size > 0 && (lngForms.get(k)?.size ?? 0) > 0)
          .map(([k]) => ({ k, have: lngForms.get(k) ?? new Set<string>() }))
          .filter(({ have }) => needed.some((c) => !have.has(c)))
          .map(({ k, have }) => `${k}: has ${[...have].join(",")}, needs ${needed.join(",")}`);
        expect(bad).toEqual([]);
      });

      it("reports the English keys it does not translate yet (allowed: they fall back to English)", () => {
        const missing = [...enForms.keys()].filter((k) => !lngForms.has(k));
        if (missing.length > 0) console.info(`${lng}: ${missing.length} keys not translated yet (npm run i18n:missing)`);
        expect(missing.length).toBeLessThanOrEqual(enForms.size);
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
