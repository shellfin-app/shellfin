// Unit tests of the updater manifest script (release workflow, docs/ARCHITECTURE.md §11.3). Synthetic minisign
// blobs only: no real key is created or used here.
import { Buffer } from "node:buffer";
import { describe, expect, it } from "vitest";
import {
  assembleManifest,
  channelUpdates,
  channelsFor,
  compareSemver,
  keyIdOfPubkey,
  notesFromChangelog,
  parseSignature,
  shouldWrite,
} from "./updater-manifest.mjs";

/** The release public key committed in src-tauri/tauri.conf.json (key id 559240A91983C276). */
const PUBKEY =
  "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDU1OTI0MEE5MTk4M0MyNzYKUldSMndvTVpxVUNTVmJsMmdrNkFEWWduTEd1L1ZEb0paSTBmL2lna3VuU2lhSWdGWjgxdWVkMEoK";
const KEY_ID = "559240A91983C276";

/** A structurally valid `.sig` content for `file` (zero signature bytes: only the layout is checked here). */
function fakeSig(file, version, keyId = KEY_ID) {
  const raw = Buffer.concat([Buffer.from("ED", "latin1"), Buffer.from(keyId, "hex").reverse(), Buffer.alloc(64)]);
  const text = [
    "untrusted comment: signature from tauri secret key",
    raw.toString("base64"),
    `trusted comment: timestamp:1760000000\tfile:${file}\tversion:${version}`,
    Buffer.alloc(64).toString("base64"),
    "",
  ].join("\n");
  return Buffer.from(text, "utf8").toString("base64");
}

const FILES = [
  "Shellfin_0.2.0_x64-setup.exe",
  "Shellfin_0.2.0_arm64-setup.exe",
  "Shellfin.app.tar.gz",
  "Shellfin_0.2.0_aarch64.dmg",
  "Shellfin_0.2.0_amd64.AppImage",
  "Shellfin_0.2.0_amd64.deb",
  "Shellfin-0.2.0-1.x86_64.rpm",
];

function input(overrides = {}) {
  const sigs = new Map(FILES.map((f) => [f, fakeSig(f, "0.2.0")]));
  return {
    files: FILES,
    readSig: (name) => sigs.get(name) ?? null,
    repo: "shellfin-app/shellfin",
    tag: "v0.2.0",
    pubkey: PUBKEY,
    notes: "### Added\n\n- Updates",
    pubDate: "2026-10-21T12:00:00Z",
    ...overrides,
  };
}

describe("SemVer precedence", () => {
  it("orders releases, pre-releases and numeric identifiers", () => {
    const ordered = [
      "0.1.0",
      "0.2.0-experimental.1",
      "0.2.0-experimental.2",
      "0.2.0-experimental.10",
      "0.2.0-rc.1",
      "0.2.0",
      "0.2.1",
      "0.10.0",
      "1.0.0-alpha",
      "1.0.0-alpha.1",
      "1.0.0-alpha.beta",
      "1.0.0",
    ];
    for (let i = 0; i + 1 < ordered.length; i++) {
      expect(compareSemver(ordered[i], ordered[i + 1]), `${ordered[i]} < ${ordered[i + 1]}`).toBe(-1);
      expect(compareSemver(ordered[i + 1], ordered[i])).toBe(1);
    }
    expect(compareSemver("v1.2.3", "1.2.3+build.7")).toBe(0);
    expect(() => compareSemver("1.2", "1.2.3")).toThrow();
    expect(() => compareSemver("01.2.3", "1.2.3")).toThrow();
  });
});

describe("minisign parsing", () => {
  it("reads the key id of the committed public key and of a signature", () => {
    expect(keyIdOfPubkey(PUBKEY)).toBe(KEY_ID);
    const sig = parseSignature(fakeSig("a.exe", "1.0.0"));
    expect(sig.keyId).toBe(KEY_ID);
    expect(sig.fields).toMatchObject({ file: "a.exe", version: "1.0.0" });
    expect(() => parseSignature(Buffer.from("hello").toString("base64"))).toThrow(/minisign/);
    expect(() => keyIdOfPubkey("bm9wZQ==")).toThrow();
  });
});

describe("assemble latest.json", () => {
  it("maps each platform to its signed artifact on the release's download URL", () => {
    const m = assembleManifest(input());
    expect(m.version).toBe("0.2.0");
    expect(m.pub_date).toBe("2026-10-21T12:00:00Z");
    expect(m.notes).toContain("- Updates");
    expect(Object.keys(m.platforms).sort()).toEqual([
      "darwin-aarch64",
      "linux-x86_64",
      "windows-aarch64",
      "windows-x86_64",
    ]);
    const base = "https://github.com/shellfin-app/shellfin/releases/download/v0.2.0/";
    expect(m.platforms["windows-x86_64"].url).toBe(`${base}Shellfin_0.2.0_x64-setup.exe`);
    expect(m.platforms["windows-aarch64"].url).toBe(`${base}Shellfin_0.2.0_arm64-setup.exe`);
    expect(m.platforms["darwin-aarch64"].url).toBe(`${base}Shellfin.app.tar.gz`);
    expect(m.platforms["linux-x86_64"].url).toBe(`${base}Shellfin_0.2.0_amd64.AppImage`);
    expect(m.platforms["linux-x86_64"].signature).toBe(fakeSig("Shellfin_0.2.0_amd64.AppImage", "0.2.0"));
  });

  it("ignores MSI files: only the NSIS installer is an updater artifact", () => {
    const m = assembleManifest(input({ files: [...FILES, "Shellfin_0.2.0_x64_en-US.msi"] }));
    expect(Object.keys(m.platforms)).not.toContain("windows-x86_64-msi");
    expect(m.platforms["windows-x86_64"].url).toMatch(/x64-setup\.exe$/);
  });

  it("refuses anything that would ship a broken or unverifiable update", () => {
    expect(() => assembleManifest(input({ files: FILES.filter((f) => !f.endsWith(".AppImage")) }))).toThrow(
      /no updater artifact for linux-x86_64/,
    );
    expect(() => assembleManifest(input({ readSig: () => null }))).toThrow(/missing signature/);
    expect(() =>
      assembleManifest(input({ readSig: (n) => fakeSig(n, "0.2.0", "0123456789ABCDEF") })),
    ).toThrow(/not the release key/);
    expect(() => assembleManifest(input({ readSig: (n) => fakeSig(n, "0.1.9") }))).toThrow(/signed for version 0.1.9/);
    expect(() => assembleManifest(input({ readSig: () => fakeSig("other.exe", "0.2.0") }))).toThrow(/made for file/);
    const spaced = FILES.map((f) => (f.endsWith("_x64-setup.exe") ? "Shell fin_0.2.0_x64-setup.exe" : f));
    expect(() => assembleManifest(input({ files: spaced }))).toThrow(/GitHub would rename/);
    expect(() => assembleManifest(input({ files: [...FILES, "Other_0.2.0_x64-setup.exe"] }))).toThrow(/more than one/);
    expect(() => assembleManifest(input({ tag: "0.2.0" }))).toThrow(/invalid tag/);
    expect(() => assembleManifest(input({ repo: "evil/repo/../x" }))).toThrow(/invalid repository/);
  });

  it("notes come from the CHANGELOG section of the version, bounded; otherwise a link", () => {
    const changelog = "# Changelog\n\n## [Unreleased]\n\n- next\n\n## [0.2.0] - 2026-10-21\n\n### Added\n\n- Updates\n\n## [0.1.0]\n\n- first\n";
    expect(notesFromChangelog(changelog, "0.2.0")).toBe("### Added\n\n- Updates");
    expect(notesFromChangelog(changelog, "0.3.0")).toBeNull();
    const m = assembleManifest(input({ notes: null }));
    expect(m.notes).toBe("Shellfin 0.2.0. Release notes: https://github.com/shellfin-app/shellfin/releases/tag/v0.2.0");
    const long = assembleManifest(input({ notes: "x".repeat(20_000) }));
    expect(Array.from(long.notes).length).toBe(8000);
  });
});

describe("channel files of a published release", () => {
  const manifest = (version) => ({ version, notes: "", pub_date: "2026-10-21T12:00:00Z", platforms: {} });

  it("stable releases write both channels; pre-releases only experimental", () => {
    expect(channelsFor("0.2.0", false)).toEqual(["stable", "experimental"]);
    expect(channelsFor("0.2.0-experimental.1", false)).toEqual(["experimental"]);
    expect(channelsFor("0.2.0-rc.1", false)).toEqual(["experimental"]);
    // Marked as a pre-release on GitHub, even without a SemVer suffix.
    expect(channelsFor("0.2.0", true)).toEqual(["experimental"]);
    expect(() => channelsFor("latest", false)).toThrow();
  });

  it("never regresses a channel: only strictly newer versions are written", () => {
    expect(shouldWrite("0.2.0", null)).toBe(true);
    expect(shouldWrite("0.2.0", { version: "garbage" })).toBe(true);
    expect(shouldWrite("0.2.0", manifest("0.1.0"))).toBe(true);
    expect(shouldWrite("0.2.0", manifest("0.2.0"))).toBe(false);
    expect(shouldWrite("0.1.5", manifest("0.2.0"))).toBe(false);
    expect(shouldWrite("0.2.0", manifest("0.2.0-experimental.3"))).toBe(true);
    expect(shouldWrite("0.2.0-experimental.3", manifest("0.2.0"))).toBe(false);
  });

  it("stable after an experimental build updates both; an old stable republished changes nothing", () => {
    const current = { stable: manifest("0.1.0"), experimental: manifest("0.2.0-experimental.2") };
    expect(channelUpdates(manifest("0.2.0"), false, current).map((u) => u.name)).toEqual(["stable.json", "experimental.json"]);
    // A newer experimental build than stable: experimental only.
    expect(channelUpdates(manifest("0.3.0-experimental.1"), true, current).map((u) => u.name)).toEqual(["experimental.json"]);
    // A stable release older than the experimental build: stable moves, experimental keeps the newer build.
    const ahead = { stable: manifest("0.1.0"), experimental: manifest("0.3.0-experimental.1") };
    expect(channelUpdates(manifest("0.2.0"), false, ahead).map((u) => u.name)).toEqual(["stable.json"]);
    // Republishing 0.1.0 later: nothing is written.
    expect(channelUpdates(manifest("0.1.0"), false, { stable: manifest("0.2.0"), experimental: manifest("0.2.0") })).toEqual([]);
    // First release ever: both files are created.
    expect(channelUpdates(manifest("0.1.0"), false, {}).map((u) => u.name)).toEqual(["stable.json", "experimental.json"]);
  });
});
