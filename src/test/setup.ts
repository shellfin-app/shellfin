import { cleanup, configure } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, vi } from "vitest";
import { setPromptInputGuardMs } from "../features/session/promptGuard";
import { initI18n } from "../i18n";

// `findBy*`/`waitFor` return as soon as their condition holds; the default 1 s upper bound is too short for the
// first load of a lazy chunk (command palette, Settings, editor) on a busy CI runner. Stays below the 5 s test timeout.
configure({ asyncUtilTimeout: 4000 });

beforeAll(async () => {
  await initI18n("en");
});

// Tests type into prompts right away; the dedicated guard tests re-enable it.
beforeEach(() => setPromptInputGuardMs(0));

afterEach(() => {
  cleanup();
});

// jsdom lacks ResizeObserver (react-resizable-panels, xterm fit).
class ResizeObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
}
window.matchMedia ??= ((query: string) => ({
  matches: false,
  media: query,
  onchange: null,
  addListener() {},
  removeListener() {},
  addEventListener() {},
  removeEventListener() {},
  dispatchEvent: () => false,
})) as unknown as typeof window.matchMedia;
globalThis.ResizeObserver ??= ResizeObserverStub as unknown as typeof ResizeObserver;

// CodeMirror measures text ranges for its selection layer; jsdom has no layout.
Range.prototype.getClientRects ??= function getClientRects() {
  return [] as unknown as DOMRectList;
};
Range.prototype.getBoundingClientRect ??= function getBoundingClientRect() {
  return new DOMRect(0, 0, 0, 0);
};

// cmdk scrolls the selected item into view.
Element.prototype.scrollIntoView ??= function scrollIntoView() {};

// react-resizable-panels hit-tests pointer events against layout rectangles, which are all zero in jsdom (it then
// steals focus into a separator). Layout is not what these tests are about: render plain containers.
vi.mock("react-resizable-panels", async () => {
  const React = await import("react");
  const box = (name: string) =>
    function Box({ children, className }: { children?: React.ReactNode; className?: string }) {
      return React.createElement("div", { className, "data-panel-stub": name }, children);
    };
  return { Group: box("group"), Panel: box("panel"), Separator: box("separator") };
});
