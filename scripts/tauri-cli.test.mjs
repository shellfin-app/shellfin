// Unit tests of the prebuilt Tauri CLI fetcher (release workflow, docs/ARCHITECTURE.md §11.3). Synthetic lockfiles
// and tarballs only; nothing is downloaded.
import { Buffer } from "node:buffer";
import { createHash } from "node:crypto";
import { describe, expect, it } from "vitest";
import { TRIPLES, checkIntegrity, cliPackage, extractMember, tripleFor } from "./tauri-cli.mjs";

const V = "2.12.3";
const SHA512 = createHash("sha512").update("tarball").digest("base64");

function lockFor(triple, overrides = {}) {
  return {
    packages: {
      "node_modules/@tauri-apps/cli": { version: V },
      [`node_modules/@tauri-apps/cli-${triple}`]: {
        version: V,
        resolved: `https://registry.npmjs.org/@tauri-apps/cli-${triple}/-/cli-${triple}-${V}.tgz`,
        integrity: `sha512-${SHA512}`,
        ...overrides,
      },
    },
  };
}
const PKG = { devDependencies: { "@tauri-apps/cli": V } };

/** One ustar header + content block. */
function tarEntry(name, content, type = "0") {
  const header = Buffer.alloc(512);
  header.write(name, 0, "utf8");
  header.write("0000644\0", 100);
  header.write("0000000\0", 108);
  header.write("0000000\0", 116);
  header.write(`${content.length.toString(8).padStart(11, "0")}\0`, 124);
  header.write("00000000000\0", 136);
  header.write(type, 156);
  header.write("ustar\0", 257);
  header.write("00", 263);
  header.fill(32, 148, 156);
  let sum = 0;
  for (const b of header) sum += b;
  header.write(`${sum.toString(8).padStart(6, "0")}\0 `, 148);
  const body = Buffer.alloc(Math.ceil(content.length / 512) * 512);
  Buffer.from(content).copy(body);
  return Buffer.concat([header, body]);
}
const END = Buffer.alloc(1024);

describe("tripleFor", () => {
  it("maps exactly the four release targets", () => {
    expect(tripleFor("win32", "x64")).toBe("win32-x64-msvc");
    expect(tripleFor("win32", "arm64")).toBe("win32-arm64-msvc");
    expect(tripleFor("linux", "x64")).toBe("linux-x64-gnu");
    expect(tripleFor("darwin", "arm64")).toBe("darwin-arm64");
    expect(Object.keys(TRIPLES)).toHaveLength(4);
    for (const [p, a] of [["darwin", "x64"], ["linux", "arm64"], ["win32", "ia32"]]) {
      expect(() => tripleFor(p, a)).toThrow(/no prebuilt/);
    }
  });
});

describe("cliPackage", () => {
  it("returns the registry URL, integrity and member", () => {
    expect(cliPackage(lockFor("linux-x64-gnu"), PKG, "linux-x64-gnu")).toEqual({
      name: "@tauri-apps/cli-linux-x64-gnu",
      version: V,
      url: `https://registry.npmjs.org/@tauri-apps/cli-linux-x64-gnu/-/cli-linux-x64-gnu-${V}.tgz`,
      sha512: SHA512,
      member: "package/cli.linux-x64-gnu.node",
    });
  });

  it("refuses a range in package.json", () => {
    expect(() => cliPackage(lockFor("darwin-arm64"), { devDependencies: { "@tauri-apps/cli": "^2.12.3" } }, "darwin-arm64")).toThrow(
      /not an exact version/,
    );
  });

  it("refuses version drift between package.json, the lockfile and the platform package", () => {
    const lock = lockFor("darwin-arm64");
    lock.packages["node_modules/@tauri-apps/cli"].version = "2.12.2";
    expect(() => cliPackage(lock, PKG, "darwin-arm64")).toThrow(/@tauri-apps\/cli 2\.12\.2/);
    expect(() => cliPackage(lockFor("darwin-arm64", { version: "2.12.2" }), PKG, "darwin-arm64")).toThrow(/2\.12\.2/);
  });

  it("refuses another registry or tarball and a missing or weak integrity", () => {
    const t = "win32-x64-msvc";
    expect(() => cliPackage(lockFor(t, { resolved: "https://evil.example.com/x.tgz" }), PKG, t)).toThrow(/resolves to/);
    expect(() => cliPackage(lockFor(t, { integrity: undefined }), PKG, t)).toThrow(/no sha512/);
    expect(() => cliPackage(lockFor(t, { integrity: "sha1-abc" }), PKG, t)).toThrow(/no sha512/);
    expect(() => cliPackage({ packages: lockFor(t).packages }, PKG, "win32-arm64-msvc")).toThrow(/has no/);
  });
});

describe("checkIntegrity", () => {
  it("accepts the recorded hash only", () => {
    expect(() => checkIntegrity(Buffer.from("tarball"), SHA512)).not.toThrow();
    expect(() => checkIntegrity(Buffer.from("tarbalL"), SHA512)).toThrow(/differs/);
  });
});

describe("extractMember", () => {
  const member = "package/cli.linux-x64-gnu.node";

  it("extracts the member and nothing else", () => {
    const tar = Buffer.concat([tarEntry("package/package.json", "{}"), tarEntry(member, "\x7fELF addon"), END]);
    expect(extractMember(tar, member).toString()).toBe("\x7fELF addon");
  });

  it("refuses a missing, duplicate or non-regular member", () => {
    expect(() => extractMember(Buffer.concat([tarEntry("package/x", "a"), END]), member)).toThrow(/not in the tarball/);
    expect(() => extractMember(Buffer.concat([tarEntry(member, "a"), tarEntry(member, "b"), END]), member)).toThrow(/twice/);
    expect(() => extractMember(Buffer.concat([tarEntry(member, "", "2"), END]), member)).toThrow(/not a regular file/);
  });

  it("refuses a corrupt header or a truncated archive", () => {
    const bad = Buffer.concat([tarEntry(member, "abc"), END]);
    bad[0] ^= 1;
    expect(() => extractMember(bad, member)).toThrow(/checksum/);
    const truncated = tarEntry(member, "x".repeat(2000)).subarray(0, 1024);
    expect(() => extractMember(truncated, member)).toThrow(/truncated/);
  });
});
