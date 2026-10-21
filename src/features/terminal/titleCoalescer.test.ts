import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MAX_TITLE_LENGTH, createTitleCoalescer } from "./titleCoalescer";

describe("OSC title coalescer", () => {
  let frames: Array<() => void>;
  beforeEach(() => {
    frames = [];
    vi.stubGlobal("requestAnimationFrame", (cb: () => void) => frames.push(cb));
    vi.stubGlobal("cancelAnimationFrame", (id: number) => {
      frames[id - 1] = () => undefined;
    });
  });
  afterEach(() => vi.unstubAllGlobals());

  const runFrame = () => {
    const pending = frames;
    frames = [];
    for (const cb of pending) cb();
  };

  it("a title flood is applied once per frame, with the last title", () => {
    const apply = vi.fn();
    const titles = createTitleCoalescer(apply);
    for (let i = 0; i < 1000; i += 1) titles.push(`title ${i}`);
    expect(apply).not.toHaveBeenCalled();
    expect(frames).toHaveLength(1); // one frame request, not one per title
    runFrame();
    expect(apply).toHaveBeenCalledTimes(1);
    expect(apply).toHaveBeenCalledWith("title 999");
    titles.push("next");
    runFrame();
    expect(apply).toHaveBeenLastCalledWith("next");
    expect(apply).toHaveBeenCalledTimes(2);
  });

  it("an empty title means no title and long ones are cut", () => {
    const apply = vi.fn();
    const titles = createTitleCoalescer(apply);
    titles.push("");
    runFrame();
    expect(apply).toHaveBeenLastCalledWith(null);
    titles.push("x".repeat(MAX_TITLE_LENGTH + 50));
    runFrame();
    expect(apply).toHaveBeenLastCalledWith("x".repeat(MAX_TITLE_LENGTH));
  });

  it("cancel drops a pending title", () => {
    const apply = vi.fn();
    const titles = createTitleCoalescer(apply);
    titles.push("late");
    titles.cancel();
    runFrame();
    expect(apply).not.toHaveBeenCalled();
  });
});
