import i18next from "i18next";
import { initReactI18next } from "react-i18next";
import en from "./locales/en/translation.json";
import { PSEUDO_LOCALE, PSEUDO_PADDING, pseudoLocalizeBundle } from "./pseudo";

/** Languages shipped in this build. Each non-English one is a lazily loaded JSON chunk (§8.2). */
export const SUPPORTED_LANGUAGES = ["en", "de", "uk"] as const;
export type SupportedLanguage = (typeof SUPPORTED_LANGUAGES)[number];

/** Endonyms for the language picker: never translated, so a user can always find their own language. */
export const LANGUAGE_ENDONYMS: Record<SupportedLanguage, string> = {
  en: "English",
  de: "Deutsch",
  uk: "Українська",
};

/**
 * `beta`: a new translation without a native-speaker review yet. The picker shows "(beta)" and a feedback link; the
 * "System" option still resolves to it. Set to `complete` once a native review is merged (CONTRIBUTING, Translations).
 */
export const LANGUAGE_STATUS: Record<SupportedLanguage, "complete" | "beta"> = {
  en: "complete",
  de: "beta",
  uk: "complete",
};

/**
 * Region and script rules for `resolveLocale` (decision 4 of #17): a tag (lower-case, a prefix of the requested tag
 * at subtag boundaries) mapped to the language it selects, or `null` for "none of ours, try the next preference".
 * Longest prefix wins. Traditional Chinese has no translation and must not fall back to Simplified; every
 * Portuguese gets Brazilian Portuguese; bare `zh` means Simplified (what browsers report for zh-CN systems).
 */
const LOCALE_RULES: Readonly<Record<string, string | null>> = {
  zh: "zh-CN",
  "zh-cn": "zh-CN",
  "zh-sg": "zh-CN",
  "zh-my": "zh-CN",
  "zh-hans": "zh-CN",
  "zh-tw": null,
  "zh-hk": null,
  "zh-mo": null,
  "zh-hant": null,
  pt: "pt-BR",
};

/**
 * The supported language for one BCP-47 tag, or undefined: an exact (case-insensitive) match, then the rule table
 * above, then the base language ("de-AT" -> "de", "uk-UA" -> "uk"). Only languages in `supported` are returned.
 */
export function matchLanguage(tag: string, supported: readonly string[] = SUPPORTED_LANGUAGES): string | undefined {
  const lower = tag.trim().toLowerCase().replaceAll("_", "-");
  if (!/^[a-z]{2,3}(-[a-z0-9]{1,8})*$/.test(lower)) return undefined; // "", "x-foo", "*", garbage
  const exact = supported.find((s) => s.toLowerCase() === lower);
  if (exact) return exact;
  const subtags = lower.split("-");
  for (let n = subtags.length; n >= 1; n -= 1) {
    const prefix = subtags.slice(0, n).join("-");
    if (!Object.hasOwn(LOCALE_RULES, prefix)) continue;
    const target = LOCALE_RULES[prefix];
    return target === null || target === undefined ? undefined : supported.find((s) => s === target);
  }
  const base = subtags[0];
  return supported.find((s) => s === base);
}

/**
 * Maps `Settings.locale` ("system" | BCP-47) to a supported language: each preferred tag in order (`matchLanguage`),
 * finally "en". `supported` is for tests (fixture languages); the app always uses `SUPPORTED_LANGUAGES`.
 */
export function resolveLocale(
  setting: string,
  systemLanguages: readonly string[] = navigator.languages,
  supported: readonly string[] = SUPPORTED_LANGUAGES,
): string {
  const wanted = setting === "system" ? systemLanguages : [setting];
  for (const lang of wanted) {
    const hit = matchLanguage(lang, supported);
    if (hit) return hit;
  }
  return "en";
}

// English is bundled (it is the fallback); every other language is its own chunk, fetched on demand.
const loaders = import.meta.glob<{ default: Record<string, unknown> }>([
  "./locales/*/translation.json",
  "!./locales/en/translation.json",
]);

/** Makes sure the resource bundle of `lng` is loaded. Resolves false when there is no bundle for it. */
export async function ensureLanguage(lng: string): Promise<boolean> {
  if (lng === "en" || i18next.hasResourceBundle(lng, "translation")) return true;
  const load = loaders[`./locales/${lng}/translation.json`];
  if (!load) return false;
  try {
    const mod = await load();
    i18next.addResourceBundle(lng, "translation", mod.default, true, true);
    return true;
  } catch {
    return false; // a missing chunk must not break the UI: English is the fallback
  }
}

let languageRequest = 0;

/** Loads (if needed) and activates `lng`; keeps `<html lang>` in sync for screen readers and hyphenation. */
export async function setLanguage(lng: string): Promise<void> {
  const token = ++languageRequest;
  const ok = await ensureLanguage(lng);
  if (token !== languageRequest) return; // a newer request arrived while the bundle was loading: latest wins
  const active = ok ? lng : "en";
  if (i18next.language !== active) await i18next.changeLanguage(active);
  if (typeof document !== "undefined") document.documentElement.lang = active;
}

export async function initI18n(locale = "en") {
  await i18next.use(initReactI18next).init({
    resources: { en: { translation: en } },
    lng: "en",
    fallbackLng: "en",
    supportedLngs: import.meta.env.DEV ? [...SUPPORTED_LANGUAGES, PSEUDO_LOCALE] : [...SUPPORTED_LANGUAGES],
    interpolation: { escapeValue: false }, // React escapes text nodes. Never pass server text into <Trans>.
    returnNull: false,
  });
  if (locale !== "en") await setLanguage(locale);
  return i18next;
}

/** Dev-only: toggles the en-XA pseudo-locale (accents + `padding`, a share of the letters; `?xa=40` = 0.4). */
export async function setPseudoLocale(enabled: boolean, padding = PSEUDO_PADDING) {
  if (!import.meta.env.DEV) return;
  if (enabled) {
    i18next.addResourceBundle(PSEUDO_LOCALE, "translation", pseudoLocalizeBundle(en as Record<string, unknown>, padding), true, true);
  }
  await i18next.changeLanguage(enabled ? PSEUDO_LOCALE : "en");
}

export { i18next };
