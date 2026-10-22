// Menu bar mnemonics. Pure helpers, shared by the menu bar (Alt+letter, the underlined letter) and by the
// `npm run i18n:check` guard (`scripts/menu-mnemonics-check.mjs`), so both resolve a mnemonic the same way.
// WARNING: imported directly by Node (`scripts/menu-mnemonics-check.mjs`, type stripping): erasable TypeScript syntax
// only (no enums, namespaces or parameter properties), and value imports need explicit `.ts` extensions.

/** The mnemonic of a menu title: its first character, lower-cased. An empty title has none. */
export function mnemonicOf(title: string): string | undefined {
  const first = Array.from(title)[0];
  return first === undefined ? undefined : first.toLowerCase();
}

/** The first character of a title as written (the one the menu bar underlines); "" for an empty title. */
export function mnemonicChar(title: string): string {
  return Array.from(title)[0] ?? "";
}

/**
 * Whether Alt+letter can open a menu with this title. The menu bar's Alt handler accepts only keys with
 * `e.key.length === 1`, so the mnemonic must exist and be a single UTF-16 unit (not an empty title, an astral
 * first character, or a letter whose lower-case form expands, such as "İ").
 */
export function isMnemonicReachable(title: string): boolean {
  return mnemonicOf(title)?.length === 1;
}

export interface MnemonicCollision {
  letter: string;
  /** The menu ids (in the given order) whose titles resolve to `letter`. */
  ids: string[];
}

/**
 * Menus of one bar (id -> translated title) that share a mnemonic. Alt+letter opens the first of them, so the
 * others are unreachable by keyboard. A title with no mnemonic (empty) cannot collide.
 */
export function mnemonicCollisions(titles: Readonly<Record<string, string>>): MnemonicCollision[] {
  const byLetter = new Map<string, string[]>();
  for (const [id, title] of Object.entries(titles)) {
    const letter = mnemonicOf(title);
    if (letter === undefined) continue;
    byLetter.set(letter, [...(byLetter.get(letter) ?? []), id]);
  }
  return [...byLetter].filter(([, ids]) => ids.length > 1).map(([letter, ids]) => ({ letter, ids }));
}
