// Part of `npm run i18n:check`: in every locale, each menu of the menu bar must be reachable with Alt+letter and no two
// menus may resolve to the same mnemonic (Alt+letter opens the first match, the other menu would be unreachable by
// keyboard). Title syntax (ARCHITECTURE §8.2.2): `&` marks the mnemonic ("Ay&uda", "ファイル(&F)"), `&&` is a literal
// ampersand, a title without a marker uses its first character. Rules:
//   1. at most one marker; no `&` at the end; no `&` before a space;
//   2. the mnemonic is a letter or a digit, a single UTF-16 unit after folding (lower-cased, accents of Latin letters dropped: `foldMnemonic`);
//   3. CJK titles (Han, Hiragana, Katakana or Hangul in the title, or any title of a ja / ko / zh* locale) need an
//      explicit marker with an ASCII letter, and it must equal the English mnemonic of the same menu (F E V C T H), so
//      muscle memory and the physical-key fallback of the menu bar agree;
//   4. no two menus share a mnemonic after folding ("Édition" and "Extras" collide).
// The dropdown items have no mnemonics (type-ahead cycles through equal first letters), so the top-level menu titles
// are the only group.
// Imports the TypeScript helpers the UI uses (Node strips the types: erasable syntax and `.ts` extensions only).
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import process from "node:process";
import { fileURLToPath, pathToFileURL } from "node:url";
import { MENU_IDS } from "../src/lib/actionIds.ts";
import {
  hasCjkScript,
  isAsciiLetterMnemonic,
  isMnemonicReachable,
  mnemonicCollisions,
  mnemonicOf,
  parseMenuTitle,
} from "../src/lib/menuMnemonic.ts";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const LOCALES = join(ROOT, "src", "i18n", "locales");

/** Locales whose menu titles always need an ASCII-letter marker, whatever script a title uses. */
const CJK_LOCALE = /^(ja|ko|zh)(-|$)/i;

const MARKER_PROBLEMS = {
  multiple: "has more than one & marker (write && for a literal ampersand)",
  trailing: "ends with & (write && for a literal ampersand)",
  beforeSpace: "has & before a space (the mnemonic must be a letter or digit)",
};

function englishMenubar() {
  return JSON.parse(readFileSync(join(LOCALES, "en", "translation.json"), "utf8")).menubar;
}

/**
 * Problems of one locale's `menubar` table (an object id -> title), split into `titles` (missing, bad markers, not
 * reachable with Alt+letter, CJK rule) and `collisions` (menus sharing a mnemonic). Both empty when the bar is fine.
 * `english` is the English `menubar` table (rule 3).
 */
export function checkLocaleParts(locale, menubar, menuIds = MENU_IDS, english = englishMenubar()) {
  const titles = {};
  const titleProblems = [];
  const cjkLocale = CJK_LOCALE.test(locale);
  for (const id of menuIds) {
    const title = menubar?.[id];
    const name = `${locale}: menubar.${id} ("${title}")`;
    if (typeof title !== "string") {
      titleProblems.push(`${locale}: menubar.${id} is missing`);
      continue;
    }
    const parsed = parseMenuTitle(title);
    const problems = parsed.problems.map((p) => `${name} ${MARKER_PROBLEMS[p]}`);
    if (!isMnemonicReachable(title)) {
      problems.push(
        `${name} cannot be opened with Alt+letter: the title is empty or its mnemonic is not a letter or digit ` +
          "that is a single UTF-16 unit once lower-cased",
      );
    } else if (cjkLocale || hasCjkScript(title)) {
      const expected = mnemonicOf(english?.[id] ?? "");
      if (!parsed.marked || !isAsciiLetterMnemonic(title)) {
        problems.push(`${name} needs an explicit ASCII-letter mnemonic like "(&${(expected ?? "x").toUpperCase()})"`);
      } else if (expected !== undefined && mnemonicOf(title) !== expected) {
        problems.push(`${name} must use the English mnemonic "${expected.toUpperCase()}" (got "${mnemonicOf(title)?.toUpperCase()}")`);
      }
    }
    if (problems.length > 0) titleProblems.push(...problems);
    else titles[id] = title;
  }
  const collisions = mnemonicCollisions(titles).map(({ letter, ids }) => {
    const items = ids.map((id) => `menubar.${id} ("${titles[id]}")`).join(", ");
    return `${locale}: menu bar: mnemonic "${letter}" is shared by ${items}`;
  });
  return { titles: titleProblems, collisions };
}

/** Error lines for one locale's `menubar` table; [] when every menu is reachable with a unique mnemonic. */
export function checkLocale(locale, menubar, menuIds = MENU_IDS, english = englishMenubar()) {
  const { titles, collisions } = checkLocaleParts(locale, menubar, menuIds, english);
  return [...titles, ...collisions];
}

function main() {
  const titleProblems = [];
  const collisions = [];
  const english = englishMenubar();
  const locales = readdirSync(LOCALES, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name);
  for (const locale of locales) {
    const json = JSON.parse(readFileSync(join(LOCALES, locale, "translation.json"), "utf8"));
    const parts = checkLocaleParts(locale, json.menubar, MENU_IDS, english);
    titleProblems.push(...parts.titles);
    collisions.push(...parts.collisions);
  }
  if (titleProblems.length > 0) {
    console.error("Menu bar titles missing, badly marked or not reachable with Alt+letter:\n" + titleProblems.map((p) => `  ${p}`).join("\n"));
    console.error("Mark the mnemonic with & (\"Ay&uda\", CJK \"ファイル(&F)\") in src/i18n/locales/<locale>/translation.json (CONTRIBUTING, Translations).");
  }
  if (collisions.length > 0) {
    console.error("Menu mnemonic collisions:\n" + collisions.map((p) => `  ${p}`).join("\n"));
    console.error("Move the & marker of one menu to another letter (src/i18n/locales/<locale>/translation.json).");
  }
  if (titleProblems.length > 0 || collisions.length > 0) process.exit(1);
  console.log("Menu mnemonics: every menu is reachable and no mnemonic is shared.");
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main();
