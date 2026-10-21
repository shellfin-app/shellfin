import { describe, expect, it } from "vitest";
import source from "./types.ts?raw";

describe("ipc/types.ts", () => {
  const generated = Object.keys(import.meta.glob("./bindings/*.ts"))
    .map((p) => p.replace(/^\.\/bindings\//, "").replace(/\.ts$/, ""))
    .sort();

  it("is a pure re-export of every generated binding (no hand-written contract types)", () => {
    const reexported = [...source.matchAll(/^export type \* from "\.\/bindings\/(\w+)";$/gm)].map((m) => m[1]).sort();
    expect(reexported).toEqual(generated);
    const code = source.split("\n").filter((l) => l.trim() !== "" && !l.startsWith("//"));
    expect(code.every((l) => l.startsWith("export type * from "))).toBe(true);
  });
});
