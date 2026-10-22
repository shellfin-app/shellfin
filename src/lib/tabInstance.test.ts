import { describe, expect, it } from "vitest";
import { nextInstance } from "./tabInstance";

describe("nextInstance", () => {
  it("starts at 1", () => {
    expect(nextInstance([], "a")).toBe(1);
    expect(nextInstance([{ label: "b", instance: 1 }], "a")).toBe(1);
  });

  it("takes the next free number for the same label", () => {
    expect(nextInstance([{ label: "a", instance: 1 }], "a")).toBe(2);
    expect(
      nextInstance(
        [
          { label: "a", instance: 1 },
          { label: "a", instance: 2 },
        ],
        "a",
      ),
    ).toBe(3);
  });

  it("reuses the smallest freed number", () => {
    expect(
      nextInstance(
        [
          { label: "a", instance: 1 },
          { label: "a", instance: 3 },
        ],
        "a",
      ),
    ).toBe(2);
    expect(nextInstance([{ label: "a", instance: 2 }], "a")).toBe(1);
  });

  it("honours a free preferred number and falls back when it is taken", () => {
    const open = [{ label: "a", instance: 1 }];
    expect(nextInstance(open, "a", 3)).toBe(3);
    expect(nextInstance(open, "a", 1)).toBe(2);
    expect(nextInstance(open, "b", 1)).toBe(1);
  });
});
