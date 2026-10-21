import { describe, expect, it } from "vitest";
import i18next from "i18next";
import { makeSettings } from "../ipc/mock/fixtures";
import { resetStore, useStore } from "../state/store";
import { formatBytes, formatDateTime, formatFileDate, formatList, formatNumber, formatRate, formatRelativeTime } from "./format";
import { setLanguage } from "./index";

describe("format (Intl-based)", () => {
  it("formats bytes and rates", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(1229)).toBe("1.2 KiB");
    expect(formatBytes(18432)).toBe("18 KiB");
    expect(formatRate(1536)).toBe("1.5 KiB/s");
  });
  it("counts in steps of 1000 with decimal units (files.sizeUnits)", () => {
    useStore.getState().setSettings({ ...makeSettings(), files: { ...makeSettings().files, sizeUnits: "decimal" } });
    try {
      expect(formatBytes(1500)).toBe("1.5 KB");
      expect(formatBytes(18000)).toBe("18 KB");
      expect(formatBytes(5_000_000)).toBe("5 MB");
      expect(formatRate(1536)).toBe("1.5 KB/s");
    } finally {
      resetStore();
    }
  });
  it("formats a file date relative or absolute (files.dateFormat)", () => {
    const now = Date.UTC(2026, 9, 20, 12, 0, 0);
    const mtimeSec = (now - 2 * 3600_000) / 1000;
    expect(formatFileDate(mtimeSec, now)).toBe("2 hours ago");
    useStore.getState().setSettings({ ...makeSettings(), files: { ...makeSettings().files, dateFormat: "absolute" } });
    try {
      expect(formatFileDate(mtimeSec, now)).toBe(formatDateTime(mtimeSec * 1000));
      expect(formatFileDate(mtimeSec, now)).not.toContain("ago");
    } finally {
      resetStore();
    }
  });
  it("formats relative time", () => {
    const now = Date.UTC(2026, 9, 20, 12, 0, 0);
    expect(formatRelativeTime(now - 2 * 3600_000, now)).toBe("2 hours ago");
    expect(formatRelativeTime(now - 24 * 3600_000, now)).toBe("yesterday");
    expect(formatRelativeTime(now - 5_000, now)).toBe("now");
  });
  it("formats lists", () => {
    expect(formatList(["id_ed25519", "work_rsa"])).toBe("id_ed25519 and work_rsa");
  });
  it("follows the active language (Intl) and restores English", async () => {
    await setLanguage("uk");
    try {
      expect(i18next.language).toBe("uk");
      expect(formatNumber(1234.5)).toBe(new Intl.NumberFormat("uk").format(1234.5));
      expect(formatNumber(1234.5)).not.toBe(new Intl.NumberFormat("en").format(1234.5));
      const now = Date.UTC(2026, 9, 20, 12, 0, 0);
      expect(formatRelativeTime(now - 2 * 3600_000, now)).toBe(new Intl.RelativeTimeFormat("uk", { numeric: "auto" }).format(-2, "hour"));
    } finally {
      await setLanguage("en");
    }
    expect(formatNumber(1234.5)).toBe("1,234.5");
  });
  it("falls back to English for a language without a bundle", async () => {
    await setLanguage("fr");
    expect(i18next.language).toBe("en");
  });
});
