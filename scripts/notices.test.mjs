// Unit tests of the licence notice generator (scripts/notices.mjs). The Rust part needs cargo-about and is checked by
// the CI job `npm run notices:check`; here only the pure helpers and the committed output are tested.
import { existsSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { INSTALLER_LICENSE, OUTPUT, groupByText, installerLicense, normalize, productionPackages } from "./notices.mjs";

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
    const text = readFileSync(OUTPUT, "utf8");
    expect(text).not.toContain("\r");
    expect(text).toContain("SIL OPEN FONT LICENSE Version 1.1");
    expect(text).toContain("Inter Project Authors");
    expect(text).toContain("JetBrains Mono Project Authors");
  });

  it("the installer licence page holds both licence texts", () => {
    const text = readFileSync(INSTALLER_LICENSE, "utf8");
    expect(text).toBe(installerLicense());
    expect(text).toContain("Permission is hereby granted, free of charge");
    expect(text).toContain("Apache License");
  });
});
