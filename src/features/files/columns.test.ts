// The file tree's size and date columns (§8.11): the CSS breakpoints are ordered and leave room for names, and every
// formatted value fits its fixed `ch` width in English and Ukrainian, so a locale or ICU change cannot clip silently.
import { readFileSync } from "node:fs";
import { afterAll, describe, expect, it } from "vitest";
import { formatBytes, formatFileDateShort } from "../../i18n/format";
import { setLanguage } from "../../i18n";

// Read from disk: the test config switches CSS processing off, which empties `?raw` CSS imports too.
const css = readFileSync("src/features/files/files.css", "utf8").replace(/\r\n/g, "\n");

/** The sidebar's UI font size at 100 % (`--fs-ui`): what the `em` breakpoints are measured in. */
const UI_PX = 13;
/** The narrowest sidebar (§8.4). */
const SIDEBAR_MIN_PX = 200;

/** `--file-col-size: 9ch` -> 9. */
function chWidth(variable: string): number {
  const match = new RegExp(`${variable}: (\\d+)ch;`).exec(css);
  if (!match?.[1]) throw new Error(`width not found: ${variable}`);
  return Number(match[1]);
}

/** Visible characters (code points): the narrow no-break spaces of ICU count as one each. */
const length = (s: string) => [...s].length;

describe("file tree columns: CSS", () => {
  it("breakpoints: size, then date, then permissions, the first one well above the narrowest sidebar", () => {
    const em = (query: RegExp) => [...css.matchAll(query)].map((m) => Number(m[1]));
    const fromSize = em(/@container \(width >= (\d+(?:\.\d+)?)em\) \{\n {2}\.file-tree--columns \.file-tree__canvas/g);
    const fromDate = em(/@container \(width >= (\d+(?:\.\d+)?)em\) \{\n {2}\.file-tree--columns \.file-row__col--date/g);
    const fromPerms = em(/@container \(\d+(?:\.\d+)?em <= width < (\d+(?:\.\d+)?)em\) \{ \.file-tree--columns \.file-row__meta \{ display: none; \}/g);
    expect(fromSize).toHaveLength(1);
    expect(fromDate).toHaveLength(1);
    expect(fromPerms).toHaveLength(1);
    const [size, date, perms] = [fromSize[0]!, fromDate[0]!, fromPerms[0]!];
    expect(size).toBeLessThan(date);
    expect(date).toBeLessThan(perms);
    // The default 280 px sidebar shows the size column; the narrowest one leaves the tree as it was.
    expect(size * UI_PX).toBeGreaterThan(SIDEBAR_MIN_PX + 60);
    expect(size * UI_PX).toBeLessThanOrEqual(280);
    // The "sorted by the hidden date column" hint covers exactly the range between the two.
    expect(css).toContain(`@container (${size}em <= width < ${date}em) { .file-tree-header__hint--mtime`);
    // The permissions span is hidden only while the columns need its room: below the size breakpoint, and in a
    // webview without container queries, it shows as without columns.
    expect(css).toContain(`@container (${size}em <= width < ${perms}em) { .file-tree--columns .file-row__meta { display: none; } }`);
    const outsideQueries = css.replace(/@container[^{]*\{(?:[^{}]*\{[^{}]*\})*[^{}]*\}/g, "");
    expect(outsideQueries).not.toMatch(/\.file-tree--columns \.file-row__meta \{[^}]*display: none/);
  });

  it("deep nesting: with columns a row clips (no sideways scroll), overflow goes to the left and the name keeps a minimum", () => {
    const block = /@container \(width >= 21em\) \{([^]*?)\n\}/.exec(css)?.[1] ?? "";
    expect(block).toContain(".file-tree--columns .file-row { overflow: hidden; justify-content: flex-end; }");
    expect(block).toMatch(/\.file-tree--columns \.file-row__name \{ flex: 1 1 16ch; min-width: min\(6ch, 100%\); \}/);
    expect(block).toContain(".file-tree--columns .file-tree__canvas { min-width: 0; }");
    // a rename input in a deep row keeps room too
    expect(block).toContain(".file-tree--columns .file-row__input { min-width: min(6ch, 100%); }");
    // the permissions span gives way first (truncating), before the name and long before the indent
    expect(block).toMatch(/\.file-tree--columns \.file-row__meta \{ flex: 0 1000 auto; min-width: 0;[^}]*text-overflow: ellipsis;/);
    // the marks slot has a fixed minimum width, so a lock or a dirty dot does not shift the permissions or cells
    expect(block).toMatch(/\.file-tree--columns \.file-row__marks \{ display: flex; flex: none;[^}]*min-width: 12px; \}/);
    // ...and is transparent without columns (the row is laid out exactly as before)
    expect(css).toContain(".file-row__marks { display: contents; }");
    // no reordering: the row keeps its DOM order (fileColumns.test.tsx), so no `order` rules are needed anywhere
    expect(css).not.toMatch(/\border:/);
    // the cells never shrink, so they stay the width of the header cells
    expect(css).toMatch(/\.file-row__col \{[^}]*flex: none;/);
  });

  it("geometry: at each breakpoint a row up to depth 2 fits without clipping its indent (UI 13 px and 16 px)", () => {
    // Fixed px values from FileTree.tsx (INDENT_BASE, INDENT_STEP, CHEVRON_SLOT, the 15 px icon) and files.css.
    const INDENT_BASE = 10;
    const INDENT_STEP = 18;
    const CHEVRON_SLOT = 20; // a file's empty slot; a folder's 12 px chevron + the 8 px gap is the same
    const ICON = 15;
    const px = (re: RegExp) => Number(re.exec(css)?.[1] ?? Number.NaN);
    const GAP = px(/\.file-row \{[^}]*gap: (\d+)px;/);
    const ROW_PAD_RIGHT = px(/\.file-row \{[^}]*padding-right: (\d+)px;/);
    const TREE_PAD = px(/\.file-tree \{[^}]*padding: 0 (\d+)px 8px;/);
    const COL_MARGIN = px(/\.file-row__col \{[^}]*margin-left: (\d+)px;/);
    const MARKS = px(/\.file-row__marks \{ display: flex;[^}]*min-width: (\d+)px; \}/);
    const META_PAD = px(/\.file-row__meta \{[^}]*padding-left: (\d+)px;/);
    const META_FONT = px(/\.file-row__meta \{[^}]*font-size: calc\((\d+)px \* var\(--ui-scale, 1\)\);/);
    const NAME_MIN = px(/\.file-tree--columns \.file-row__name \{[^}]*min-width: min\((\d+)ch, 100%\);/);
    const NAME_BASIS = px(/\.file-tree--columns \.file-row__name \{ flex: 1 1 (\d+)ch;/);
    const SCROLLBAR = 11; // `scrollbar-width: thin` (global.css), the widest of the three webviews
    const MONO_PX = 12.5; // --fs-mono at 100 %
    const CH = 0.6; // JetBrains Mono advance width per em
    const PERMS_CH = "rwxr-xr-x deploy deploy".length; // a typical permissions span
    const DEPTH = 2; // e.g. /srv/app/data/<entry> with the tree at /srv/app
    const sizeCh = chWidth("--file-col-size");
    const dateCh = chWidth("--file-col-date");
    for (const n of [GAP, ROW_PAD_RIGHT, TREE_PAD, COL_MARGIN, MARKS, META_PAD, META_FONT, NAME_MIN, NAME_BASIS]) expect(Number.isFinite(n)).toBe(true);
    const em = (re: RegExp) => Number(re.exec(css)?.[1]);
    const sizeEm = em(/@container \(width >= (\d+(?:\.\d+)?)em\) \{\n {2}\.file-tree--columns \.file-tree__canvas/);
    const dateEm = em(/@container \(width >= (\d+(?:\.\d+)?)em\) \{\n {2}\.file-tree--columns \.file-row__col--date/);
    const permsEm = em(/@container \(\d+(?:\.\d+)?em <= width < (\d+(?:\.\d+)?)em\) \{ \.file-tree--columns \.file-row__meta/);

    for (const uiPx of [13, 16]) {
      const scale = uiPx / 13;
      const ch = MONO_PX * scale * CH;
      const metaCh = META_FONT * scale * CH;
      const chrome = 2 * TREE_PAD + ROW_PAD_RIGHT + SCROLLBAR;
      const lead = INDENT_BASE + INDENT_STEP * DEPTH + CHEVRON_SLOT + ICON + GAP;
      const marks = GAP + MARKS;
      const sizeCell = GAP + COL_MARGIN + sizeCh * ch;
      const dateCell = GAP + COL_MARGIN + dateCh * ch;
      const perms = GAP + META_PAD + PERMS_CH * metaCh;
      const needed = {
        size: chrome + lead + NAME_MIN * ch + marks + sizeCell,
        date: chrome + lead + NAME_MIN * ch + marks + sizeCell + dateCell,
        perms: chrome + lead + NAME_BASIS * ch + perms + marks + sizeCell + dateCell,
      };
      expect(needed.size, `size at ${uiPx}px`).toBeLessThanOrEqual(sizeEm * uiPx);
      expect(needed.date, `date at ${uiPx}px`).toBeLessThanOrEqual(dateEm * uiPx);
      expect(needed.perms, `permissions at ${uiPx}px`).toBeLessThanOrEqual(permsEm * uiPx);
    }
  });

  it("rows and header share the column widths", () => {
    expect(css).toContain(".file-row__col--size { width: var(--file-col-size); }");
    expect(css).toContain(".file-row__col--date { width: var(--file-col-date); }");
    expect(css).toMatch(/\.file-tree-header__cell--size \{[^}]*width: var\(--file-col-size\)/);
    expect(css).toMatch(/\.file-tree-header__cell--mtime \{[^}]*width: var\(--file-col-date\)/);
  });
});

describe("file tree columns: every formatted value fits its width", () => {
  const sizeCh = chWidth("--file-col-size");
  const dateCh = chWidth("--file-col-date");
  const now = new Date(2026, 9, 20, 15, 30, 0).getTime();

  const sizes: number[] = [0, 1, 9, 99, 999, 1000, 1023, 1024, 1536, 9_999, 10_239, 10_240, 999_999, 1_048_575];
  for (let unit = 1; unit <= 4; unit += 1) {
    for (const base of [1000, 1024]) {
      for (const n of [1, 9.96, 10, 99.6, 999, 999.6, 1000, 1023, 1023.9]) sizes.push(Math.floor(n * base ** unit));
    }
  }
  const ago = (s: number) => now - s * 1000;
  const MIN = 60;
  const HOUR = 3600;
  const DAY = 24 * HOUR;
  const times = [0, 10, 44, 45, 59, 90, MIN * 59, HOUR, HOUR * 23, DAY, DAY * 2, DAY * 6, DAY * 29, DAY * 31, DAY * 200, DAY * 330, DAY * 364, DAY * 366, DAY * 5 * 365, DAY * 99 * 365]
    .flatMap((s) => [ago(s), now + s * 1000]) // future: clock skew
    .concat([new Date(2026, 9, 20, 0, 0).getTime(), new Date(2026, 9, 20, 23, 59).getTime(), new Date(2025, 11, 31, 23, 59).getTime(), new Date(1999, 0, 1).getTime()]);

  afterAll(async () => {
    await setLanguage("en");
  });

  for (const language of ["en", "uk"]) {
    it(`${language}: sizes (binary and decimal) and dates (relative and absolute)`, async () => {
      await setLanguage(language);
      for (const system of ["binary", "decimal"] as const) {
        for (const size of sizes) {
          const text = formatBytes(size, system);
          expect(length(text), `${language} ${system} ${size}: "${text}"`).toBeLessThanOrEqual(sizeCh);
        }
      }
      for (const format of ["relative", "absolute"] as const) {
        for (const ms of times) {
          const text = formatFileDateShort(ms / 1000, now, format);
          expect(text.length > 0, `${language} ${format} ${ms}`).toBe(true);
          expect(length(text), `${language} ${format} ${new Date(ms).toISOString()}: "${text}"`).toBeLessThanOrEqual(dateCh);
        }
      }
    });
  }
});
