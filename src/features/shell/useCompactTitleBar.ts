import { useEffect, useLayoutEffect, useState, type RefObject } from "react";
import i18next from "i18next";
import { SEL_MENUBAR_TITLES, SEL_TITLEBAR_CONTENT, SEL_TITLEBAR_FLEXIBLE } from "../../lib/domIds";
import { isCompactTitleBar } from "./layoutLimits";

const LANGUAGE_CHANGED = "languageChanged"; // i18next event name (data, not UI copy)

/** Whether the window is narrower than `below` px; follows window resizes (never focus changes). */
export function useNarrowWindow(below: number): boolean {
  const [narrow, setNarrow] = useState(() => window.innerWidth < below);
  useEffect(() => {
    const onResize = () => setNarrow(window.innerWidth < below);
    onResize();
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, [below]);
  return narrow;
}

function width(el: Element): number {
  return el.getBoundingClientRect().width;
}

/** Changes below half a pixel are rounding noise, not a new layout (keeps the observers from re-rendering). */
function settle(next: number): (prev: number | null) => number | null {
  return (prev) => (prev !== null && Math.abs(prev - next) < 0.5 ? prev : next);
}

/**
 * Whether the title bar shows the compact menu button instead of the six titles (ARCHITECTURE §8.4). Below
 * `COMPACT_TITLEBAR_BELOW` always; above it when the titles in the current language do not fit next to the logo, the
 * search button, the minimum tab strip and the drag area. `barRef` is the menu bar (`role=menubar`) inside
 * `.title-bar__content`. Driven by window resizes and a `ResizeObserver` on the bar and its row, never by focus.
 *
 * The titles can only be measured while they are shown: the last natural width is kept for the compact mode and
 * dropped when the language, the UI scale (`<html style>`) or `<html lang>` (the font stack) changes. Then the full bar renders once more and is
 * measured in a layout effect, before the browser paints, so no frame shows titles that do not fit.
 */
export function useCompactTitleBar(barRef: RefObject<HTMLElement | null>): boolean {
  const [windowWidth, setWindowWidth] = useState(() => window.innerWidth);
  const [menuWidth, setMenuWidth] = useState<number | null>(null);
  const [contentWidth, setContentWidth] = useState<number | null>(null);
  const [othersWidth, setOthersWidth] = useState<number | null>(null);
  const [generation, setGeneration] = useState(0);
  const compact = isCompactTitleBar(
    windowWidth,
    menuWidth === null || contentWidth === null || othersWidth === null ? null : { menuWidth, contentWidth, othersWidth },
  );

  useEffect(() => {
    const onResize = () => setWindowWidth(window.innerWidth);
    onResize();
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  // The compact bar cannot measure the titles: forget their width when it may have changed.
  useEffect(() => {
    const reset = () => {
      setMenuWidth(null);
      setGeneration((g) => g + 1); // measure again even when the mode does not change
    };
    i18next.on(LANGUAGE_CHANGED, reset);
    const scale = typeof MutationObserver === "undefined" ? null : new MutationObserver(reset);
    scale?.observe(document.documentElement, { attributes: true, attributeFilter: ["style", "lang"] });
    return () => {
      i18next.off(LANGUAGE_CHANGED, reset);
      scale?.disconnect();
    };
  }, []);

  useLayoutEffect(() => {
    const bar = barRef.current;
    const content = bar?.closest(SEL_TITLEBAR_CONTENT);
    if (!bar || !(content instanceof HTMLElement)) return undefined;
    const measure = () => {
      const style = getComputedStyle(content);
      const pad = (parseFloat(style.paddingLeft) || 0) + (parseFloat(style.paddingRight) || 0);
      const gap = parseFloat(style.columnGap) || 0;
      const items = [...content.children];
      // Logo, search button and every gap; the menu bar, the tab strip and the drag area are counted separately.
      const fixed = items
        .filter((el) => el !== bar && !el.matches(SEL_TITLEBAR_FLEXIBLE))
        .reduce((sum, el) => sum + width(el), 0);
      setContentWidth(settle(content.clientWidth - pad));
      setOthersWidth(settle(fixed + gap * Math.max(0, items.length - 1)));
      if (!compact) {
        const titles = [...bar.querySelectorAll(SEL_MENUBAR_TITLES)].reduce((sum, el) => sum + width(el), 0);
        setMenuWidth(settle(titles));
      }
    };
    measure();
    if (typeof ResizeObserver === "undefined") return undefined;
    const observer = new ResizeObserver(measure);
    observer.observe(content);
    observer.observe(bar);
    return () => observer.disconnect();
  }, [barRef, compact, generation]);

  return compact;
}
