import { useId, useLayoutEffect, useRef, useState, type KeyboardEvent } from "react";
import { useTranslation } from "react-i18next";
import { Icon } from "./Icon";
import "./infotip.css";

/**
 * Small (?) button with a tooltip for the longer explanation of a setting. It is a tooltip, not a dialog: it opens on
 * hover, keyboard focus and click, closes on blur, mouse leave and Escape, never moves focus and never traps it.
 * The text stays in the DOM so `aria-describedby` always resolves; `setting` is the label it explains.
 */
export function InfoTip({ setting, text }: { setting: string; text: string }) {
  const { t } = useTranslation();
  const id = useId();
  const [hover, setHover] = useState(false);
  const [focus, setFocus] = useState(false);
  const [pinned, setPinned] = useState(false);
  const open = hover || focus || pinned;
  const [alignEnd, setAlignEnd] = useState(false);
  const popRef = useRef<HTMLSpanElement>(null);
  // When opening, check whether the tip would cross the right edge of its scroll area (or the window) and flip it.
  useLayoutEffect(() => {
    const pop = popRef.current;
    if (!open || !pop) return;
    const anchor = pop.parentElement?.getBoundingClientRect();
    if (!anchor) return;
    let edge = window.innerWidth;
    for (let el = pop.parentElement?.parentElement; el; el = el.parentElement) {
      const overflowX = getComputedStyle(el).overflowX;
      if (overflowX === "auto" || overflowX === "scroll" || overflowX === "hidden") {
        edge = Math.min(edge, el.getBoundingClientRect().right);
        break;
      }
    }
    setAlignEnd(anchor.left + pop.offsetWidth > edge);
  }, [open]);
  const close = () => {
    setHover(false);
    setFocus(false);
    setPinned(false);
  };
  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>) => {
    if (e.key !== "Escape" || !open) return;
    // Only the tooltip closes; a surrounding dialog stays open (it closes on an Escape that is not defaultPrevented).
    e.preventDefault();
    e.stopPropagation();
    e.nativeEvent.stopImmediatePropagation();
    close();
  };
  return (
    <span className="infotip" onMouseEnter={() => setHover(true)} onMouseLeave={() => setHover(false)}>
      <button
        type="button"
        className="infotip__button"
        aria-label={t("common.moreAbout", { setting })}
        aria-describedby={id}
        onFocus={() => setFocus(true)}
        onBlur={close}
        onClick={() => setPinned((p) => !p)}
        onKeyDown={onKeyDown}
      >
        <Icon name="help" size={14} />
      </button>
      <span
        ref={popRef}
        id={id}
        role="tooltip"
        className={`infotip__pop${open ? " infotip__pop--open" : ""}${alignEnd ? " infotip__pop--end" : ""}`}
      >
        {text}
      </span>
    </span>
  );
}
