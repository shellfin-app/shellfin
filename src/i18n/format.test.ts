import { describe, expect, it, vi } from "vitest";
import i18next from "i18next";
import { makeSettings } from "../ipc/mock/fixtures";
import { resetStore, useStore } from "../state/store";
import { formatBytes, formatDateTime, formatFileDate, formatFileDateShort, formatList, formatNumber, formatRate, formatRelativeTime } from "./format";
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

describe("file tree date column (formatFileDateShort)", () => {
  const now = new Date(2026, 9, 20, 15, 30, 0).getTime(); // local time: "same day" is a local-calendar check
  const sec = (ms: number) => ms / 1000;

  it("relative: the narrow style, also for future dates (clock skew)", () => {
    expect(formatFileDateShort(sec(now - 3 * 3600_000), now, "relative")).toBe(formatRelativeTime(now - 3 * 3600_000, now, "narrow"));
    expect(formatFileDateShort(sec(now - 3 * 3600_000), now, "relative")).not.toBe("3 hours ago");
    expect(formatFileDateShort(sec(now - 24 * 3600_000), now, "relative")).toBe("yesterday");
    expect(formatFileDateShort(sec(now + 5 * 60_000), now, "relative")).toBe(new Intl.RelativeTimeFormat("en", { numeric: "auto", style: "narrow" }).format(5, "minute"));
  });

  it("relative narrow stays numeric for months and years (no 'last mo.' / «торік»), words only for now and yesterday", async () => {
    const narrow = (l: string, numeric: "auto" | "always", v: number, u: Intl.RelativeTimeFormatUnit) =>
      new Intl.RelativeTimeFormat(l, { numeric, style: "narrow" }).format(v, u);
    const monthAgo = now - 31 * 24 * 3600_000;
    const yearAgo = now - 366 * 24 * 3600_000;
    const twoDaysAgo = now - 2 * 24 * 3600_000;
    expect(formatRelativeTime(monthAgo, now, "narrow")).toBe(narrow("en", "always", -1, "month"));
    expect(formatRelativeTime(yearAgo, now, "narrow")).toBe(narrow("en", "always", -1, "year"));
    // the long tooltip form keeps the words
    expect(formatRelativeTime(yearAgo, now)).toBe(new Intl.RelativeTimeFormat("en", { numeric: "auto" }).format(-1, "year"));
    await setLanguage("uk");
    try {
      expect(formatRelativeTime(monthAgo, now, "narrow")).toBe(narrow("uk", "always", -1, "month"));
      expect(formatRelativeTime(yearAgo, now, "narrow")).toBe(narrow("uk", "always", -1, "year"));
      expect(formatRelativeTime(twoDaysAgo, now, "narrow")).toBe(narrow("uk", "always", -2, "day")); // not «позавчора»
      expect(formatRelativeTime(now - 24 * 3600_000, now, "narrow")).toBe(narrow("uk", "auto", -1, "day")); // «учора»
    } finally {
      await setLanguage("en");
    }
  });

  it("reuses formatter instances per language and options", () => {
    const spy = vi.spyOn(Intl, "NumberFormat");
    formatNumber(1.5, { maximumFractionDigits: 7 });
    formatNumber(2.5, { maximumFractionDigits: 7 });
    formatNumber(3.5, { maximumFractionDigits: 7 });
    expect(spy.mock.calls.filter((c) => (c[1] as Intl.NumberFormatOptions | undefined)?.maximumFractionDigits === 7)).toHaveLength(1);
    spy.mockRestore();
  });

  it("absolute: the time for today, the short date otherwise", () => {
    const today = new Date(2026, 9, 20, 9, 5).getTime();
    const earlier = new Date(2026, 9, 12, 23, 59).getTime();
    expect(formatFileDateShort(sec(today), now, "absolute")).toBe(new Intl.DateTimeFormat("en", { timeStyle: "short" }).format(today));
    expect(formatFileDateShort(sec(earlier), now, "absolute")).toBe(new Intl.DateTimeFormat("en", { dateStyle: "short" }).format(earlier));
  });

  it("follows files.dateFormat when no format is passed", () => {
    useStore.getState().setSettings({ ...makeSettings(), files: { ...makeSettings().files, dateFormat: "absolute" } });
    try {
      const earlier = new Date(2026, 0, 2, 10, 0).getTime();
      expect(formatFileDateShort(sec(earlier), now)).toBe(new Intl.DateTimeFormat("en", { dateStyle: "short" }).format(earlier));
    } finally {
      resetStore();
    }
  });

  it("never throws on odd server values (0, negative, huge)", () => {
    for (const value of [0, -1, -2_000_000_000, 4_102_444_800, 1e15, Number.MAX_SAFE_INTEGER, Number.NaN]) {
      for (const format of ["relative", "absolute"] as const) {
        expect(() => formatFileDateShort(value, now, format)).not.toThrow();
      }
    }
    expect(formatFileDateShort(Number.MAX_SAFE_INTEGER, now, "absolute")).toBe("");
    // the long (tooltip) form is guarded the same way
    for (const value of [1e15, Number.MAX_SAFE_INTEGER, Number.NaN, -1e15]) {
      expect(formatFileDate(value, now, "absolute")).toBe("");
      expect(formatFileDate(value, now, "relative")).toBe("");
    }
    expect(formatFileDate(0, now, "absolute")).not.toBe("");
  });
});
