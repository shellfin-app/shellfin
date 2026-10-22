import { useCallback, useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import i18next from "i18next";
import { useTranslation } from "react-i18next";
import { Icon } from "../../components/Icon";
import { MENUS, MENU_IDS, shortcutLabel, type MenuId } from "../../lib/actionIds";
import { SEL_MENU_ITEM } from "../../lib/domIds";
import { mnemonicChar, mnemonicOf } from "../../lib/menuMnemonic";
import { useStore } from "../../state/store";
import { isModalOpen } from "../session/promptVisibility";
import { ACTIONS, actionEnabled, actionLabel, runAction } from "../actions/registry";
import "../menu/menu.css";

interface Open {
  menu: MenuId;
  /** Opened with the keyboard: the first item gets the focus. */
  viaKeyboard: boolean;
}

function inTerminal(el: Element | null): boolean {
  return el?.closest(".xterm") != null;
}

/** The label with its mnemonic letter (first letter) marked. */
function MenuLabel({ text }: { text: string }) {
  const first = mnemonicChar(text);
  return (
    <>
      <span className="menubar__mnemonic">{first}</span>
      {text.slice(first.length)}
    </>
  );
}

function Dropdown({
  menu,
  left,
  top,
  viaKeyboard,
  onClose,
  onSwitch,
}: {
  menu: MenuId;
  left: number;
  top: number;
  viaKeyboard: boolean;
  onClose: (restore: boolean) => void;
  onSwitch: (delta: 1 | -1) => void;
}) {
  const { t } = useTranslation();
  const rootRef = useRef<HTMLDivElement>(null);
  const state = useStore((s) => s);
  const platform = state.platform;

  useLayoutEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    if (viaKeyboard) root.querySelector<HTMLElement>(SEL_MENU_ITEM)?.focus();
    else root.focus();
  }, [menu, viaKeyboard]);

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
        onSwitch(-1);
        return;
      case "ArrowRight":
        e.preventDefault();
        onSwitch(1);
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
      aria-label={t(`menubar.${menu}`)}
      tabIndex={-1}
      style={{ left, top }}
      onKeyDown={onKeyDown}
    >
      {MENUS[menu].map((entry, i) => {
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
  const barRef = useRef<HTMLDivElement>(null);
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

  const show = useCallback((menu: MenuId, viaKeyboard: boolean) => {
    const button = buttons.current[menu];
    if (button) {
      const rect = button.getBoundingClientRect();
      setAnchor({ left: rect.left, top: rect.bottom });
    }
    setCurrent(menu);
    setActive(true);
    setOpen({ menu, viaKeyboard });
  }, []);

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
      const state = useStore.getState();
      if (isModalOpen(state) || inTerminal(document.activeElement)) return;
      if (e.key === "F10" && !e.ctrlKey && !e.shiftKey && !e.altKey && !e.metaKey) {
        e.preventDefault();
        remember();
        show("file", true); // Windows convention: F10 opens the first menu, not the last used one
        return;
      }
      if (e.altKey && !e.ctrlKey && !e.metaKey && !e.shiftKey && e.getModifierState("AltGraph") !== true && e.key.length === 1) {
        const letter = e.key.toLowerCase();
        // The mnemonic is the first letter of the translated name; with another keyboard layout than the UI language
        // (Ukrainian UI on a Latin layout and the reverse) the physical key of the English name's first letter works too.
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
      buttons.current["file"]?.focus();
    };
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("keyup", onKeyUp, true);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("keyup", onKeyUp, true);
    };
  }, [remember, show, t]);

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
      {MENU_IDS.map((id) => (
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
      ))}
      {open ? (
        <Dropdown
          menu={open.menu}
          left={anchor.left}
          top={anchor.top}
          viaKeyboard={open.viaKeyboard}
          onClose={close}
          onSwitch={(delta) => show(step(open.menu, delta), true)}
        />
      ) : null}
    </div>
  );
}
