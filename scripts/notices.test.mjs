// Unit tests of the licence notice generator (scripts/notices.mjs). The Rust part needs cargo-about and is checked by
// the CI job `npm run notices:check`; here only the pure helpers and the committed output are tested.
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { relative } from "node:path";
import process from "node:process";
import { describe, expect, it } from "vitest";
import { INSTALLER_LICENSE, OUTPUT, groupByText, installerLicense, normalize, productionPackages } from "./notices.mjs";

/**
 * The committed content of a generated file. A working copy may have CRLF (core.autocrlf=true on Windows) although the
 * repository holds LF, so the line-ending assertions read the blob from the git index. Outside a git checkout (or for
 * an untracked file) it falls back to the file on disk.
 */
function committedText(path) {
  try {
    return execFileSync("git", ["show", `:${relative(process.cwd(), path).replaceAll("\\", "/")}`], {
      encoding: "utf8",
      maxBuffer: 64 * 1024 * 1024,
      stdio: ["ignore", "pipe", "ignore"],
    });
  } catch {
    return readFileSync(path, "utf8");
  }
}

describe("normalize", () => {
  it("uses LF, strips trailing blanks and ends with exactly one newline", () => {
    expect(normalize("a  \r\nb\t\r\n\r\n\r\n")).toBe("a\nb\n");
  });
});

describe("productionPackages", () => {
  it("skips the root and dev packages and sorts by name", () => {
    const lock = {
      packages: {
        "": { name: "shellfin" },
        "node_modules/zed": { version: "1.0.0", license: "MIT" },
        "node_modules/alpha": { version: "2.0.0", license: "ISC" },
        "node_modules/tool": { version: "3.0.0", license: "MIT", dev: true },
        "node_modules/alpha/node_modules/@scope/inner": { version: "1.1.0" },
      },
    };
    expect(productionPackages(lock).map((p) => `${p.name}@${p.version} ${p.license}`)).toEqual([
      "@scope/inner@1.1.0 UNKNOWN",
      "alpha@2.0.0 ISC",
      "zed@1.0.0 MIT",
    ]);
  });
});

describe("groupByText", () => {
  it("merges packages with the same licence text", () => {
    const groups = groupByText([
      { name: "b", version: "1", license: "MIT", text: "T\n" },
      { name: "a", version: "1", license: "MIT", text: "T\n" },
      { name: "c", version: "1", license: "MIT", text: "other\n" },
    ]);
    expect(groups).toHaveLength(2);
    expect(groups.find((g) => g.text === "T\n")?.used).toEqual(["b 1", "a 1"]);
  });
});

describe("shipped notices", () => {
  it("contain both OFL font licences and are LF only", () => {
    expect(existsSync(OUTPUT)).toBe(true);
    const text = committedText(OUTPUT);
    expect(text).not.toContain("\r");
    expect(text).toContain("SIL OPEN FONT LICENSE Version 1.1");
    expect(text).toContain("Inter Project Authors");
    expect(text).toContain("JetBrains Mono Project Authors");
  });

  it("the installer licence page holds both licence texts", () => {
    const text = committedText(INSTALLER_LICENSE);
    expect(text).toBe(installerLicense());
    expect(text).toContain("Permission is hereby granted, free of charge");
    expect(text).toContain("Apache License");
  });
});
