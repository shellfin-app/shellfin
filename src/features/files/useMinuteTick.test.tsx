import { act, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { formatFileDateShort } from "../../i18n/format";
import { useMinuteTick } from "./useMinuteTick";

/** A date cell as the file tree renders it: relative to the ticking "now". */
function Cell({ mtimeSec, active }: { mtimeSec: number; active: boolean }) {
  const now = useMinuteTick(active);
  return <span data-testid="cell">{formatFileDateShort(mtimeSec, now, "relative")}</span>;
}

function setVisibility(state: "visible" | "hidden") {
  Object.defineProperty(document, "visibilityState", { configurable: true, get: () => state });
  document.dispatchEvent(new Event("visibilitychange"));
}

describe("useMinuteTick (relative dates in the date column)", () => {
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "Date"] });
    vi.setSystemTime(new Date(2026, 9, 20, 12, 0, 0));
  });
  afterEach(() => {
    setVisibility("visible");
    vi.useRealTimers();
  });

  it("moves 'now' on once a minute", () => {
    const mtime = Math.floor(Date.now() / 1000);
    render(<Cell mtimeSec={mtime} active />);
    expect(screen.getByTestId("cell").textContent).toBe("now");
    act(() => vi.advanceTimersByTime(60_000));
    expect(screen.getByTestId("cell").textContent).toBe(new Intl.RelativeTimeFormat("en", { numeric: "auto", style: "narrow" }).format(-1, "minute"));
  });

  it("pauses while the window is hidden and catches up when it is shown again", () => {
    const mtime = Math.floor(Date.now() / 1000);
    render(<Cell mtimeSec={mtime} active />);
    act(() => setVisibility("hidden"));
    act(() => vi.advanceTimersByTime(5 * 60_000));
    expect(screen.getByTestId("cell").textContent).toBe("now");
    expect(vi.getTimerCount()).toBe(0);
    act(() => setVisibility("visible"));
    expect(screen.getByTestId("cell").textContent).toBe(new Intl.RelativeTimeFormat("en", { numeric: "auto", style: "narrow" }).format(-5, "minute"));
  });

  it("runs no timer while inactive (columns off)", () => {
    render(<Cell mtimeSec={Math.floor(Date.now() / 1000)} active={false} />);
    expect(vi.getTimerCount()).toBe(0);
  });
});
