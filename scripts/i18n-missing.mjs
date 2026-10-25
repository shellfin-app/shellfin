// `npm run i18n:missing`: the keys each locale has not translated yet, measured against English (ARCHITECTURE
// §8.2.4). Only English is complete and CI-enforced; any other locale may leave keys out, and i18next then shows the
// English text. A plural key counts as missing when any form the language needs (Intl.PluralRules, as i18next uses)
// is missing; an empty string counts as missing too (it falls back to English, `returnEmptyString: false`).
// Exits 0 (CI prints the report as information) unless `--strict` is passed, then 1 when anything is missing.
// `--github` adds one GitHub Actions `::warning::` annotation per incomplete language (the CI step uses it).
//
// `--prune` (run by `npm run i18n:extract`, after the extractor has updated English) removes from every other locale
// what `src/i18n/parity.test.ts` rejects and cannot be translated into place: keys English no longer has, plural forms
// the language does not use, a plural/non-plural mismatch with English, and empty strings. It never adds a key:
// i18next-cli only manages the English file (`locales: ["en"]` in i18next.config.ts), so contributors do not get ""
// placeholders in other locales.
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import process from "node:process";
import { fileURLToPath, pathToFileURL } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const LOCALES = join(ROOT, "src", "i18n", "locales");
const PRIMARY = "en";

export const PLURAL_SUFFIX = /_(zero|one|two|few|many|other)$/;

/** `{ a: { b: "x" } }` -> `{ "a.b": "x" }`; non-string leaves are ignored. */
export function flatten(node, prefix = "", out = {}) {
  if (typeof node === "string") out[prefix] = node;
  else if (node && typeof node === "object") {
    for (const [k, v] of Object.entries(node)) flatten(v, prefix ? `${prefix}.${k}` : k, out);
  }
  return out;
}

/** The CLDR plural categories i18next selects for `lng` (`one`, `other` for English). */
export function pluralCategories(lng) {
  return new Intl.PluralRules(lng).resolvedOptions().pluralCategories;
}

/**
 * The leaf keys a complete translation of `enFlat` has in `lng`, by English base key: a plain key maps to itself, a
 * plural key (English `x_one`/`x_other`) to `x_<category>` for every category of the language.
 */
export function expectedKeys(enFlat, lng) {
  const plural = new Set();
  const plain = new Set();
  for (const key of Object.keys(enFlat)) {
    if (PLURAL_SUFFIX.test(key)) plural.add(key.replace(PLURAL_SUFFIX, ""));
    else plain.add(key);
  }
  const cats = pluralCategories(lng);
  const out = new Map();
  for (const base of plain) out.set(base, [base]);
  for (const base of plural) out.set(base, cats.map((c) => `${base}_${c}`));
  return out;
}

/**
 * English base keys that `lngFlat` does not translate (completely), sorted. For a plural key with some forms present
 * the entry names the missing forms: `hosts.count (few, many)`.
 */
export function missingKeys(enFlat, lngFlat, lng) {
  const has = (k) => typeof lngFlat[k] === "string" && lngFlat[k].trim() !== "";
  const out = [];
  for (const [base, leaves] of expectedKeys(enFlat, lng)) {
    const absent = leaves.filter((k) => !has(k));
    if (absent.length === 0) continue;
    const isPlural = leaves.length > 1 || leaves[0] !== base;
    if (isPlural && absent.length < leaves.length) {
      out.push(`${base} (${absent.map((k) => PLURAL_SUFFIX.exec(k)?.[1]).join(", ")})`);
    } else out.push(base);
  }
  return out.sort();
}

/** Leaf keys of `lngFlat` that a translation of `enFlat` into `lng` cannot have, sorted. */
export function extraKeys(enFlat, lngFlat, lng) {
  const allowed = new Set([...expectedKeys(enFlat, lng).values()].flat());
  return Object.keys(lngFlat).filter((k) => !allowed.has(k)).sort();
}

/** Deep copy of `lngJson` without extra keys, empty strings and objects left empty; `removed` lists the leaf keys. */
export function pruneLocale(enJson, lngJson, lng) {
  const allowed = new Set([...expectedKeys(flatten(enJson), lng).values()].flat());
  const removed = [];
  const walk = (node, prefix) => {
    const out = {};
    for (const [k, v] of Object.entries(node)) {
      const path = prefix ? `${prefix}.${k}` : k;
      if (v && typeof v === "object" && !Array.isArray(v)) {
        const child = walk(v, path);
        if (Object.keys(child).length > 0) out[k] = child;
      } else if (typeof v === "string" && allowed.has(path) && v.trim() !== "") out[k] = v;
      else removed.push(path);
    }
    return out;
  };
  return { json: walk(lngJson, ""), removed };
}

/** The report lines for one locale. */
export function reportLines(lng, missing) {
  if (missing.length === 0) return [`${lng}: complete`];
  return [`${lng}: ${missing.length} missing`, ...missing.map((k) => `  ${k}`)];
}

/** Runs the report (and `--prune`) over `localesDir`; returns the exit code. */
export function run(argv, localesDir = LOCALES, log = console.log) {
  const args = new Set(argv);
  const file = (lng) => join(localesDir, lng, "translation.json");
  const read = (lng) => JSON.parse(readFileSync(file(lng), "utf8"));
  const en = read(PRIMARY);
  const enFlat = flatten(en);
  const locales = readdirSync(localesDir, { withFileTypes: true })
    .filter((entry) => entry.isDirectory() && entry.name !== PRIMARY)
    .map((entry) => entry.name)
    .sort();
  let total = 0;
  for (const lng of locales) {
    let json = read(lng);
    if (args.has("--prune")) {
      const pruned = pruneLocale(en, json, lng);
      if (pruned.removed.length > 0) {
        writeFileSync(file(lng), `${JSON.stringify(pruned.json, null, 2)}\n`);
        log(`${lng}: removed ${pruned.removed.join(", ")}`);
      }
      json = pruned.json;
    }
    const missing = missingKeys(enFlat, flatten(json), lng);
    total += missing.length;
    for (const line of reportLines(lng, missing)) log(line);
    if (args.has("--github") && missing.length > 0) {
      log(`::warning title=Untranslated keys::${lng}: ${missing.length} keys fall back to English (npm run i18n:missing)`);
    }
  }
  if (total > 0) {
    log("Untranslated keys fall back to English. Translate them in src/i18n/locales/<lng>/translation.json (CONTRIBUTING, Adding a translation).");
  }
  return args.has("--strict") && total > 0 ? 1 : 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exitCode = run(process.argv.slice(2));
}
