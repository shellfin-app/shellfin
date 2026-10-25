import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { startApp } from "./bootstrap";
import { Button } from "./components/Button";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { DialogHost } from "./features/shell/DialogHost";
import { Notices } from "./features/shell/Notices";
import { ContextMenu } from "./features/menu/ContextMenu";
import { buildMenu } from "./features/menu/buildMenu";
import { useMenuStore } from "./features/menu/menuStore";
import { Shell } from "./features/shell/Shell";
import { closeActiveBuffer, cycleBuffer, saveActiveBuffer, saveAllBuffers } from "./features/editor/editorActions";
import { actionEnabled, runAction, toggleTerminalPanelFocusAware } from "./features/actions/registry";
import { openTerminalTab, rightClickPastes } from "./features/terminal/terminalActions";
import { errorMessage } from "./i18n/errorMessage";
import { isBrowserAccelerator } from "./lib/accelerators";
import { installDragDropGuard } from "./lib/dragdrop";
import { connectionTabIndex, matchShortcut } from "./lib/keybindings";
import { isModalOpen } from "./features/session/promptVisibility";
import { guard } from "./lib/reportError";
import { useStore } from "./state/store";

/** Global keyboard shortcuts (§8.7). Terminal-specific handling lives in the terminal registry. */
function useGlobalShortcuts() {
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      const state = useStore.getState();
      // No global shortcut while a modal (dialog, palette, prompt) is open: the modal owns the keyboard.
      if (isModalOpen(state)) return;
      const inTerminal = e.target instanceof Element && e.target.closest(".xterm") !== null;
      const tab = connectionTabIndex(e, state.platform);
      if (tab !== null) {
        const id = state.sessionOrder[tab];
        if (id) {
          e.preventDefault();
          state.setActiveSession(id);
        }
        return;
      }
      const action = matchShortcut(e, state.platform, inTerminal);
      if (!action) return;
      switch (action) {
        case "palette":
        case "paletteInTerminal":
          e.preventDefault();
          state.openPalette("all");
          break;
        case "newConnection":
          e.preventDefault();
          state.openDialog({ kind: "bookmark", bookmarkId: null });
          break;
        case "toggleSidebar":
          e.preventDefault();
          state.setSidebarOpen(!state.sidebarOpen);
          break;
        case "newTerminal": {
          const id = state.activeSessionId;
          if (id && state.workspaces[id] && state.sessionsById[id]?.state.phase === "connected") {
            e.preventDefault();
            guard(openTerminalTab(id));
          }
          break;
        }
        case "terminalFind": {
          const id = state.activeSessionId;
          const active = id ? state.workspaces[id]?.activeTerminal : null;
          if (active) {
            e.preventDefault();
            state.setTerminalSearch(active);
          }
          break;
        }
        case "save":
        case "saveAll":
        case "closeTab":
        case "nextTab":
        case "prevTab": {
          // Editor shortcuts: only with an open file in the visible session.
          const id = state.activeSessionId;
          if (!id || !state.workspaces[id]?.activeBuffer) break;
          e.preventDefault();
          if (action === "save") saveActiveBuffer(id);
          else if (action === "saveAll") guard(saveAllBuffers({ sessionId: id, interactive: true }));
          else if (action === "closeTab") closeActiveBuffer(id);
          else cycleBuffer(id, action === "nextTab" ? 1 : -1);
          break;
        }
        case "toggleTerminalPanel": {
          const id = state.activeSessionId;
          if (id && state.workspaces[id]) {
            e.preventDefault();
            toggleTerminalPanelFocusAware();
          }
          break;
        }
        // Menu-bar shortcuts and focus movement: one registry for menus, shortcuts and the palette.
        default:
          if (actionEnabled(action)) {
            e.preventDefault();
            runAction(action);
          }
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);
}

/**
 * Browser behaviour the app must not expose: the native context menu (replaced by our own), reload /
 * print / zoom / history accelerators, and navigation by dropped files or links.
 */
function useBrowserGuards() {
  useEffect(() => {
    const onContextMenu = (e: MouseEvent) => {
      e.preventDefault();
      const menus = useMenuStore.getState();
      menus.close();
      if (!(e.target instanceof Element)) return;
      if (rightClickPastes(e.target)) return; // terminal.rightClick = "paste": no menu
      const built = buildMenu(e.target, useStore.getState().platform);
      if (built.items.length > 0) menus.open({ x: e.clientX, y: e.clientY, items: built.items, restoreFocus: built.restoreFocus });
    };
    // Bubble phase and only for keys nobody handled: a focused terminal has consumed Ctrl+R, Ctrl+P ... already.
    const onKeyDown = (e: KeyboardEvent) => {
      if (!e.defaultPrevented && isBrowserAccelerator(e, useStore.getState().platform)) e.preventDefault();
    };
    document.addEventListener("contextmenu", onContextMenu, true);
    window.addEventListener("keydown", onKeyDown);
    const uninstallDrag = installDragDropGuard();
    return () => {
      document.removeEventListener("contextmenu", onContextMenu, true);
      window.removeEventListener("keydown", onKeyDown);
      uninstallDrag();
    };
  }, []);

  // Ctrl+wheel page zoom. WebView2 has zoom control off natively (IsZoomControlEnabled = false), so on Windows no
  // listener is needed; a non-passive wheel listener makes every wheel event wait for the main thread.
  const needsWheelGuard = useStore((s) => s.platform !== "windows");
  useEffect(() => {
    if (!needsWheelGuard) return;
    const onWheel = (e: WheelEvent) => {
      if (e.ctrlKey) e.preventDefault();
    };
    window.addEventListener("wheel", onWheel, { passive: false });
    return () => window.removeEventListener("wheel", onWheel);
  }, [needsWheelGuard]);
}

function BootError() {
  const { t } = useTranslation();
  const error = useStore((s) => s.bootError);
  if (!error) return null;
  return (
    <div className="boot-error" role="alert">
      <p>{t("app.bootFailed")}</p>
      <p className="boot-error__detail">{errorMessage(error)}</p>
      <Button
        variant="primary"
        onClick={() => {
          useStore.getState().setBootError(null);
          guard(startApp());
        }}
      >
        {t("app.retry")}
      </Button>
    </div>
  );
}

export function App() {
  const ready = useStore((s) => s.ready);
  const failed = useStore((s) => s.bootError !== null);
  useGlobalShortcuts();
  useBrowserGuards();
  return (
    <ErrorBoundary reportHost>
      {ready ? <Shell /> : failed ? <BootError /> : null}
      <DialogHost />
      <Notices />
      <ContextMenu />
    </ErrorBoundary>
  );
}
