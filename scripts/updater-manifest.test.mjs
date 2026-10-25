// Unit tests of the updater manifest script (release workflow, docs/ARCHITECTURE.md §11.3). Synthetic minisign
// blobs only: no real key is created or used here.
import { Buffer } from "node:buffer";
import { describe, expect, it } from "vitest";
import {
  MANIFEST_FILES,
  MAX_NOTES_CHARS,
  assembleManifest,
  assertStableRelease,
  compareSemver,
  currentManifests,
  keyIdOfPubkey,
  manifestUpdates,
  notesFromChangelog,
  parseSignature,
  shouldWrite,
  truncateNotes,
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
    expect(Array.from(long.notes).length).toBeLessThanOrEqual(MAX_NOTES_CHARS);
    expect(long.notes.endsWith(`\n\n[Full release notes on GitHub](${PAGE})`)).toBe(true);
  });
});

const PAGE = "https://github.com/shellfin-app/shellfin/releases/tag/v0.2.0";
const LINK = `\n\n[Full release notes on GitHub](${PAGE})`;
const item = (n, words = 30) => `- Item ${n}: ${"word ".repeat(words).trim()}\n  continued line of item ${n}.`;
const len = (s) => Array.from(s).length;

describe("release notes bounded for the manifest", () => {
  it("leaves notes within the limit unchanged, CRLF included", () => {
    const notes = "### Added\r\n\r\n- One\r\n- Two";
    expect(truncateNotes(notes, 100, PAGE)).toBe(notes);
    const exact = "x".repeat(100);
    expect(truncateNotes(exact, 100, PAGE)).toBe(exact);
  });

  it("cuts long notes after a whole item, within the limit, and ends with the release page link", () => {
    const items = Array.from({ length: 40 }, (_, i) => item(i));
    const notes = `### Added\n\n${items.join("\n")}`;
    const max = 2000;
    const out = truncateNotes(notes, max, PAGE);
    expect(len(out)).toBeLessThanOrEqual(max);
    expect(out.endsWith(LINK)).toBe(true);
    const body = out.slice(0, -LINK.length);
    // A prefix of whole items: the last kept item ends with its continuation line.
    expect(notes.startsWith(body)).toBe(true);
    expect(body).toMatch(/continued line of item \d+\.$/);
    const kept = body.split("\n").filter((l) => l.startsWith("- ")).length;
    expect(kept).toBeGreaterThan(5);
    // The next item would not have fitted.
    expect(len(`${body}\n${items[kept]}${LINK}`)).toBeGreaterThan(max);
  });

  it("never leaves a heading without items at the end", () => {
    const notes = `### Added\n\n${item(1)}\n${item(2)}\n\n### Changed\n\n${item(3, 400)}\n\n### Fixed\n\n${item(4)}`;
    const out = truncateNotes(notes, 900, PAGE);
    expect(len(out)).toBeLessThanOrEqual(900);
    expect(out).toBe(`### Added\n\n${item(1)}\n${item(2)}${LINK}`);
    expect(out).not.toContain("### Changed");
  });

  it("cuts a single giant item hard and still links the release page", () => {
    const notes = `- ${"long ".repeat(5000)}`;
    const out = truncateNotes(notes, 8000, PAGE);
    expect(len(out)).toBeLessThanOrEqual(8000);
    expect(out.endsWith(`…${LINK}`)).toBe(true);
    expect(out.startsWith("- long long")).toBe(true);
    // A heading alone before the giant item is dropped as dangling, so the hard cut applies too.
    const headed = truncateNotes(`### Added\n\n${notes}`, 8000, PAGE);
    expect(headed.startsWith("### Added\n\n- long")).toBe(true);
    expect(headed.endsWith(`…${LINK}`)).toBe(true);
  });

  it("handles CRLF input: cuts at item boundaries and outputs LF", () => {
    const notes = `### Added\r\n\r\n${Array.from({ length: 40 }, (_, i) => item(i)).join("\r\n")}`;
    const out = truncateNotes(notes, 2000, PAGE);
    expect(len(out)).toBeLessThanOrEqual(2000);
    expect(out).not.toContain("\r");
    expect(out.endsWith(LINK)).toBe(true);
    expect(out.slice(0, -LINK.length)).toMatch(/continued line of item \d+\.$/);
  });

  it("returns CRLF notes that fit once converted to LF whole, without the link", () => {
    const lf = `### Added\n\n${item(1)}\n${item(2)}`;
    const crlf = lf.replace(/\n/g, "\r\n");
    const max = len(lf);
    expect(len(crlf)).toBeGreaterThan(max);
    expect(truncateNotes(crlf, max, PAGE)).toBe(lf);
  });

  it("counts code points, not UTF-16 units", () => {
    const notes = Array.from({ length: 200 }, (_, i) => `- 🐟 item ${i} 😀😀😀`).join("\n");
    const out = truncateNotes(notes, 1000, PAGE);
    expect(len(out)).toBeLessThanOrEqual(1000);
    expect(out.endsWith(LINK)).toBe(true);
    expect(out.slice(0, -LINK.length)).toMatch(/😀😀😀$/);
  });
});

describe("update manifests of a published release", () => {
  const manifest = (version) => ({ version, notes: "", pub_date: "2026-10-21T12:00:00Z", platforms: {} });

  it("accepts only stable releases", () => {
    expect(() => assertStableRelease("0.2.0", false)).not.toThrow();
    expect(() => assertStableRelease("0.2.0-experimental.1", false)).toThrow(/pre-release version/);
    expect(() => assertStableRelease("0.2.0-rc.1", false)).toThrow(/pre-release version/);
    // Marked as a pre-release on GitHub, even without a SemVer suffix.
    expect(() => assertStableRelease("0.2.0", true)).toThrow(/marked as a pre-release/);
    expect(() => assertStableRelease("latest", false)).toThrow(/SemVer/);
    // A pre-release writes nothing at all.
    expect(() => manifestUpdates(manifest("0.3.0-experimental.1"), false, {})).toThrow();
    expect(() => manifestUpdates(manifest("0.3.0"), true, {})).toThrow();
  });

  it("keeps experimental.json as the legacy mirror of stable.json", () => {
    expect(MANIFEST_FILES).toEqual(["stable.json", "experimental.json"]);
  });

  it("treats only unlisted manifests as missing; a listed one must have been downloaded intact", () => {
    const downloaded = { "stable.json": manifest("0.2.0"), "experimental.json": manifest("0.2.0") };
    const read = (name) => downloaded[name] ?? null;
    expect(currentManifests(["stable.json", "experimental.json", "other.txt"], read)).toEqual(downloaded);
    // No `updater` release yet (empty listing), or a release without the mirror: those files are missing.
    expect(currentManifests([], read)).toEqual({ "stable.json": null, "experimental.json": null });
    expect(currentManifests(["stable.json"], read)).toEqual({ "stable.json": manifest("0.2.0"), "experimental.json": null });
    // A listed file whose download failed, or that is not a manifest, fails instead of being overwritten.
    expect(() => currentManifests(["stable.json", "experimental.json"], (n) => (n === "stable.json" ? read(n) : null))).toThrow(
      /experimental\.json is listed/,
    );
    expect(() => currentManifests(["stable.json"], () => ({ version: "garbage" }))).toThrow(/stable\.json is listed/);
    expect(() => currentManifests(["stable.json"], () => ["not", "a", "manifest"])).toThrow(/stable\.json is listed/);
    // With a failed download treated as missing, republishing 0.1.0 would have overwritten 0.2.0; now it cannot.
    const current = currentManifests(["stable.json", "experimental.json"], read);
    expect(manifestUpdates(manifest("0.1.0"), false, current)).toEqual([]);
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

  it("a stable release updates both files; the old experimental build moves to it; nothing is downgraded", () => {
    const names = (updates) => updates.map((u) => u.name);
    // Today's state: experimental.json still holds the 0.2.0-experimental.1 pre-release; 0.2.0 replaces it.
    const current = { "stable.json": manifest("0.1.1"), "experimental.json": manifest("0.2.0-experimental.1") };
    expect(names(manifestUpdates(manifest("0.2.0"), false, current))).toEqual(["stable.json", "experimental.json"]);
    // A stable release older than what the legacy mirror holds: only stable.json moves.
    const ahead = { "stable.json": manifest("0.1.0"), "experimental.json": manifest("0.3.0-experimental.1") };
    expect(names(manifestUpdates(manifest("0.2.0"), false, ahead))).toEqual(["stable.json"]);
    // Republishing 0.1.0 later: nothing is written.
    const newer = { "stable.json": manifest("0.2.0"), "experimental.json": manifest("0.2.0") };
    expect(manifestUpdates(manifest("0.1.0"), false, newer)).toEqual([]);
    // First release ever, or a missing mirror: the files are created.
    expect(names(manifestUpdates(manifest("0.1.0"), false, {}))).toEqual(["stable.json", "experimental.json"]);
    const noMirror = { "stable.json": manifest("0.2.0"), "experimental.json": null };
    expect(names(manifestUpdates(manifest("0.2.1"), false, noMirror))).toEqual(["stable.json", "experimental.json"]);
    // Every written file carries the release's manifest unchanged.
    expect(manifestUpdates(manifest("0.2.0"), false, {}).every((u) => u.manifest.version === "0.2.0")).toBe(true);
  });
});
