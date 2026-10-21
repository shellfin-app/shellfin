// Bandwidth limit fields: the contract stores KiB/s (0 = unlimited); the UI lets the user pick KiB/s or MiB/s.
import type { LimitOverridesInfo, Settings } from "../ipc/types";

export const BANDWIDTH_MAX_KIBPS = 1048576;

export type BandwidthUnit = "KiB" | "MiB";

export interface BandwidthField {
  /** Digits as typed. */
  value: string;
  unit: BandwidthUnit;
}

/** 0 -> "0 KiB", 2048 -> "2 MiB", 1500 -> "1500 KiB" (MiB only when it is exact). */
export function fieldFromKiBps(kibps: number): BandwidthField {
  if (kibps > 0 && kibps % 1024 === 0) return { value: String(kibps / 1024), unit: "MiB" };
  return { value: String(kibps), unit: "KiB" };
}

/** The KiB/s the field means, or `null` when it is not a whole number in 0..1048576. */
export function kibpsFromField(field: BandwidthField): number | null {
  const text = field.value.trim();
  if (!/^\d+$/.test(text)) return null;
  const n = Number(text) * (field.unit === "MiB" ? 1024 : 1);
  return Number.isSafeInteger(n) && n <= BANDWIDTH_MAX_KIBPS ? n : null;
}

/**
 * The limits that apply to a connection: the override the backend captured when the session opened
 * (`SessionInfo.limitOverrides`) when it has one, else the live global setting. 0 = unlimited.
 */
export function effectiveLimits(
  settings: Pick<Settings, "transfers"> | null | undefined,
  overrides: LimitOverridesInfo | null | undefined,
): { upload: number; download: number } {
  return {
    upload: overrides?.uploadKibps ?? settings?.transfers.uploadLimitKiBps ?? 0,
    download: overrides?.downloadKibps ?? settings?.transfers.downloadLimitKiBps ?? 0,
  };
}

/** Unit choices of a bandwidth field (the labels are `settings.unitKiB` / `settings.unitMiB`). */
export const BANDWIDTH_UNITS: readonly BandwidthUnit[] = ["KiB", "MiB"];
