import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent } from "react";
import { Icon } from "../../components/Icon";
import { SEL_MENU_ITEM } from "../../lib/domIds";
import { useTranslation } from "react-i18next";
import { useMenuStore, type MenuItem } from "./menuStore";
import { useStore } from "../../state/store";
import { placeMenu } from "./menuPlacement";
import "./menu.css";

/** The app's context menu: keyboard accessible (arrows, Home/End, Enter/Space, Escape, Tab closes). */
export function ContextMenu() {
  const { t } = useTranslation();
  const menu = useMenuStore((s) => s.menu);
  const close = useMenuStore((s) => s.close);
  const rootRef = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ left: 0, top: 0 });

  // Keep the menu inside the viewport.
  useLayoutEffect(() => {
    const el = rootRef.current;
    if (!menu || !el) return;
    setPosition(
      placeMenu({
        x: menu.x,
        y: menu.y,
        width: el.offsetWidth,
        height: el.offsetHeight,
        viewportWidth: window.innerWidth,
        viewportHeight: window.innerHeight,
        windows: useStore.getState().platform === "windows",
      }),
    );
    // No enabled item: the menu itself keeps the focus, so Escape and Tab still close it.
    (el.querySelector<HTMLElement>(SEL_MENU_ITEM) ?? el).focus();
  }, [menu]);

  // Anything that moves the pointer context closes the menu.
  useEffect(() => {
    if (!menu) return undefined;
    const closeIfOutside = (e: Event) => {
      if (e.target instanceof Node && rootRef.current?.contains(e.target)) return;
      close();
    };
    const dismiss = () => close();
    document.addEventListener("mousedown", closeIfOutside, true);
    document.addEventListener("contextmenu", closeIfOutside, true);
    window.addEventListener("blur", dismiss);
    window.addEventListener("resize", dismiss);
    window.addEventListener("wheel", dismiss, { passive: true });
    return () => {
      document.removeEventListener("mousedown", closeIfOutside, true);
      document.removeEventListener("contextmenu", closeIfOutside, true);
      window.removeEventListener("blur", dismiss);
      window.removeEventListener("resize", dismiss);
      window.removeEventListener("wheel", dismiss);
    };
  }, [menu, close]);

  if (!menu) return null;

  const enabledItems = () => [...(rootRef.current?.querySelectorAll<HTMLElement>(SEL_MENU_ITEM) ?? [])];
  const move = (delta: number | "first" | "last") => {
    const items = enabledItems();
    if (items.length === 0) return;
    const current = items.indexOf(document.activeElement as HTMLElement);
    const next =
      delta === "first" ? 0 : delta === "last" ? items.length - 1 : (current + delta + items.length) % items.length;
    items[next]?.focus();
  };
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        move(1);
        break;
      case "ArrowUp":
        e.preventDefault();
        move(-1);
        break;
      case "Home":
        e.preventDefault();
        move("first");
        break;
      case "End":
        e.preventDefault();
        move("last");
        break;
      case "Escape":
        e.preventDefault();
        e.stopPropagation();
        close();
        menu.restoreFocus();
        break;
      case "Tab":
        e.preventDefault();
        close();
        menu.restoreFocus();
        break;
    }
  };
  const select = (item: MenuItem) => {
    if (item.disabled) return;
    close();
    menu.restoreFocus();
    item.onSelect();
  };

  return (
    <div
      ref={rootRef}
      className="context-menu"
      role="menu"
      aria-label={t("menu.label")}
      tabIndex={-1}
      style={{ left: position.left, top: position.top }}
      onKeyDown={onKeyDown}
    >
      {menu.items.map((item) => (
        <div key={item.id} role="none">
          {item.separatorBefore ? <div className="context-menu__separator" role="separator" /> : null}
          <button
            type="button"
            role={item.checked === undefined ? "menuitem" : "menuitemcheckbox"}
            aria-checked={item.checked}
            className={`context-menu__item${item.danger ? " context-menu__item--danger" : ""}`}
            aria-disabled={item.disabled === true ? true : undefined}
            title={item.title}
            aria-keyshortcuts={item.shortcut}
            tabIndex={-1}
            onClick={() => select(item)}
          >
            <span className="context-menu__label">
              {item.checked === undefined ? null : (
                <span className="context-menu__check" aria-hidden="true">
                  {item.checked ? <Icon name="check" size={12} strokeWidth={2} /> : null}
                </span>
              )}
              {item.leading}
              {item.label}
            </span>
            {item.shortcut ? <span className="context-menu__shortcut">{item.shortcut}</span> : null}
          </button>
        </div>
      ))}
    </div>
  );
}
