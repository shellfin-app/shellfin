import { useCallback, useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type WheelEvent } from "react";
import { useTranslation } from "react-i18next";
import { Icon } from "../../components/Icon";
import { ColorMark } from "../../components/ColorMark";
import { IconButton } from "../../components/IconButton";
import { InlineRename } from "../../components/InlineRename";
import { StatusDot, dotForPhase } from "../../components/StatusDot";
import { colorVar } from "../../lib/bookmarkColor";
import { bookmarkDialog } from "../../lib/dialogs";
import type { SessionEntry } from "../../state/slices/sessions";
import { useStore } from "../../state/store";
import { useMenuStore, type MenuItem } from "../menu/menuStore";
import { requestCloseSession } from "../session/sessionActions";
import { sessionTitle } from "../session/sessionTitle";

/** Top-level connection tabs: Home + one per session, a scrolling strip, the "all sessions" list and "new connection". */
export function ConnectionTabs() {
  const { t } = useTranslation();
  const order = useStore((s) => s.sessionOrder);
  const byId = useStore((s) => s.sessionsById);
  const activeId = useStore((s) => s.activeSessionId);
  const prompts = useStore((s) => s.prompts);
  const setActiveSession = useStore((s) => s.setActiveSession);
  const openDialog = useStore((s) => s.openDialog);
  const renaming = useStore((s) => s.renamingSession);
  const setRenaming = useStore((s) => s.setRenamingSession);
  const renameSession = useStore((s) => s.renameSession);
  const scrollRef = useRef<HTMLDivElement>(null);
  const listButtonRef = useRef<HTMLButtonElement>(null);
  const [edges, setEdges] = useState({ left: false, right: false });

  const tabTitle = (entry: SessionEntry): string => sessionTitle(entry, t);

  const measure = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    const left = el.scrollLeft > 1;
    const right = el.scrollLeft + el.clientWidth < el.scrollWidth - 1;
    setEdges((prev) => (prev.left === left && prev.right === right ? prev : { left, right }));
  }, []);

  // Fades follow the scroll position and the available width.
  useLayoutEffect(() => {
    measure();
    const el = scrollRef.current;
    if (!el || typeof ResizeObserver === "undefined") return undefined;
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [measure, order.length]);

  // The active tab is always scrolled into view (new session, Mod+1..9, the list).
  useEffect(() => {
    const el = scrollRef.current?.querySelector<HTMLElement>(".conn-tab--active");
    el?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
    measure();
  }, [activeId, order.length, measure]);

  const onWheel = (e: WheelEvent<HTMLDivElement>) => {
    const el = scrollRef.current;
    if (!el || e.deltaY === 0 || e.deltaX !== 0) return;
    el.scrollLeft += e.deltaY; // a vertical wheel scrolls the (horizontal) strip
  };

  const openList = () => {
    const button = listButtonRef.current;
    if (!button) return;
    const rect = button.getBoundingClientRect();
    const items: MenuItem[] = [
      {
        id: "home",
        label: t("shell.home"),
        checked: activeId === null,
        onSelect: () => setActiveSession(null),
      },
      ...order.flatMap((id): MenuItem[] => {
        const entry = byId[id];
        if (!entry) return [];
        return [
          {
            id,
            label: tabTitle(entry),
            checked: id === activeId,
            leading: (
              <>
                <StatusDot state={dotForPhase(entry.state.phase)} label={t(`session.state.${entry.state.phase}`)} />
                <ColorMark color={entry.color} />
              </>
            ),
            onSelect: () => setActiveSession(id),
          },
        ];
      }),
    ];
    useMenuStore.getState().open({ x: rect.left, y: rect.bottom + 2, items, restoreFocus: () => button.focus() });
  };

  return (
    <div className="conn-tabs-wrap">
      <div
        ref={scrollRef}
        className={`conn-tabs${edges.left ? " conn-tabs--fade-left" : ""}${edges.right ? " conn-tabs--fade-right" : ""}`}
        role="tablist"
        aria-label={t("shell.connections")}
        onScroll={measure}
        onWheel={onWheel}
      >
        <div className={`conn-tab${activeId === null ? " conn-tab--active" : ""} conn-tab--home`}>
          <button
            type="button"
            role="tab"
            aria-selected={activeId === null}
            className="conn-tab__main"
            onClick={() => setActiveSession(null)}
          >
            <Icon name="home" size={13} />
            {t("shell.home")}
          </button>
        </div>
        {order.map((id) => {
          const entry = byId[id];
          if (!entry) return null;
          const selected = id === activeId;
          return (
            <div
              key={id}
              data-session-tab={id}
              data-color={entry.color ?? undefined}
              className={`conn-tab${selected ? " conn-tab--active" : ""}${entry.color ? " conn-tab--colored" : ""}`}
              style={entry.color ? ({ "--tab-color": colorVar(entry.color) } as CSSProperties) : undefined}
            >
              {renaming === id ? (
                <InlineRename
                  className="conn-tab__rename"
                  label={t("menu.renameConnection")}
                  initial={entry.tabName ?? ""}
                  onDone={(name) => {
                    setRenaming(null);
                    if (name !== undefined) renameSession(id, name);
                  }}
                />
              ) : (
                <button
                  type="button"
                  role="tab"
                  aria-selected={selected}
                  className="conn-tab__main"
                  title={tabTitle(entry) + (entry.color ? ` (${t(`bookmarkColor.${entry.color}`)})` : "")}
                  onClick={() => setActiveSession(id)}
                  onDoubleClick={() => setRenaming(id)}
                >
                  <StatusDot state={dotForPhase(entry.state.phase)} label={t(`session.state.${entry.state.phase}`)} />
                  <ColorMark color={entry.color} />
                  <bdi className="conn-tab__name">{tabTitle(entry)}</bdi>
                  {prompts[id] && !selected ? (
                    <span className="conn-tab__badge" role="img" aria-label={t("session.needsAnswer")} title={t("session.needsAnswer")} />
                  ) : null}
                </button>
              )}
              <IconButton
                icon="close"
                iconSize={12}
                size={20}
                label={t("shell.closeConnection", { name: tabTitle(entry) })}
                onClick={() => requestCloseSession(id)}
              />
            </div>
          );
        })}
      </div>
      <IconButton
        icon="plus"
        size={32}
        label={t("shell.newConnection")}
        onClick={() => openDialog(bookmarkDialog(null))}
      />
      <button
        ref={listButtonRef}
        type="button"
        className="icon-button conn-tabs__list"
        style={{ width: 28, height: 32 }}
        aria-label={t("shell.allConnections")}
        title={t("shell.allConnections")}
        aria-haspopup="menu"
        onClick={openList}
      >
        <Icon name="chevronDown" size={14} />
      </button>
    </div>
  );
}
