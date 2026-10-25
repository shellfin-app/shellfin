// Name ordering in the UI language (not the OS default locale), so lists sort the same way the UI reads (#17).
// No store import: pure modules (bookmark ordering) use it.
import i18next from "i18next";

const collators = new Map<string, Intl.Collator>();

/** Compares two display names with the collator of the current UI language (en-XA sorts like en). */
export function compareNames(a: string, b: string): number {
  const raw = i18next.language || "en";
  const lng = raw === "en-XA" ? "en" : raw;
  let collator = collators.get(lng);
  if (!collator) {
    collator = new Intl.Collator(lng);
    collators.set(lng, collator);
  }
  return collator.compare(a, b);
}
