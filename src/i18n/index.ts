import i18next from "i18next";
import { initReactI18next } from "react-i18next";
import en from "./locales/en/translation.json";
import { PSEUDO_LOCALE, pseudoLocalizeBundle } from "./pseudo";

/** Languages shipped in this build. Each non-English one is a lazily loaded JSON chunk (§8.2). */
export const SUPPORTED_LANGUAGES = ["en", "uk"] as const;
export type SupportedLanguage = (typeof SUPPORTED_LANGUAGES)[number];

/** Endonyms for the language picker: never translated, so a user can always find their own language. */
export const LANGUAGE_ENDONYMS: Record<SupportedLanguage, string> = {
  en: "English",
  uk: "Українська",
};

/** Maps `Settings.locale` ("system" | BCP-47) to a supported language. */
export function resolveLocale(setting: string, systemLanguages: readonly string[] = navigator.languages): string {
  const wanted = setting === "system" ? systemLanguages : [setting];
  for (const lang of wanted) {
    const base = lang.split("-")[0]?.toLowerCase();
    const hit = SUPPORTED_LANGUAGES.find((s) => s === base);
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

/** Dev-only: toggles the en-XA pseudo-locale (accents + padding). */
export async function setPseudoLocale(enabled: boolean) {
  if (!import.meta.env.DEV) return;
  if (enabled && !i18next.hasResourceBundle(PSEUDO_LOCALE, "translation")) {
    i18next.addResourceBundle(PSEUDO_LOCALE, "translation", pseudoLocalizeBundle(en as Record<string, unknown>), true, true);
  }
  await i18next.changeLanguage(enabled ? PSEUDO_LOCALE : "en");
}

export { i18next };
