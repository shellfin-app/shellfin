// The bindings orphan check (scripts/bindings-check.mjs) against the real sources.
import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { orphanBindings } from "./bindings-check.mjs";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");

function rustSources(dir) {
  return readdirSync(dir).flatMap((name) => {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) return rustSources(p);
    return p.endsWith(".rs") ? [readFileSync(p, "utf8")] : [];
  });
}

describe("orphanBindings", () => {
  it("finds a binding whose Rust type is gone", () => {
    const rust = ["pub struct Alive { a: u8 }", "pub enum Mode { A }"];
    expect(orphanBindings(["Alive.ts", "Mode.ts", "Gone.ts"], rust)).toEqual(["Gone.ts"]);
  });

  it("every committed binding has a Rust type", () => {
    const files = readdirSync(join(ROOT, "src", "ipc", "bindings")).filter((f) => f.endsWith(".ts"));
    expect(files.length).toBeGreaterThan(0);
    expect(orphanBindings(files, rustSources(join(ROOT, "src-tauri", "src")))).toEqual([]);
  });
});
