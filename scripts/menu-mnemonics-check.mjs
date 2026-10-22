// Part of `npm run i18n:check`: in every locale, each menu of the menu bar must be reachable with Alt+letter (a title
// that exists and starts with a single-UTF-16-unit mnemonic, as the menu bar's Alt handler requires) and no two menus
// may resolve to the same mnemonic (Alt+letter opens the first match, the other menu would be unreachable by keyboard).
// The dropdown items have no mnemonics (type-ahead cycles through equal first letters), so the top-level menu titles
// are the only group.
// Imports the TypeScript helpers the UI uses (Node strips the types: erasable syntax and `.ts` extensions only).
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import process from "node:process";
import { fileURLToPath, pathToFileURL } from "node:url";
import { MENU_IDS } from "../src/lib/actionIds.ts";
import { isMnemonicReachable, mnemonicCollisions } from "../src/lib/menuMnemonic.ts";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const LOCALES = join(ROOT, "src", "i18n", "locales");

/**
 * Problems of one locale's `menubar` table (an object id -> title), split into `titles` (missing or not reachable
 * with Alt+letter) and `collisions` (menus sharing a mnemonic). Both empty when the bar is fine.
 */
export function checkLocaleParts(locale, menubar, menuIds = MENU_IDS) {
  const titles = {};
  const titleProblems = [];
  for (const id of menuIds) {
    const title = menubar?.[id];
    if (typeof title !== "string") {
      titleProblems.push(`${locale}: menubar.${id} is missing`);
    } else if (!isMnemonicReachable(title)) {
      titleProblems.push(
        `${locale}: menubar.${id} ("${title}") cannot be opened with Alt+letter: ` +
          "the title is empty or its first character is not a single UTF-16 letter once lower-cased",
      );
    } else {
      titles[id] = title;
    }
  }
  const collisions = mnemonicCollisions(titles).map(({ letter, ids }) => {
    const items = ids.map((id) => `menubar.${id} ("${titles[id]}")`).join(", ");
    return `${locale}: menu bar: mnemonic "${letter}" is shared by ${items}`;
  });
  return { titles: titleProblems, collisions };
}

/** Error lines for one locale's `menubar` table; [] when every menu is reachable with a unique mnemonic. */
export function checkLocale(locale, menubar, menuIds = MENU_IDS) {
  const { titles, collisions } = checkLocaleParts(locale, menubar, menuIds);
  return [...titles, ...collisions];
}

function main() {
  const titleProblems = [];
  const collisions = [];
  const locales = readdirSync(LOCALES, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name);
  for (const locale of locales) {
    const json = JSON.parse(readFileSync(join(LOCALES, locale, "translation.json"), "utf8"));
    const parts = checkLocaleParts(locale, json.menubar);
    titleProblems.push(...parts.titles);
    collisions.push(...parts.collisions);
  }
  if (titleProblems.length > 0) {
    console.error("Menu bar titles missing or not reachable with Alt+letter:\n" + titleProblems.map((p) => `  ${p}`).join("\n"));
    console.error("Give every menu a title starting with a letter (src/i18n/locales/<locale>/translation.json).");
  }
  if (collisions.length > 0) {
    console.error("Menu mnemonic collisions:\n" + collisions.map((p) => `  ${p}`).join("\n"));
    console.error("Rename one of the menus so each starts with a different letter (src/i18n/locales/<locale>/translation.json).");
  }
  if (titleProblems.length > 0 || collisions.length > 0) process.exit(1);
  console.log("Menu mnemonics: every menu is reachable and no mnemonic is shared.");
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main();
