// Locale-aware formatting — the ONLY place numbers, sizes, dates and lists are formatted (§8.2.2).
import i18next from "i18next";
import { useStore } from "../state/store";

function lang(): string {
  // en-XA formats like en.
  const l = i18next.language || "en";
  return l === "en-XA" ? "en" : l;
}

/**
 * `Intl` formatters are costly to build and the file tree formats every visible row on each scroll frame: one
 * instance per kind + language + options, dropped when the language changes.
 */
const formatters = new Map<string, unknown>();
let formattersLang = "";

function cached<T>(kind: string, options: object | undefined, make: (language: string) => T): T {
  const language = lang();
  if (language !== formattersLang) {
    formatters.clear();
    formattersLang = language;
  }
  const key = `${kind}|${options === undefined ? "" : JSON.stringify(options)}`;
  let formatter = formatters.get(key) as T | undefined;
  if (formatter === undefined) {
    formatter = make(language);
    formatters.set(key, formatter);
  }
  return formatter;
}

export function formatNumber(n: number, options?: Intl.NumberFormatOptions): string {
  return cached("number", options, (l) => new Intl.NumberFormat(l, options)).format(n);
}

const BINARY_UNITS = ["B", "KiB", "MiB", "GiB", "TiB"] as const;
const DECIMAL_UNITS = ["B", "KB", "MB", "GB", "TB"] as const;

/** `files.sizeUnits` (binary until the settings are loaded). */
function sizeUnits(): "binary" | "decimal" {
  return useStore.getState().settings?.files.sizeUnits ?? "binary";
}

/**
 * 1536 -> "1.5 KiB" (binary, multiples of 1024) or "1.5 KB" (decimal, multiples of 1000), following the setting
 * `files.sizeUnits`. Decimal digits only when < 10.
 */
export function formatBytes(bytes: number, system: "binary" | "decimal" = sizeUnits()): string {
  if (!Number.isFinite(bytes) || bytes < 0) return `${formatNumber(0)} ${i18next.t("units.B")}`;
  const binary = system === "binary";
  const units = binary ? BINARY_UNITS : DECIMAL_UNITS;
  const step = binary ? 1024 : 1000;
  let value = bytes;
  let unit = 0;
  while (value >= step && unit < units.length - 1) {
    value /= step;
    unit += 1;
  }
  const digits = unit === 0 || value >= 10 ? 0 : 1;
  return `${formatNumber(value, { maximumFractionDigits: digits })} ${i18next.t(`units.${units[unit] ?? "B"}`)}`;
}

export function formatRate(bytesPerSec: number): string {
  return i18next.t("units.perSecond", { size: formatBytes(bytesPerSec) });
}

const RELATIVE_STEPS: [Intl.RelativeTimeFormatUnit, number][] = [
  ["year", 365 * 24 * 3600],
  ["month", 30 * 24 * 3600],
  ["day", 24 * 3600],
  ["hour", 3600],
  ["minute", 60],
  ["second", 1],
];

function relativeFormat(numeric: "auto" | "always", style: Intl.RelativeTimeFormatStyle): Intl.RelativeTimeFormat {
  const options = { numeric, style };
  return cached("relative", options, (l) => new Intl.RelativeTimeFormat(l, options));
}

/**
 * "3 hours ago" (`long`, the default) or the compact "3h ago" (`narrow`, the file tree's date column). The narrow
 * form keeps words only for "now" and "yesterday"/"tomorrow"; other units stay numeric ("1 mo. ago", not "last mo."),
 * so the column reads the same way down the list (uk: «1 міс. тому», «1 р. тому», like «2 р. тому»).
 */
export function formatRelativeTime(
  timestampMs: number,
  nowMs: number = Date.now(),
  style: Intl.RelativeTimeFormatStyle = "long",
): string {
  const diffSec = Math.round((timestampMs - nowMs) / 1000);
  if (Math.abs(diffSec) < 45) return relativeFormat("auto", style).format(0, "second"); // "now"
  for (const [unit, secs] of RELATIVE_STEPS) {
    if (Math.abs(diffSec) >= secs) {
      const value = Math.round(diffSec / secs);
      const numeric = style === "long" || (unit === "day" && Math.abs(value) === 1) ? "auto" : "always";
      return relativeFormat(numeric, style).format(value, unit);
    }
  }
  return relativeFormat("auto", style).format(0, "second");
}

export function formatDateTime(timestampMs: number, options?: Intl.DateTimeFormatOptions): string {
  const resolved = options ?? { dateStyle: "medium", timeStyle: "short" };
  return cached("date", resolved, (l) => new Intl.DateTimeFormat(l, resolved)).format(timestampMs);
}

/** Server mtimes can be anything: beyond the range of a JS date (±8.64e15 ms) or not a number, nothing is shown. */
function validMs(mtimeSec: number): number | null {
  const ms = mtimeSec * 1000;
  return Number.isFinite(ms) && Math.abs(ms) <= 8.64e15 ? ms : null;
}

/**
 * A file's modification time (Unix seconds) following `files.dateFormat`: relative ("3 hours ago") or absolute.
 * An out-of-range value gives an empty string.
 */
export function formatFileDate(
  mtimeSec: number,
  nowMs: number = Date.now(),
  format: "relative" | "absolute" = useStore.getState().settings?.files.dateFormat ?? "relative",
): string {
  const ms = validMs(mtimeSec);
  if (ms === null) return "";
  if (format === "absolute") return formatDateTime(ms);
  return formatRelativeTime(ms, nowMs);
}

function sameLocalDay(a: number, b: number): boolean {
  const x = new Date(a);
  const y = new Date(b);
  return x.getFullYear() === y.getFullYear() && x.getMonth() === y.getMonth() && x.getDate() === y.getDate();
}

/**
 * The most characters (code points) a compact date may have: the width of the file tree's date column
 * (`--file-col-date` in files.css; `columns.test.ts` keeps both equal and checks every shipped language).
 */
export const COMPACT_DATE_MAX_CHARS = 14;

/**
 * The compact form of `formatFileDate` for the file tree's date column (fixed width, §8.11): relative ->
 * "3h ago" (the narrow style; the short one where narrow uses a sign, see `narrowUsesSign`); absolute -> the time
 * for today ("14:05"), else the short date ("12.10.25"). A relative text longer than `COMPACT_DATE_MAX_CHARS`
 * (es "dentro de 10 min", a future time from clock skew) gives the absolute form instead, so it is never clipped.
 * Never throws on odd server values (0, negative, far future): out-of-range dates give an empty string.
 */
export function formatFileDateShort(
  mtimeSec: number,
  nowMs: number = Date.now(),
  format: "relative" | "absolute" = useStore.getState().settings?.files.dateFormat ?? "relative",
): string {
  const ms = validMs(mtimeSec);
  if (ms === null) return "";
  if (format === "relative") {
    const relative = formatRelativeTime(ms, nowMs, narrowUsesSign() ? "short" : "narrow");
    if ([...relative].length <= COMPACT_DATE_MAX_CHARS) return relative;
  }
  return sameLocalDay(ms, nowMs) ? formatDateTime(ms, { timeStyle: "short" }) : formatDateTime(ms, { dateStyle: "short" });
}

/**
 * Whether the narrow relative style of the UI language writes the direction as a sign instead of words (CLDR fr:
 * "-5 min", "+3 h"), which reads like a negative duration in a date column. Such languages get the short style
 * ("il y a 5 min") there. Probed once per language with `formatToParts`, so no language list has to be kept.
 */
function narrowUsesSign(): boolean {
  return cached("narrowSign", undefined, (l) =>
    new Intl.RelativeTimeFormat(l, { numeric: "always", style: "narrow" })
      .formatToParts(-5, "minute")
      .some((part) => part.type === "literal" && /[-+−]/.test(part.value)),
  );
}

export function formatList(items: readonly string[], type: Intl.ListFormatType = "conjunction"): string {
  const options = { style: "long", type } as const;
  return cached("list", options, (l) => new Intl.ListFormat(l, options)).format(items);
}
