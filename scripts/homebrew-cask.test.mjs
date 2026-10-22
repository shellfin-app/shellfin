// Unit tests of the Homebrew cask renderer (scripts/homebrew-cask.mjs, docs/ARCHITECTURE.md §11.3).
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  CASK_FILE,
  CASK_TOKEN,
  caskVersion,
  isNewer,
  macosRequirement,
  renderCask,
  sha256FromSums,
  tapReadme,
} from "./homebrew-cask.mjs";

const HASH = "a".repeat(64);
const base = { repo: "shellfin-app/shellfin", version: "0.2.0", sha256: HASH, macos: ":big_sur", notarized: false };

describe("macosRequirement", () => {
  it("maps the bundle minimum to a Homebrew symbol", () => {
    expect(macosRequirement("11.0")).toBe(":big_sur");
    expect(macosRequirement("14")).toBe(":sonoma");
    expect(() => macosRequirement("10.15")).toThrow(/no Homebrew macOS symbol/);
    expect(() => macosRequirement(undefined)).toThrow();
  });

  it("matches tauri.conf.json", () => {
    const dir = dirname(fileURLToPath(import.meta.url));
    const conf = JSON.parse(readFileSync(join(dir, "..", "src-tauri", "tauri.conf.json"), "utf8"));
    expect(macosRequirement(conf.bundle.macOS.minimumSystemVersion)).toMatch(/^:[a-z_]+$/);
  });
});

describe("sha256FromSums", () => {
  const sums = `${HASH}  Shellfin_0.2.0_aarch64.dmg\n${"b".repeat(64)} *other.exe\n`;
  it("finds exactly one entry", () => {
    expect(sha256FromSums(sums, "Shellfin_0.2.0_aarch64.dmg")).toBe(HASH);
    expect(sha256FromSums(sums, "other.exe")).toBe("b".repeat(64));
  });
  it("refuses a missing, duplicate or malformed entry", () => {
    expect(() => sha256FromSums(sums, "missing.dmg")).toThrow(/exactly once/);
    expect(() => sha256FromSums(`${sums}${sums}`, "other.exe")).toThrow(/exactly once/);
    expect(() => sha256FromSums("XYZ  Shellfin_0.2.0_aarch64.dmg", "Shellfin_0.2.0_aarch64.dmg")).toThrow();
  });
});

describe("renderCask", () => {
  it("renders the stable cask", () => {
    const text = renderCask(base);
    expect(text.startsWith('cask "shellfin" do\n')).toBe(true);
    expect(text).toContain('  version "0.2.0"');
    expect(text).toContain(`  sha256 "${HASH}"`);
    expect(text).toContain(
      '  url "https://github.com/shellfin-app/shellfin/releases/download/v#{version}/Shellfin_#{version}_aarch64.dmg"',
    );
    expect(text).toContain("  auto_updates true");
    expect(text).not.toContain("conflicts_with");
    expect(text).not.toContain("experimental");
    expect(text).toContain("  depends_on arch: :arm64");
    expect(text).toContain("  depends_on macos: :big_sur");
    expect(text).toContain('  app "Shellfin.app"');
    expect(text).toContain('  homepage "https://shellfin.app/"');
    expect(text).toContain("  livecheck do\n    skip ");
    expect(text).toContain('"~/Library/Application Support/app.shellfin.desktop",');
    expect(text).toContain('"~/Library/Logs/app.shellfin.desktop",');
    expect(text).toContain('choose "Open Anyway"');
    expect(text).not.toContain("no-quarantine");
    expect(text.endsWith("end\n")).toBe(true);
  });

  it("refuses a pre-release version", () => {
    expect(() => renderCask({ ...base, version: "0.3.0-experimental.1" })).toThrow(/pre-release/);
    expect(() => renderCask({ ...base, version: "0.3.0-rc.1" })).toThrow(/pre-release/);
  });

  it("drops the caveats once the app is notarized", () => {
    expect(renderCask({ ...base, notarized: true })).not.toContain("caveats");
  });

  it("keeps Homebrew's stanza order", () => {
    const text = renderCask(base);
    const order = ["version ", "sha256 ", "url ", "name ", "desc ", "homepage ", "livecheck ", "auto_updates ", "depends_on arch", "depends_on macos", "app ", "zap ", "caveats "];
    const at = order.map((s) => text.indexOf(`\n  ${s}`));
    expect(at.every((i) => i > 0)).toBe(true);
    expect([...at].sort((x, y) => x - y)).toEqual(at);
    expect(text.split("\n").every((l) => l === l.trimEnd())).toBe(true);
  });

  it("refuses bad input", () => {
    expect(() => renderCask({ ...base, sha256: "abc" })).toThrow(/sha256/);
    expect(() => renderCask({ ...base, version: "1.2" })).toThrow(/SemVer/);
    expect(() => renderCask({ ...base, repo: "evil/x/../y" })).toThrow(/repository/);
  });
});

describe("one cask / never downgrade", () => {
  it("has a single cask", () => {
    expect(CASK_TOKEN).toBe("shellfin");
    expect(CASK_FILE).toBe("shellfin.rb");
  });

  it("writes only a strictly newer version", () => {
    const old = renderCask(base);
    expect(caskVersion(old)).toBe("0.2.0");
    expect(isNewer("0.2.1", old)).toBe(true);
    expect(isNewer("0.2.0", old)).toBe(false);
    expect(isNewer("0.1.9", old)).toBe(false);
    expect(isNewer("0.2.0", 'cask "shellfin" do\n  version "0.2.0-experimental.2"\nend\n')).toBe(true);
    expect(isNewer("0.1.0", "")).toBe(true);
  });
});

describe("tapReadme", () => {
  it("documents the one install command", () => {
    const readme = tapReadme();
    expect(readme).toContain("brew install --cask shellfin-app/tap/shellfin\n");
    expect(readme).not.toContain("experimental");
  });
});
