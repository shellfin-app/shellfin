import { useCallback, useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import i18next from "i18next";
import { useTranslation } from "react-i18next";
import { Icon } from "../../components/Icon";
import { MENUS, MENU_IDS, shortcutLabel, type MenuId } from "../../lib/actionIds";
import { SEL_MENU_ITEM } from "../../lib/domIds";
import { isImeKeyEvent } from "../../lib/ime";
import { foldMnemonic, mnemonicOf, parseMenuTitle, plainMenuTitle } from "../../lib/menuMnemonic";
import { useStore } from "../../state/store";
import { isModalOpen } from "../session/promptVisibility";
import { ACTIONS, actionEnabled, actionLabel, runAction } from "../actions/registry";
import { useCompactTitleBar } from "./useCompactTitleBar";
import "../menu/menu.css";

interface Open {
  /** `root` is the list of menus behind the compact title bar's single menu button. */
  menu: MenuId | "root";
  /** Opened with the keyboard: the first item gets the focus. */
  viaKeyboard: boolean;
  /** Compact list reached with Back: the menu it came from (its entry gets the focus). */
  returnTo?: MenuId;
}

function inTerminal(el: Element | null): boolean {
  return el?.closest(".xterm") != null;
}

/** The label with its mnemonic character marked: the `&`-marked one, else the first (`lib/menuMnemonic.ts`). */
function MenuLabel({ text }: { text: string }) {
  const { before, mnemonic, after } = parseMenuTitle(text);
  return (
    <>
      {before}
      <span className="menubar__mnemonic">{mnemonic}</span>
      {after}
    </>
  );
}

/** In the compact title bar: the "Menu" entry that goes from a menu back to the list of menus. */
const SEL_BACK = "[data-menu-back]";

function Dropdown({
  menu,
  compact,
  left,
  top,
  viaKeyboard,
  returnTo,
  onClose,
  onSwitch,
  onPick,
  onBack,
}: {
  /** A menu, or `root`: the list of the menus (compact title bar only). */
  menu: MenuId | "root";
  compact: boolean;
  left: number;
  top: number;
  viaKeyboard: boolean;
  /** `root` reached with Back from this menu: its entry gets the focus (not the first one). */
  returnTo: MenuId | undefined;
  onClose: (restore: boolean) => void;
  onSwitch: (delta: 1 | -1) => void;
  onPick: (menu: MenuId, viaKeyboard: boolean) => void;
  onBack: () => void;
}) {
  const { t } = useTranslation();
  const rootRef = useRef<HTMLDivElement>(null);
  const state = useStore((s) => s);
  const platform = state.platform;

  useLayoutEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    const list = [...root.querySelectorAll<HTMLElement>(SEL_MENU_ITEM)];
    const origin = menu === "root" && returnTo !== undefined ? list.find((el) => el.dataset["menuId"] === returnTo) : undefined;
    // A menu whose items are all disabled (Terminal on the Home tab) never drops the focus to the body, which would
    // leave Escape / Left / Right / Backspace with nothing to act on: the "‹ Menu" entry (compact) or the menu keeps it.
    const first = list.find((el) => !el.matches(SEL_BACK)) ?? list[0];
    if (origin) origin.focus();
    else if (viaKeyboard && first) first.focus();
    else root.focus();
  }, [menu, viaKeyboard, returnTo]);

  const items = () => [...(rootRef.current?.querySelectorAll<HTMLElement>(SEL_MENU_ITEM) ?? [])];
  const move = (to: number | "first" | "last") => {
    const list = items();
    if (list.length === 0) return;
    const current = list.indexOf(document.activeElement as HTMLElement);
    const next = to === "first" ? 0 : to === "last" ? list.length - 1 : (current + to + list.length) % list.length;
    list[next]?.focus();
  };

  const onKeyDown = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        move(1);
        return;
      case "ArrowUp":
        e.preventDefault();
        move(-1);
        return;
      case "Home":
        e.preventDefault();
        move("first");
        return;
      case "End":
        e.preventDefault();
        move("last");
        return;
      case "ArrowLeft":
        e.preventDefault();
        if (menu !== "root") onSwitch(-1);
        return;
      case "ArrowRight": {
        e.preventDefault();
        if (menu !== "root") {
          onSwitch(1);
          return;
        }
        const id = (document.activeElement as HTMLElement | null)?.dataset["menuId"];
        if (id !== undefined && (MENU_IDS as readonly string[]).includes(id)) onPick(id as MenuId, true);
        return;
      }
      case "Backspace":
        if (compact && menu !== "root") {
          e.preventDefault();
          onBack();
        }
        return;
      case "Escape":
        e.preventDefault();
        e.stopPropagation();
        onClose(true);
        return;
      case "Tab":
        e.preventDefault();
        onClose(true);
        return;
    }
    // Type-ahead: the next enabled item whose label starts with the typed letter.
    if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) {
      const letter = e.key.toLowerCase();
      const list = items();
      const start = list.indexOf(document.activeElement as HTMLElement);
      for (let step = 1; step <= list.length; step += 1) {
        const candidate = list[(start + step) % list.length];
        if (candidate?.textContent?.trim().toLowerCase().startsWith(letter)) {
          candidate.focus();
          e.preventDefault();
          return;
        }
      }
    }
  };

  return (
    <div
      ref={rootRef}
      className="context-menu menubar__dropdown"
      role="menu"
      aria-label={menu === "root" ? t("menubar.compact") : plainMenuTitle(t(`menubar.${menu}`))}
      tabIndex={-1}
      style={{ left, top }}
      onKeyDown={onKeyDown}
    >
      {menu === "root"
        ? MENU_IDS.map((id) => (
            <button
              key={id}
              type="button"
              role="menuitem"
              aria-haspopup="menu"
              data-menu-id={id}
              tabIndex={-1}
              className="context-menu__item"
              onMouseDown={(e) => e.preventDefault()}
              onClick={(e) => onPick(id, e.detail === 0)}
            >
              <span className="context-menu__label">
                <span>
                  <MenuLabel text={t(`menubar.${id}`)} />
                </span>
              </span>
              <Icon name="chevronRight" size={12} />
            </button>
          ))
        : null}
      {menu !== "root" && compact ? (
        <>
          <button
            type="button"
            role="menuitem"
            data-menu-back=""
            tabIndex={-1}
            className="context-menu__item"
            onMouseDown={(e) => e.preventDefault()}
            onClick={onBack}
          >
            <span className="context-menu__label">
              <Icon name="chevronLeft" size={12} />
              {t("menubar.compact")}
            </span>
          </button>
          <div className="context-menu__separator" role="separator" />
        </>
      ) : null}
      {menu === "root" ? null : MENUS[menu].map((entry, i) => {
        if (entry === "-") return <div key={`sep-${i}`} className="context-menu__separator" role="separator" />;
        const def = ACTIONS[entry];
        const checked = def.checked?.(state);
        const enabled = actionEnabled(entry, state);
        const hint = shortcutLabel(entry, platform);
        return (
          <button
            key={entry}
            type="button"
            role={checked === undefined ? "menuitem" : "menuitemcheckbox"}
            aria-checked={checked}
            aria-disabled={enabled ? undefined : true}
            aria-keyshortcuts={hint}
            tabIndex={-1}
            className="context-menu__item"
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => {
              if (!enabled) return;
              onClose(true);
              // Focus is back on the previous element first, so Edit commands reach the editor / terminal.
              setTimeout(() => runAction(entry), 0);
            }}
          >
            <span className="context-menu__label">
              <span className="context-menu__check" aria-hidden="true">
                {checked ? <Icon name="check" size={12} strokeWidth={2} /> : null}
              </span>
              {actionLabel(entry)}
            </span>
            {hint ? <span className="context-menu__shortcut">{hint}</span> : null}
          </button>
        );
      })}
    </div>
  );
}

/**
 * The menu bar of the custom title bar (Windows / Linux; macOS uses the native menu). Keyboard: Alt or F10 focuses
 * it, arrows move, Enter / Down opens, Escape leaves, Alt+letter opens a menu. The previous focus is restored when
 * a menu closes, so Edit commands act on the editor, terminal or field the user came from.
 */
export function MenuBar() {
  const { t } = useTranslation();
  const [open, setOpen] = useState<Open | null>(null);
  const [current, setCurrent] = useState<MenuId>("file");
  const [active, setActive] = useState(false);
  const [anchor, setAnchor] = useState({ left: 0, top: 0 });
  const buttons = useRef<Partial<Record<MenuId, HTMLButtonElement | null>>>({});
  const compactButton = useRef<HTMLButtonElement>(null);
  const barRef = useRef<HTMLDivElement>(null);
  const compact = useCompactTitleBar(barRef);
  const previousFocus = useRef<HTMLElement | null>(null);

  const remember = useCallback(() => {
    const el = document.activeElement;
    if (el instanceof HTMLElement && !barRef.current?.contains(el) && !el.closest(".menubar__dropdown")) previousFocus.current = el;
  }, []);

  const restore = useCallback(() => {
    const el = previousFocus.current;
    previousFocus.current = null;
    if (el?.isConnected) el.focus();
    else if (document.activeElement instanceof HTMLElement && barRef.current?.contains(document.activeElement)) document.activeElement.blur();
  }, []);

  const close = useCallback(
    (restoreFocus: boolean) => {
      setOpen(null);
      setActive(false);
      if (restoreFocus) restore();
    },
    [restore],
  );

  const show = useCallback(
    (menu: MenuId | "root", viaKeyboard: boolean, returnTo?: MenuId) => {
      const button = compact ? compactButton.current : menu === "root" ? null : buttons.current[menu];
      if (button) {
        const rect = button.getBoundingClientRect();
        setAnchor({ left: rect.left, top: rect.bottom });
      }
      if (menu !== "root") setCurrent(menu);
      setActive(true);
      setOpen(returnTo === undefined ? { menu, viaKeyboard } : { menu, viaKeyboard, returnTo });
    },
    [compact],
  );

  const step = useCallback(
    (from: MenuId, delta: 1 | -1) => MENU_IDS[(MENU_IDS.indexOf(from) + delta + MENU_IDS.length) % MENU_IDS.length] ?? from,
    [],
  );

  // Alt / F10 / Alt+letter, from anywhere except modals and (to keep readline and mc working) the terminal.
  useEffect(() => {
    let altAlone = false;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Alt") {
        altAlone = !e.ctrlKey && !e.shiftKey && !e.metaKey;
        return;
      }
      altAlone = false;
      if (isImeKeyEvent(e)) return; // a key the IME is composing with is not a menu key
      const state = useStore.getState();
      if (isModalOpen(state) || inTerminal(document.activeElement)) return;
      if (e.key === "F10" && !e.ctrlKey && !e.shiftKey && !e.altKey && !e.metaKey) {
        e.preventDefault();
        remember();
        show("file", true); // Windows convention: F10 opens the first menu, not the last used one
        return;
      }
      if (e.altKey && !e.ctrlKey && !e.metaKey && !e.shiftKey && e.getModifierState("AltGraph") !== true && e.key.length === 1) {
        const letter = foldMnemonic(e.key);
        // The mnemonic is the `&`-marked (else the first) letter of the translated name, compared without accents
        // (Alt+E opens "Édition"); with another keyboard layout than the UI language (Ukrainian UI on a Latin layout
        // and the reverse) the physical key of the English name's mnemonic works too.
        const physical = /^Key([A-Z])$/.exec(e.code)?.[1]?.toLowerCase();
        const english = i18next.getFixedT("en");
        const hit =
          MENU_IDS.find((id) => mnemonicOf(t(`menubar.${id}`)) === letter) ??
          (physical === undefined ? undefined : MENU_IDS.find((id) => mnemonicOf(english(`menubar.${id}`)) === physical));
        if (hit) {
          e.preventDefault();
          remember();
          show(hit, true);
        }
      }
    };
    const onKeyUp = (e: KeyboardEvent) => {
      if (e.key !== "Alt") return;
      const state = useStore.getState();
      if (!altAlone || isModalOpen(state) || inTerminal(document.activeElement)) {
        altAlone = false;
        return;
      }
      altAlone = false;
      e.preventDefault(); // keeps the OS window menu from taking the Alt tap
      remember();
      setActive(true);
      setCurrent("file");
      (compact ? compactButton.current : buttons.current["file"])?.focus();
    };
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("keyup", onKeyUp, true);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("keyup", onKeyUp, true);
    };
  }, [remember, show, t, compact]);

  // Clicking anywhere else closes the menu without moving focus.
  useEffect(() => {
    if (!open) return undefined;
    const onDown = (e: MouseEvent) => {
      if (e.target instanceof Node && (barRef.current?.contains(e.target) || (e.target as Element).closest?.(".menubar__dropdown"))) return;
      close(false);
    };
    const dismiss = () => close(false);
    document.addEventListener("mousedown", onDown, true);
    window.addEventListener("blur", dismiss);
    window.addEventListener("resize", dismiss);
    return () => {
      document.removeEventListener("mousedown", onDown, true);
      window.removeEventListener("blur", dismiss);
      window.removeEventListener("resize", dismiss);
    };
  }, [open, close]);

  const onBarKeyDown = (e: ReactKeyboardEvent<HTMLButtonElement>, menu: MenuId) => {
    switch (e.key) {
      case "ArrowRight":
      case "ArrowLeft": {
        e.preventDefault();
        const next = step(menu, e.key === "ArrowRight" ? 1 : -1);
        setCurrent(next);
        buttons.current[next]?.focus();
        return;
      }
      case "ArrowDown":
      case "ArrowUp":
      case "Enter":
      case " ":
        e.preventDefault();
        show(menu, true);
        return;
      case "Escape":
        e.preventDefault();
        close(true);
        return;
    }
  };

  return (
    <div
      ref={barRef}
      className={`menubar${active ? " menubar--active" : ""}`}
      role="menubar"
      aria-label={t("menubar.label")}
      onBlur={(e) => {
        // Focus left the bar for good (not into a dropdown): the Alt-activated state ends.
        const next = e.relatedTarget;
        if (open === null && !(next instanceof Node && barRef.current?.contains(next))) setActive(false);
      }}
    >
      {compact ? (
        <button
          ref={compactButton}
          type="button"
          role="menuitem"
          className={`menubar__item menubar__item--compact${open ? " menubar__item--open" : ""}`}
          aria-label={t("menubar.compact")}
          title={t("menubar.compact")}
          aria-haspopup="menu"
          aria-expanded={open !== null}
          onMouseDown={(e) => {
            e.preventDefault();
            remember();
          }}
          onClick={(e) => (open ? close(true) : show("root", e.detail === 0))}
          onKeyDown={(e) => {
            switch (e.key) {
              case "ArrowDown":
              case "ArrowUp":
              case "Enter":
              case " ":
                e.preventDefault();
                show("root", true);
                return;
              case "Escape":
                e.preventDefault();
                close(true);
                return;
            }
          }}
        >
          <Icon name="menu" size={16} />
        </button>
      ) : (
        MENU_IDS.map((id) => (
        <button
          key={id}
          ref={(el) => {
            buttons.current[id] = el;
          }}
          type="button"
          role="menuitem"
          className={`menubar__item${open?.menu === id ? " menubar__item--open" : ""}`}
          aria-haspopup="menu"
          aria-expanded={open?.menu === id}
          tabIndex={current === id ? 0 : -1}
          // Keep the focus where it is (the editor, a field ...) when the pointer opens a menu.
          onMouseDown={(e) => {
            e.preventDefault();
            remember();
          }}
          onClick={() => (open?.menu === id ? close(true) : show(id, false))}
          onMouseEnter={() => {
            if (open && open.menu !== id) show(id, false);
          }}
          onFocus={() => setCurrent(id)}
          onKeyDown={(e) => onBarKeyDown(e, id)}
        >
          <MenuLabel text={t(`menubar.${id}`)} />
        </button>
        ))
      )}
      {open ? (
        <Dropdown
          menu={open.menu}
          compact={compact}
          left={anchor.left}
          top={anchor.top}
          viaKeyboard={open.viaKeyboard}
          returnTo={open.returnTo}
          onClose={close}
          onSwitch={(delta) => {
            if (open.menu !== "root") show(step(open.menu, delta), true);
          }}
          onPick={(menu, viaKeyboard) => show(menu, viaKeyboard)}
          onBack={() => show("root", true, open.menu === "root" ? undefined : open.menu)}
        />
      ) : null}
    </div>
  );
}
