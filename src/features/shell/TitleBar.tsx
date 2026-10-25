import { useEffect, useState, type MouseEvent } from "react";
import { useTranslation } from "react-i18next";
import { BrandMark } from "../../components/BrandMark";
import { Icon } from "../../components/Icon";
import { getBackend } from "../../ipc";
import { guard } from "../../lib/reportError";
import { useStore } from "../../state/store";
import { ConnectionTabs } from "./ConnectionTabs";
import { MenuBar } from "./MenuBar";

function WindowControls() {
  const { t } = useTranslation();
  const maximized = useStore((s) => s.windowState.maximized);
  const backend = () => getBackend();
  // Windows: the native Snap Layouts overlay sits over the maximize button and takes the mouse, so the button's
  // hover look comes from `window:max-hover`. Transient, local state; the click still arrives through the overlay.
  const [maxHover, setMaxHover] = useState(false);
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void getBackend()
      .listen("window:max-hover", ({ hovered }) => setMaxHover(hovered))
      .then((off) => {
        if (cancelled) off();
        else unlisten = off;
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);
  return (
    <div className="window-controls">
      <button
        type="button"
        className="window-controls__button"
        aria-label={t("titlebar.minimize")}
        title={t("titlebar.minimize")}
        onClick={() => guard(backend().windowMinimize())}
      >
        <Icon name="winMinimize" size={16} strokeWidth={1.4} />
      </button>
      <button
        type="button"
        className={`window-controls__button${maxHover ? " window-controls__button--hover" : ""}`}
        aria-label={maximized ? t("titlebar.restore") : t("titlebar.maximize")}
        title={maximized ? t("titlebar.restore") : t("titlebar.maximize")}
        onClick={() => guard(backend().windowToggleMaximize())}
      >
        <Icon name={maximized ? "winRestore" : "winMaximize"} size={16} strokeWidth={1.4} />
      </button>
      <button
        type="button"
        className="window-controls__button window-controls__button--close"
        aria-label={t("titlebar.close")}
        title={t("titlebar.close")}
        onClick={() => guard(backend().windowClose())}
      >
        <Icon name="close" size={16} strokeWidth={1.4} />
      </button>
    </div>
  );
}

/**
 * Title bar of the frameless window: brand mark, menu bar (Windows / Linux), connection tabs, the Ctrl+K
 * search button and the window controls, all in one row. macOS keeps the native traffic lights on the left (the
 * content is offset) and the native menu bar. Empty areas are the drag region.
 */
export function TitleBar() {
  const { t } = useTranslation();
  const platform = useStore((s) => s.platform);
  const fullscreen = useStore((s) => s.windowState.fullscreen);
  const focused = useStore((s) => s.windowState.focused);
  const openPalette = useStore((s) => s.openPalette);
  const mac = platform === "macos";

  // `data-tauri-drag-region` needs a `core:window` permission that is not granted (ARCHITECTURE §4.2, Window): drag the window
  // from here instead. A double click on the bar maximizes / restores it. Buttons, tabs and the menu are not handles.
  const onMouseDown = (e: MouseEvent<HTMLElement>) => {
    if (e.button !== 0 || !(e.target instanceof Element)) return;
    if (e.target.closest("button, [role=tab], [role=menuitem], [role=menubar], input, a, .window-controls") !== null) return;
    guard(e.detail === 2 ? getBackend().windowToggleMaximize() : getBackend().windowStartDragging());
  };

  return (
    <header
      className={`title-bar title-bar--${platform}${mac && !fullscreen ? " title-bar--traffic-lights" : ""}${focused ? "" : " title-bar--unfocused"}`}
      onMouseDown={onMouseDown}
    >
      {/* Everything but the window controls: it may shrink and clips, so the controls stay flush right (the Snap
          Layouts overlay is placed by geometry). Below 960 px the menu bar is one menu button (MenuBar). */}
      <div className="title-bar__content">
      <div className="title-bar__logo">
        <BrandMark size={20} />
      </div>
      {mac ? null : <MenuBar />}
      <ConnectionTabs />
      <div className="title-bar__drag" />
      {/* A button styled as a search field: it opens the palette only on activation, never on focus —
          dialogs and menus restore focus here when they close, and opening on focus would loop. Below 1200 px it
          shrinks to the icon. */}
      <button
        type="button"
        className="search-box"
        aria-label={t("shell.search.label")}
        title={t("shell.search.label")}
        aria-haspopup="dialog"
        onClick={() => openPalette("all")}
      >
        <Icon name="search" size={14} />
        <span className="search-box__input">{t("shell.search.placeholder")}</span>
        <kbd className="search-box__hint mono">{mac ? t("shortcut.paletteMac") : t("shortcut.palette")}</kbd>
      </button>
      </div>
      {mac || fullscreen ? null : <WindowControls />}
    </header>
  );
}
