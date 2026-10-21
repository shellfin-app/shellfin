import { describe, expect, it } from "vitest";
import { effectiveLimits, fieldFromKiBps, kibpsFromField } from "./bandwidth";

describe("bandwidth fields", () => {
  it("picks MiB only for exact multiples", () => {
    expect(fieldFromKiBps(0)).toEqual({ value: "0", unit: "KiB" });
    expect(fieldFromKiBps(512)).toEqual({ value: "512", unit: "KiB" });
    expect(fieldFromKiBps(2048)).toEqual({ value: "2", unit: "MiB" });
    expect(fieldFromKiBps(1500)).toEqual({ value: "1500", unit: "KiB" });
  });

  it("converts back and rejects bad input", () => {
    expect(kibpsFromField({ value: "2", unit: "MiB" })).toBe(2048);
    expect(kibpsFromField({ value: " 300 ", unit: "KiB" })).toBe(300);
    expect(kibpsFromField({ value: "0", unit: "MiB" })).toBe(0);
    expect(kibpsFromField({ value: "1024", unit: "MiB" })).toBe(1048576);
    expect(kibpsFromField({ value: "1025", unit: "MiB" })).toBeNull();
    expect(kibpsFromField({ value: "", unit: "KiB" })).toBeNull();
    expect(kibpsFromField({ value: "1.5", unit: "KiB" })).toBeNull();
    expect(kibpsFromField({ value: "-1", unit: "KiB" })).toBeNull();
  });

  it("a bookmark override wins, including 0 = unlimited", () => {
    const settings = {
      transfers: {
        uploadLimitKiBps: 100,
        downloadLimitKiBps: 200,
        maxConcurrent: 3,
        onConflict: "ask" as const,
        preserveTimestamps: true,
        notifyOnComplete: false,
      },
    };
    expect(effectiveLimits(settings, null)).toEqual({ upload: 100, download: 200 });
    expect(effectiveLimits(settings, { uploadKibps: 0, downloadKibps: null })).toEqual({ upload: 0, download: 200 });
    expect(effectiveLimits(null, undefined)).toEqual({ upload: 0, download: 0 });
  });
});
