// Unit tests of the frontend manifest script (release workflow, docs/ARCHITECTURE.md §11.3): the signing jobs trust
// dist/ only when every file matches, so tampering, missing and extra files must all fail.
import { Buffer } from "node:buffer";
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  checkPath,
  compareTrees,
  formatManifest,
  hashTree,
  parseArgs,
  parseManifest,
  sha256Hex,
  verifyManifest,
  writeManifest,
} from "./dist-manifest.mjs";

const A = "a".repeat(64);
const B = "b".repeat(64);

let root;
let dist;
let manifest;

beforeEach(() => {
  root = mkdtempSync(join(tmpdir(), "dist-manifest-"));
  dist = join(root, "dist");
  manifest = join(root, "dist.sha256");
  mkdirSync(join(dist, "assets"), { recursive: true });
  writeFileSync(join(dist, "index.html"), "<!doctype html>\n");
  writeFileSync(join(dist, "assets", "index-abc.js"), "console.log(1);\n");
  writeFileSync(join(dist, ".hidden"), "x");
});

afterEach(() => {
  rmSync(root, { recursive: true, force: true });
});

describe("write and verify", () => {
  it("round-trips and binds the manifest hash", () => {
    const sha = writeManifest(dist, manifest);
    expect(sha).toMatch(/^[0-9a-f]{64}$/);
    expect(verifyManifest(dist, manifest, sha)).toBe(3);
    expect(verifyManifest(dist, manifest)).toBe(3);
  });

  it("writes the sha256sum format, sorted, with hidden files", () => {
    writeManifest(dist, manifest);
    const entries = hashTree(dist);
    expect(entries.map((e) => e.path)).toEqual([".hidden", "assets/index-abc.js", "index.html"]);
    expect(entries[2].sha256).toBe(sha256Hex(Buffer.from("<!doctype html>\n")));
  });

  it("fails on a changed file", () => {
    const sha = writeManifest(dist, manifest);
    writeFileSync(join(dist, "assets", "index-abc.js"), "console.log(2);\n");
    expect(() => verifyManifest(dist, manifest, sha)).toThrow(/changed: assets\/index-abc\.js/);
  });

  it("fails on a missing file", () => {
    const sha = writeManifest(dist, manifest);
    rmSync(join(dist, "index.html"));
    expect(() => verifyManifest(dist, manifest, sha)).toThrow(/missing: index\.html/);
  });

  it("fails on an extra file, also in a new directory", () => {
    const sha = writeManifest(dist, manifest);
    mkdirSync(join(dist, "evil"));
    writeFileSync(join(dist, "evil", "inject.js"), "alert(1)");
    expect(() => verifyManifest(dist, manifest, sha)).toThrow(/extra: evil\/inject\.js/);
  });

  it("fails when the manifest was replaced together with the files", () => {
    const sha = writeManifest(dist, manifest);
    writeFileSync(join(dist, "index.html"), "<script>evil()</script>");
    writeManifest(dist, manifest);
    expect(() => verifyManifest(dist, manifest, sha)).toThrow(/manifest's SHA-256 differs/);
  });

  it("refuses a malformed expected hash", () => {
    writeManifest(dist, manifest);
    expect(() => verifyManifest(dist, manifest, "")).toThrow(/not a lowercase SHA-256/);
    expect(() => verifyManifest(dist, manifest, A.toUpperCase())).toThrow(/not a lowercase SHA-256/);
  });

  it("refuses an empty directory", () => {
    const empty = join(root, "empty");
    mkdirSync(empty);
    expect(() => writeManifest(empty, manifest)).toThrow(/no files/);
  });

  it("refuses symlinks", (ctx) => {
    try {
      symlinkSync(join(dist, "index.html"), join(dist, "link.html"));
    } catch {
      ctx.skip(); // Windows without the symlink privilege
    }
    expect(() => hashTree(dist)).toThrow(/not a regular file/);
  });
});

describe("parseManifest", () => {
  it("accepts what formatManifest writes", () => {
    const text = formatManifest([
      { path: "b.js", sha256: B },
      { path: "a.js", sha256: A },
    ]);
    expect(text).toBe(`${A}  a.js\n${B}  b.js\n`);
    expect(parseManifest(text)).toEqual([
      { path: "a.js", sha256: A },
      { path: "b.js", sha256: B },
    ]);
  });

  it.each([
    ["empty", ""],
    ["no final newline", `${A}  a.js`],
    ["CRLF", `${A}  a.js\r\n`],
    ["blank line", `${A}  a.js\n\n`],
    ["uppercase hex", `${A.toUpperCase()}  a.js\n`],
    ["short hash", `abc  a.js\n`],
    ["one space", `${A} a.js\n`],
    ["unsorted", `${A}  b.js\n${B}  a.js\n`],
    ["duplicate", `${A}  a.js\n${B}  a.js\n`],
    ["case duplicate", `${A}  A.js\n${B}  a.js\n`],
    ["parent segment", `${A}  ../a.js\n`],
    ["absolute", `${A}  /a.js\n`],
    ["backslash", `${A}  x\\a.js\n`],
  ])("refuses %s", (_name, text) => {
    expect(() => parseManifest(text)).toThrow();
  });
});

describe("checkPath and compareTrees", () => {
  it("accepts portable relative paths only", () => {
    for (const good of ["assets/index-abc.js", ".hidden", "a/b@2x+c~d_e.woff2", "console.log", "auxiliary.js"]) {
      expect(() => checkPath(good)).not.toThrow();
    }
    const bad = [
      "",
      ".",
      "a/./b",
      "a/../b",
      "a//b",
      "a/",
      "/a",
      "a\u0000b",
      "a\tb",
      "C:\\x",
      "a b.js",
      "a.js ",
      "a.",
      "dir./b",
      "café.js", // NFC but outside the allowlist
      "cafe\u0301.js", // not NFC
      "a:b",
      "a*b",
      "con",
      "NUL.js",
      "assets/com1.txt",
      "lpt9",
      "aux.tar.gz",
    ];
    for (const path of bad) expect(() => checkPath(path), JSON.stringify(path)).toThrow();
    expect(() => checkPath("cafe\u0301.js")).toThrow(/NFC/);
  });

  it("refuses a repeated or incomplete option", () => {
    expect(parseArgs(["--dir", "d", "--out", "o"])).toEqual({ dir: "d", out: "o" });
    expect(() => parseArgs(["--dir", "d", "--dir", "e"])).toThrow(/twice/);
    expect(() => parseArgs(["--dir"])).toThrow(/bad arguments/);
    expect(() => parseArgs(["dir", "d"])).toThrow(/bad arguments/);
  });

  it("lists every difference", () => {
    const problems = compareTrees(
      [
        { path: "a", sha256: A },
        { path: "b", sha256: A },
      ],
      [
        { path: "a", sha256: B },
        { path: "c", sha256: A },
      ],
    );
    expect(problems).toEqual(["changed: a", "missing: b", "extra: c"]);
  });
});
