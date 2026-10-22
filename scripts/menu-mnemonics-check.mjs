// Part of `npm run i18n:check`: in every locale, no two menus of the menu bar may resolve to the same mnemonic
// (Alt+letter opens the first match, the other menu would be unreachable by keyboard). The dropdown items have no
// mnemonics (type-ahead cycles through equal first letters), so the top-level menu titles are the only group.
// Imports the TypeScript helpers the UI uses (Node strips the types).
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import process from "node:process";
import { fileURLToPath, pathToFileURL } from "node:url";
import { MENU_IDS } from "../src/lib/actionIds.ts";
import { mnemonicCollisions } from "../src/lib/menuMnemonic.ts";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const LOCALES = join(ROOT, "src", "i18n", "locales");

/** Error lines for one locale's `menubar` table (an object id -> title); [] when every mnemonic is unique. */
export function checkLocale(locale, menubar, menuIds = MENU_IDS) {
  const titles = {};
  const problems = [];
  for (const id of menuIds) {
    const title = menubar?.[id];
    if (typeof title !== "string") problems.push(`${locale}: menubar.${id} is missing`);
    else titles[id] = title;
  }
  for (const { letter, ids } of mnemonicCollisions(titles)) {
    const items = ids.map((id) => `menubar.${id} ("${titles[id]}")`).join(", ");
    problems.push(`${locale}: menu bar: mnemonic "${letter}" is shared by ${items}`);
  }
  return problems;
}

function main() {
  const problems = [];
  for (const locale of readdirSync(LOCALES)) {
    const json = JSON.parse(readFileSync(join(LOCALES, locale, "translation.json"), "utf8"));
    problems.push(...checkLocale(locale, json.menubar));
  }
  if (problems.length > 0) {
    console.error("Menu mnemonic collisions:\n" + problems.map((p) => `  ${p}`).join("\n"));
    console.error("Rename one of the menus so each starts with a different letter (src/i18n/locales/<locale>/translation.json).");
    process.exit(1);
  }
  console.log("Menu mnemonics: no collisions.");
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main();
