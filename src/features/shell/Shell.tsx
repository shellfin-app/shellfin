import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Group, Panel, Separator, type PanelImperativeHandle } from "react-resizable-panels";
import { Button } from "../../components/Button";
import { Icon } from "../../components/Icon";
import { IconButton } from "../../components/IconButton";
import { ServerText } from "../../components/ServerText";
import { loadLayoutPrefs, saveLayoutPrefs } from "../../state/layoutPrefs";
import { useStore } from "../../state/store";
import { focusRegion } from "../actions/focusRegions";
import { toggleMaximizePanel } from "../actions/registry";
import { ReconnectOverlay } from "../session/ReconnectOverlay";
import { requestReconnectWithChanges } from "../session/sessionActions";
import { SessionErrorPanel } from "../session/SessionErrorPanel";
import { ConnectingCard } from "../session/ConnectingCard";
import { EditorArea } from "../editor/EditorArea";
import { TerminalPanel } from "../terminal/TerminalPanel";
import { WelcomeScreen } from "../welcome/WelcomeScreen";
import { ActivityBar } from "./ActivityBar";
import { Sidebar } from "./Sidebar";
import { StatusBar } from "./StatusBar";
import { TitleBar } from "./TitleBar";
import { MAIN_MIN, SIDEBAR_MIN, clampSidebarWidth, sidebarMaxPx } from "./layoutLimits";
import { createSidebarWidthTracker, sidebarCorrection } from "./sidebarWidth";
import "./shell.css";

const TERMINAL_MIN = 120;
const TERMINAL_DEFAULT = 300;

/** The window width, kept current: the sidebar's upper bound is a share of it. */
function useWindowWidth(): number {
  const [width, setWidth] = useState(() => window.innerWidth);
  useEffect(() => {
    const onResize = () => setWidth(window.innerWidth);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);
  return width;
}

function SessionWorkbench({ sessionId }: { sessionId: string }) {
  const { t } = useTranslation();
  const phase = useStore((s) => s.sessionsById[sessionId]?.state.phase);
  const hasInfo = useStore((s) => s.sessionsById[sessionId]?.info !== undefined);
  const banner = useStore((s) => s.sessionsById[sessionId]?.banner);
  const dismissBanner = useStore((s) => s.dismissBanner);
  const needsReconnect = useStore((s) => s.sessionsById[sessionId]?.needsReconnect === true);
  const panelOpen = useStore((s) => s.workspaces[sessionId]?.panelOpen ?? true);
  const settingsHeight = useStore((s) => s.settings?.ui.terminalPanelHeight ?? TERMINAL_DEFAULT);
  // Read when the panel (re)appears so hide/show restores the last dragged height, not the one from app start.
  const savedHeight = useMemo(() => (panelOpen ? loadLayoutPrefs().terminalHeight : null), [panelOpen]);
  const terminalHeight = savedHeight ?? settingsHeight;
  const terminalCount = useStore((s) => s.workspaces[sessionId]?.terminalTabs.length ?? 0);
  const maximized = useStore((s) => s.workspaces[sessionId]?.panelMaximized === true);
  const setPanelMaximized = useStore((s) => s.setPanelMaximized);
  const editorRef = useRef<PanelImperativeHandle | null>(null);
  // The terminal height before it was maximized (the pixel value saved while the editor is hidden would be the whole
  // area): written back when the editor collapses, so a later restore and the next start use the height from before.
  const normalHeight = useRef<number | null>(null);

  // The store flag drives the editor panel: the editor is collapsed (hidden, still mounted), never closed.
  useEffect(() => {
    const editor = editorRef.current;
    if (!editor) return;
    const want = panelOpen && maximized;
    if (want && !editor.isCollapsed()) editor.collapse();
    else if (!want && editor.isCollapsed()) editor.expand();
  }, [panelOpen, maximized, hasInfo, sessionId]);

  if (!phase) return null;

  // Before the first successful connection there is no workspace: show progress or the failure (§5.8).
  if (!hasInfo) {
    if (phase === "disconnected") return <SessionErrorPanel sessionId={sessionId} />;
    return <ConnectingCard sessionId={sessionId} />;
  }

  return (
    <div className="workbench">
      {banner ? (
        <div className="session-banner" role="status">
          <span className="session-banner__text">
            <ServerText text={banner.text} limit={8192} keepNewlines />
          </span>
          <IconButton icon="close" iconSize={12} size={22} label={t("session.dismissBanner")} onClick={() => dismissBanner(sessionId)} />
        </div>
      ) : null}
      {needsReconnect ? (
        <div className="session-hint" role="status" title={t("session.reconnectToApplyHint")}>
          <Icon name="refresh" size={13} />
          <span className="session-hint__text">{t("session.reconnectToApply")}</span>
          <Button onClick={() => requestReconnectWithChanges(sessionId)}>{t("session.reconnectToApplyButton")}</Button>
        </div>
      ) : null}
      <Group orientation="vertical" id="workbench" className="shell__group">
        {/* Collapsible to 0: dragging the splitter to the top snaps the terminal panel to "maximized". */}
        <Panel
          id="editor"
          panelRef={editorRef}
          minSize={TERMINAL_MIN}
          collapsible
          collapsedSize={0}
          onResize={() => {
            if (!panelOpen) return;
            const collapsed = editorRef.current?.isCollapsed() === true;
            if (collapsed === maximized) return;
            if (collapsed && normalHeight.current !== null) saveLayoutPrefs({ terminalHeight: normalHeight.current });
            setPanelMaximized(sessionId, collapsed);
          }}
        >
          <EditorArea sessionId={sessionId} />
        </Panel>
        {panelOpen ? (
          // Double click maximizes / restores (the library's default, "reset to the initial size", is turned off).
          <Separator className="shell__separator shell__separator--h" disableDoubleClick onDoubleClick={toggleMaximizePanel} />
        ) : null}
        {panelOpen ? (
          <Panel
            id="terminal"
            defaultSize={terminalHeight}
            minSize={TERMINAL_MIN}
            groupResizeBehavior="preserve-pixel-size"
            onResize={(size) => {
              if (size.inPixels <= 0 || editorRef.current?.isCollapsed() === true) return;
              normalHeight.current = Math.round(size.inPixels);
              saveLayoutPrefs({ terminalHeight: normalHeight.current }, { debounce: true });
            }}
          >
            <TerminalPanel sessionId={sessionId} />
          </Panel>
        ) : null}
      </Group>
      {panelOpen ? null : (
        // A hidden terminal panel leaves a strip behind that brings it back (and counts the terminals).
        <button
          type="button"
          className="terminals-bar"
          aria-expanded={false}
          aria-label={t("terminal.showPanel", { count: terminalCount })}
          onClick={() => focusRegion("terminal")}
        >
          <Icon name="chevronUp" size={12} />
          <span className="sec">{t("terminal.collapsed", { count: terminalCount })}</span>
        </button>
      )}
      {panelOpen ? null : <ReconnectOverlay sessionId={sessionId} fill />}
    </div>
  );
}

/** App shell: title bar, activity bar, resizable sidebar + main area, status bar (§8.4). */
export function Shell() {
  const activeId = useStore((s) => s.activeSessionId);
  const sidebarOpen = useStore((s) => s.sidebarOpen);
  const statusBarOpen = useStore((s) => s.statusBarOpen);
  const activity = useStore((s) => s.activity);
  const windowWidth = useWindowWidth();
  const sidebarMax = sidebarMaxPx(windowWidth);
  const sidebarRef = useRef<PanelImperativeHandle | null>(null);
  // Only widths the user chose are stored; the clamp to the current window happens here, at render time.
  const widths = useMemo(() => createSidebarWidthTracker(loadLayoutPrefs().sidebarWidth), []);
  // The user's width clamped to the window it is restored into (it may come from a larger window).
  const sidebarWidth = useMemo(
    () => (sidebarOpen ? clampSidebarWidth(widths.userWidth(), window.innerWidth) : 0),
    [sidebarOpen, widths],
  );

  // The shown width is always the user's width clamped to the current window: a shrinking window takes the sidebar
  // with it (the main area keeps its usable width), a widening one gives the user's width back, also in one step
  // (maximize, Snap). The panel library keeps percentages when its group resizes, so the shell corrects it on the next
  // frame after every window or group resize and after every size change the user did not make. None of this is
  // stored (`widths.onResize` ignores resizes the user did not make). At most a few corrections per resize, so a
  // size the library refuses can never loop.
  const bodyRef = useRef<HTMLDivElement>(null);
  const syncFrame = useRef(0);
  const corrections = useRef(0);
  const scheduleSync = useCallback(() => {
    if (syncFrame.current !== 0) return;
    syncFrame.current = requestAnimationFrame(() => {
      syncFrame.current = 0;
      const panel = sidebarRef.current;
      if (!panel || corrections.current >= 4) return;
      const want = sidebarCorrection(panel.getSize().inPixels, widths.userWidth(), window.innerWidth);
      if (want === null) return;
      corrections.current += 1;
      panel.resize(want);
    });
  }, [widths]);
  useEffect(() => {
    corrections.current = 0;
    scheduleSync();
  }, [windowWidth, scheduleSync]);
  useEffect(() => {
    const body = bodyRef.current;
    if (!body || typeof ResizeObserver === "undefined") return undefined;
    const observer = new ResizeObserver(() => {
      corrections.current = 0;
      scheduleSync();
    });
    observer.observe(body);
    return () => {
      observer.disconnect();
      if (syncFrame.current !== 0) cancelAnimationFrame(syncFrame.current);
      syncFrame.current = 0;
    };
  }, [scheduleSync]);

  // User resizes of the sidebar: a press on its separator (until release) or a key on it. Window capture listeners
  // see them before the panel library handles them.
  useEffect(() => {
    const onSeparator = (e: Event) => e.target instanceof Element && e.target.closest(".shell__separator--v") !== null;
    const down = (e: Event) => {
      if (onSeparator(e)) widths.pointerDown();
    };
    const up = () => widths.pointerUp();
    const key = (e: Event) => {
      if (onSeparator(e)) widths.key();
    };
    window.addEventListener("pointerdown", down, true);
    window.addEventListener("pointerup", up, true);
    window.addEventListener("pointercancel", up, true);
    window.addEventListener("keydown", key, true);
    return () => {
      window.removeEventListener("pointerdown", down, true);
      window.removeEventListener("pointerup", up, true);
      window.removeEventListener("pointercancel", up, true);
      window.removeEventListener("keydown", key, true);
    };
  }, [widths]);

  // Layout choices are remembered between runs (local UI state, not settings: no contract field exists for them).
  useEffect(() => saveLayoutPrefs({ sidebarOpen, statusBarOpen, activity }), [sidebarOpen, statusBarOpen, activity]);

  return (
    <div className="shell">
      <TitleBar />
      <div ref={bodyRef} className="shell__body">
        <ActivityBar />
        <Group orientation="horizontal" id="shell" className="shell__group">
          {sidebarOpen ? (
            <Panel
              id="sidebar"
              panelRef={sidebarRef}
              defaultSize={sidebarWidth}
              minSize={SIDEBAR_MIN}
              maxSize={sidebarMax}
              groupResizeBehavior="preserve-pixel-size"
              onResize={(size) => {
                const chosen = widths.onResize(size.inPixels);
                if (chosen !== null) saveLayoutPrefs({ sidebarWidth: chosen }, { debounce: true });
                else scheduleSync(); // a layout change (group resize): back to the clamped user width
              }}
            >
              <Sidebar />
            </Panel>
          ) : null}
          {sidebarOpen ? <Separator className="shell__separator shell__separator--v" /> : null}
          <Panel id="main" minSize={MAIN_MIN}>
            <main className="shell__main">{activeId ? <SessionWorkbench key={activeId} sessionId={activeId} /> : <WelcomeScreen />}</main>
          </Panel>
        </Group>
      </div>
      {statusBarOpen ? <StatusBar /> : null}
    </div>
  );
}
