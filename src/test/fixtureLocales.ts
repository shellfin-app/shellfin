// Test-only locale fixtures for the i18n infrastructure (#17): a CJK-like and a long, German-like menu bar, so the
// mnemonic markers, the CJK rules and the compact title bar are proven before any real language ships. Never
// registered in SUPPORTED_LANGUAGES and never a file under src/i18n/locales/ (the check script and the build would
// pick it up there).

/** Japanese-style titles with the Windows "(&F)" convention and the English letters. */
export const CJK_MENUBAR = {
  file: "ファイル(&F)",
  edit: "編集(&E)",
  view: "表示(&V)",
  connection: "接続(&C)",
  terminal: "ターミナル(&T)",
  help: "ヘルプ(&H)",
  window: "ウィンドウ(&W)",
} as const;

/** Long, accented Latin titles with first-letter collisions resolved by markers (de / fr / es-like). */
export const LONG_MENUBAR = {
  file: "Dateiverwaltung",
  edit: "&Édition des éléments",
  view: "Ansichtsoptionen",
  connection: "Verbindungsverwaltung",
  terminal: "Terminalsitzungen",
  help: "Ay&uda und Unterstützung",
  window: "Fensteranordnung",
} as const;
