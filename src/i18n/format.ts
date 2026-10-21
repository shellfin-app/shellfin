// Locale-aware formatting — the ONLY place numbers, sizes, dates and lists are formatted (§8.2.2).
import i18next from "i18next";
import { useStore } from "../state/store";

function lang(): string {
  // en-XA formats like en.
  const l = i18next.language || "en";
  return l === "en-XA" ? "en" : l;
}

export function formatNumber(n: number, options?: Intl.NumberFormatOptions): string {
  return new Intl.NumberFormat(lang(), options).format(n);
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

export function formatRelativeTime(timestampMs: number, nowMs: number = Date.now()): string {
  const rtf = new Intl.RelativeTimeFormat(lang(), { numeric: "auto" });
  const diffSec = Math.round((timestampMs - nowMs) / 1000);
  if (Math.abs(diffSec) < 45) return rtf.format(0, "second"); // "now"
  for (const [unit, secs] of RELATIVE_STEPS) {
    if (Math.abs(diffSec) >= secs) return rtf.format(Math.round(diffSec / secs), unit);
  }
  return rtf.format(0, "second");
}

export function formatDateTime(timestampMs: number, options?: Intl.DateTimeFormatOptions): string {
  return new Intl.DateTimeFormat(lang(), options ?? { dateStyle: "medium", timeStyle: "short" }).format(timestampMs);
}

/** A file's modification time (Unix seconds) following `files.dateFormat`: relative ("3 hours ago") or absolute. */
export function formatFileDate(
  mtimeSec: number,
  nowMs: number = Date.now(),
  format: "relative" | "absolute" = useStore.getState().settings?.files.dateFormat ?? "relative",
): string {
  const ms = mtimeSec * 1000;
  if (format === "absolute") return formatDateTime(ms);
  return formatRelativeTime(ms, nowMs);
}

export function formatList(items: readonly string[], type: Intl.ListFormatType = "conjunction"): string {
  return new Intl.ListFormat(lang(), { style: "long", type }).format(items);
}
