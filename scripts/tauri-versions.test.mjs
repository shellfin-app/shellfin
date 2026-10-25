// Unit tests of the npm/crate Tauri version check (docs/ARCHITECTURE.md §11.3).
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { cargoLockVersions, tauriMismatches } from "./tauri-versions.mjs";

const cargo = (pkgs) => pkgs.map(([n, v]) => `[[package]]\nname = "${n}"\nversion = "${v}"\n`).join("\n");
const npm = (pkgs) => ({
  packages: Object.fromEntries(pkgs.map(([n, v, dev]) => [`node_modules/@tauri-apps/${n}`, { version: v, dev }])),
});

describe("tauriMismatches", () => {
  it("accepts the same major.minor for api and plugins", () => {
    const c = cargo([["tauri", "2.12.4"], ["tauri-plugin-dialog", "2.8.3"]]);
    expect(tauriMismatches(npm([["api", "2.12.0"], ["plugin-dialog", "2.8.9"]]), c)).toEqual([]);
  });

  it("reports a minor mismatch and a missing crate", () => {
    const c = cargo([["tauri", "2.11.0"]]);
    expect(tauriMismatches(npm([["api", "2.12.4"], ["plugin-fs", "2.7.1"]]), c)).toEqual([
      "@tauri-apps/api 2.12.4 and tauri 2.11.0 differ in major.minor",
      "@tauri-apps/plugin-fs 2.7.1: expected exactly one tauri-plugin-fs in Cargo.lock, found 0",
    ]);
  });

  it("refuses two versions of a crate and a lockfile without @tauri-apps/api", () => {
    expect(tauriMismatches(npm([["api", "2.12.4"]]), cargo([["tauri", "2.12.4"], ["tauri", "2.12.1"]]))[0]).toMatch(/found 2/);
    expect(tauriMismatches(npm([]), cargo([["tauri", "2.12.4"]]))).toEqual(["package-lock.json has no @tauri-apps/api"]);
  });

  it("ignores the CLI and dev-only packages", () => {
    const c = cargo([["tauri", "2.12.4"]]);
    expect(tauriMismatches(npm([["api", "2.12.4"], ["cli", "2.0.0"], ["plugin-x", "1.0.0", true]]), c)).toEqual([]);
  });

  it("passes on the repository lockfiles", () => {
    const lock = JSON.parse(readFileSync("package-lock.json", "utf8"));
    const text = readFileSync("src-tauri/Cargo.lock", "utf8");
    expect(cargoLockVersions(text).get("tauri")).toHaveLength(1);
    expect(tauriMismatches(lock, text)).toEqual([]);
  });
});
