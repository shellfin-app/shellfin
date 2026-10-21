import { useMemo, useRef, useState, type CSSProperties, type KeyboardEvent, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { Icon } from "../../components/Icon";
import { IconButton } from "../../components/IconButton";
import { ColorMark } from "../../components/ColorMark";
import { StatusDot, dotForPhase } from "../../components/StatusDot";
import { confirmDeleteDialog, deleteGroupDialog } from "../../lib/dialogs";
import { SEL_HOST_NAV } from "../../lib/domIds";
import { guard } from "../../lib/reportError";
import { displayName } from "../../lib/serverText";
import { useStore } from "../../state/store";
import { actionEnabled, runAction } from "../actions/registry";
import { useMenuStore, type MenuItem } from "../menu/menuStore";
import { requestOpenSession, sameTarget } from "../session/sessionActions";
import { colorVar } from "../../lib/bookmarkColor";
import type { BookmarksDto } from "../../ipc/types";
import { bookmarksOfGroup, finalIndexInSameList, sortedGroups, stepGroupTarget, stepTarget } from "./bookmarkOrder";
import { copyFromSshConfig, moveBookmark, moveGroup } from "./hostActions";
import {
  computeBookmarkDrop,
  computeGroupDrop,
  measureGroups,
  measureSections,
  usePointerReorder,
  type DragItem,
  type DropTarget,
} from "./pointerDrag";
import { bookmarkRows, sshConfigRows, type HostRow } from "./hostRows";
import "./hosts.css";

interface Section {
  id: string;
  /** Bookmark sections (ungrouped list, groups) accept drops: `null` = ungrouped, else the group id. Undefined = read-only ssh-config. */
  groupId?: string | null;
  title: string;
  mono: boolean;
  rows: HostRow[];
  showEmpty: boolean;
}

const BOOKMARK_IMPORT = "importBookmarks" as const;
const BOOKMARK_EXPORT = "exportBookmarks" as const;

/** Group headers and rows are the keyboard-navigable items of the list. */
const GROUP_PREFIX = "group-header:";

/**
 * Bookmarks list: "~/.ssh/config" group (read-only) + "My bookmarks" + user groups (Main.dc / Welcome.dc).
 * A row is selected by a single click and connects on double-click or Enter; the list is one tab stop and moves
 * with the arrow keys (roving tabindex): Up / Down / Home / End, Left / Right collapse and expand groups, F2 edits,
 * Delete deletes, typing jumps to a name. Right-click (or the context-menu key) opens the row menu (hostMenu.ts).
 */
export function BookmarksPanel({ showHint, headerLead }: { showHint: boolean; headerLead?: ReactNode }) {
  const { t } = useTranslation();
  const sshConfig = useStore((s) => s.sshConfig);
  const bookmarks = useStore((s) => s.bookmarks);
  const sessionsById = useStore((s) => s.sessionsById);
  const sessionOrder = useStore((s) => s.sessionOrder);
  const activeSessionId = useStore((s) => s.activeSessionId);
  const openDialog = useStore((s) => s.openDialog);
  const readOnly = bookmarks?.readOnly ?? false;
  const [collapsed, setCollapsed] = useState<Record<string, true>>({});
  const [selected, setSelected] = useState<string | null>(null);
  const [focusKey, setFocusKey] = useState<string | null>(null);
  const listRef = useRef<HTMLDivElement>(null);

  // `connection.readSshConfig = false` hides the ~/.ssh/config group (the backend then sends an empty snapshot anyway).
  const readSshConfig = useStore((s) => s.settings?.connection.readSshConfig !== false);
  const configRows = useMemo(() => (readSshConfig ? sshConfigRows(sshConfig?.hosts ?? []) : []), [sshConfig, readSshConfig]);
  const groups = useMemo(() => [...(bookmarks?.groups ?? [])].sort((a, b) => a.order - b.order), [bookmarks]);
  const ungrouped = useMemo(
    () => bookmarkRows((bookmarks?.bookmarks ?? []).filter((b) => b.groupId === null || !groups.some((g) => g.id === b.groupId))),
    [bookmarks, groups],
  );
  const configFile = sshConfig?.mainFile ?? "";

  const sections: Section[] = [
    ...(configRows.length > 0 ? [{ id: "ssh-config", title: configFile, mono: true, rows: configRows, showEmpty: true }] : []),
    { id: "my", groupId: null, title: t("hosts.myBookmarks"), mono: false, rows: ungrouped, showEmpty: true },
    ...groups.map((g) => ({
      id: `group:${g.id}`,
      groupId: g.id,
      title: g.name,
      mono: false,
      rows: bookmarkRows((bookmarks?.bookmarks ?? []).filter((b) => b.groupId === g.id)),
      showEmpty: false,
    })),
  ];
  // Navigation order = what is on screen.
  const navKeys = sections.flatMap((s) => [`${GROUP_PREFIX}${s.id}`, ...(collapsed[s.id] ? [] : s.rows.map((r) => r.key))]);
  const tabKey = focusKey !== null && navKeys.includes(focusKey) ? focusKey : (navKeys[0] ?? null);

  const toggle = (id: string, expand?: boolean) =>
    setCollapsed((c) => {
      const isCollapsed = c[id] === true;
      if (expand === isCollapsed || expand === undefined) {
        const next = { ...c };
        if (isCollapsed) delete next[id];
        else next[id] = true;
        return next;
      }
      return c;
    });

  const rowFor = (key: string): HostRow | undefined => sections.flatMap((s) => s.rows).find((r) => r.key === key);
  const sectionOf = (rowKey: string): Section | undefined => sections.find((s) => s.rows.some((r) => r.key === rowKey));
  const focusItem = (key: string) => {
    setFocusKey(key);
    [...(listRef.current?.querySelectorAll<HTMLElement>(SEL_HOST_NAV) ?? [])].find((el) => el.dataset["hostNav"] === key)?.focus();
  };

  const connect = (row: HostRow) => guard(requestOpenSession(row.target, row.label));

  const reorder = usePointerReorder({
    listRef,
    enabled: !readOnly,
    resolve: (item, y, list) => {
      if (item.kind === "group") {
        const drop = computeGroupDrop(y, measureGroups(list));
        return drop ? { kind: "group", ...drop } : null;
      }
      const drop = computeBookmarkDrop(y, measureSections(list));
      return drop ? { kind: "bookmark", ...drop } : null;
    },
    onDrop: (item, drop) => {
      const dto = useStore.getState().bookmarks;
      const move = dto ? resolveMove(dto, item, drop) : null;
      if (!move) return;
      if (move.kind === "group") guard(moveGroup(item.id, move.index));
      else guard(moveBookmark(item.id, move.groupId, move.index));
    },
  });
  const dragging = reorder.drag;
  const dragMove = dragging?.drop && bookmarks ? resolveMove(bookmarks, dragging.item, dragging.drop) : null;

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (!(e.target instanceof HTMLElement)) return;
    const key = e.target.dataset["hostNav"];
    if (!key) return;
    const index = navKeys.indexOf(key);
    const isHeader = key.startsWith(GROUP_PREFIX);
    const row = isHeader ? undefined : rowFor(key);
    const sectionId = isHeader ? key.slice(GROUP_PREFIX.length) : undefined;
    const go = (to: number) => {
      const k = navKeys[Math.max(0, Math.min(navKeys.length - 1, to))];
      if (k) focusItem(k);
    };
    if (e.altKey && (e.key === "ArrowUp" || e.key === "ArrowDown") && !readOnly) {
      e.preventDefault();
      const delta = e.key === "ArrowUp" ? -1 : 1;
      const dto = useStore.getState().bookmarks;
      if (dto && row?.target.kind === "bookmark") {
        const to = stepTarget(dto, row.target.id, delta);
        if (to) guard(moveBookmark(row.target.id, to.groupId, to.index));
      } else if (dto && sectionId?.startsWith("group:")) {
        const to = stepGroupTarget(dto, sectionId.slice(6), delta);
        if (to !== null) guard(moveGroup(sectionId.slice(6), to));
      }
      return;
    }
    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        go(index + 1);
        return;
      case "ArrowUp":
        e.preventDefault();
        go(index - 1);
        return;
      case "Home":
        e.preventDefault();
        go(0);
        return;
      case "End":
        e.preventDefault();
        go(navKeys.length - 1);
        return;
      case "ArrowRight":
        if (sectionId !== undefined) {
          e.preventDefault();
          if (collapsed[sectionId]) toggle(sectionId, true);
          else go(index + 1);
        }
        return;
      case "ArrowLeft":
        e.preventDefault();
        if (sectionId !== undefined) toggle(sectionId, false);
        else if (row) {
          const section = sectionOf(row.key);
          if (section) focusItem(`${GROUP_PREFIX}${section.id}`);
        }
        return;
      case "Enter":
        e.preventDefault();
        if (sectionId !== undefined) toggle(sectionId);
        else if (row) connect(row);
        return;
      case " ":
        e.preventDefault();
        if (sectionId !== undefined) toggle(sectionId);
        else setSelected(key);
        return;
      case "F2":
        if (row?.target.kind === "bookmark" && !readOnly) {
          e.preventDefault();
          openDialog({ kind: "bookmark", bookmarkId: row.target.id });
        }
        return;
      case "Delete":
        if (row?.target.kind === "bookmark" && !readOnly) {
          e.preventDefault();
          openDialog(confirmDeleteDialog(row.target.id));
        } else if (sectionId?.startsWith("group:") && !readOnly) {
          e.preventDefault();
          openDialog(deleteGroupDialog(sectionId.slice(6)));
        }
        return;
    }
    // Type-ahead over the visible rows.
    if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey && e.key !== " ") {
      const letter = e.key.toLowerCase();
      for (let step = 1; step <= navKeys.length; step += 1) {
        const k = navKeys[(index + step) % navKeys.length];
        const candidate = k ? rowFor(k) : undefined;
        if (k && candidate && displayName(candidate.label).toLowerCase().startsWith(letter)) {
          e.preventDefault();
          focusItem(k);
          return;
        }
      }
    }
  };

  const renderRow = (row: HostRow) => {
    const sessionId = sessionOrder.find((id) => {
      const entry = sessionsById[id];
      return entry !== undefined && sameTarget(entry.target, row.target);
    });
    const phase = sessionId ? sessionsById[sessionId]?.state.phase : undefined;
    const active = sessionId !== undefined && sessionId === activeSessionId;
    const bookmarkId = row.target.kind === "bookmark" ? row.target.id : null;
    const address = row.via ? t("hosts.via", { host: displayName(row.via) }) : displayName(row.address);
    const colorName = row.color ? t(`bookmarkColor.${row.color}`) : null;
    const isDragged = dragging?.item.kind === "bookmark" && bookmarkId !== null && dragging.item.id === bookmarkId;
    return (
      <div
        key={row.key}
        data-host-key={row.key}
        className={`host-row${active ? " host-row--active" : ""}${selected === row.key ? " host-row--selected" : ""}${row.color ? " host-row--colored" : ""}${isDragged ? " host-row--dragging" : ""}`}
        style={row.color ? ({ "--row-color": colorVar(row.color) } as CSSProperties) : undefined}
      >
        <button
          type="button"
          className="host-row__main"
          data-host-nav={row.key}
          tabIndex={tabKey === row.key ? 0 : -1}
          aria-label={sessionId ? t("hosts.goTo", { name: row.label }) : t("hosts.connect", { name: row.label })}
          aria-current={active ? "true" : undefined}
          aria-pressed={selected === row.key}
          title={`${displayName(row.label)}  ${address}${colorName ? ` (${colorName})` : ""}`}
          onPointerDown={(e) => {
            if (bookmarkId) reorder.start(e, { kind: "bookmark", id: bookmarkId, label: row.label });
          }}
          onClick={(e) => {
            if (reorder.consumeClick()) {
              e.preventDefault();
              return;
            }
            setSelected(row.key);
            setFocusKey(row.key);
          }}
          onDoubleClick={() => connect(row)}
          onFocus={() => setFocusKey(row.key)}
        >
          <StatusDot state={dotForPhase(phase)} label={phase ? t(`session.state.${phase}`) : t("status.notConnected")} />
          <ColorMark color={row.color} />
          <span className="host-row__name">{displayName(row.label)}</span>
          <span className="host-row__address mono">{address}</span>
        </button>
        <span className="host-row__actions">
          {bookmarkId ? (
            <>
              <IconButton
                icon="edit"
                size={22}
                iconSize={12}
                tabIndex={-1}
                label={t("hosts.edit", { name: row.label })}
                disabled={readOnly}
                onClick={() => openDialog({ kind: "bookmark", bookmarkId })}
              />
              <IconButton
                icon="trash"
                size={22}
                iconSize={12}
                tabIndex={-1}
                label={t("hosts.delete", { name: row.label })}
                disabled={readOnly}
                onClick={() => openDialog(confirmDeleteDialog(bookmarkId))}
              />
            </>
          ) : (
            <IconButton
              icon="copy"
              size={22}
              iconSize={12}
              tabIndex={-1}
              label={t("hosts.copyToBookmarks", { name: row.label })}
              disabled={readOnly}
              onClick={() => guard(copyFromSshConfig(row.label))}
            />
          )}
        </span>
      </div>
    );
  };

  const renderSection = (section: Section) => {
    const isCollapsed = collapsed[section.id] === true;
    const headerKey = `${GROUP_PREFIX}${section.id}`;
    return (
      <div
        className="host-group"
        key={section.id}
        data-drop-section={section.groupId !== undefined ? "" : undefined}
        data-drop-group={section.groupId !== undefined ? (section.groupId ?? "") : undefined}
        data-collapsed={isCollapsed ? "true" : undefined}
      >
        <button
          type="button"
          data-drop-header=""
          className={`host-group__header${dragging?.item.kind === "group" && `group:${dragging.item.id}` === section.id ? " host-group__header--dragging" : ""}`}
          onPointerDown={(e) => {
            if (section.groupId) reorder.start(e, { kind: "group", id: section.groupId, label: section.title });
          }}
          data-host-nav={headerKey}
          data-group-id={section.groupId || undefined}
          tabIndex={tabKey === headerKey ? 0 : -1}
          aria-expanded={!isCollapsed}
          title={displayName(section.title)}
          onClick={() => {
            if (!reorder.consumeClick()) toggle(section.id);
          }}
          onFocus={() => setFocusKey(headerKey)}
        >
          <Icon name={isCollapsed ? "chevronRight" : "chevronDown"} size={12} />
          <bdi className={section.mono ? "mono" : undefined}>{displayName(section.title)}</bdi>
          <span className="host-group__count">{section.rows.length}</span>
        </button>
        {isCollapsed ? null : section.rows.length === 0 ? (
          section.showEmpty ? (
            <div className="host-group__empty">
              <p>{t("hosts.noBookmarks")}</p>
              {section.id === "my" && !readOnly ? (
                <>
                  <Button icon="plus" onClick={() => openDialog({ kind: "bookmark", bookmarkId: null })}>
                    {t("hosts.add")}
                  </Button>
                  <p>{t("hosts.orQuickConnect")}</p>
                </>
              ) : null}
            </div>
          ) : null
        ) : (
          section.rows.map(renderRow)
        )}
      </div>
    );
  };

  return (
    <div className="bookmarks-panel">
      <div className="panel-header">
        <span className="sidebar__title-lead">
          {headerLead}
          <span className="sec">{t("hosts.title")}</span>
        </span>
        {/* One flex child for both buttons: with space-between a third child would push "+" to the middle. */}
        <div className="panel-header__actions">
          <IconButton
            icon="plus"
            size={26}
            label={t("hosts.add")}
            disabled={readOnly}
            onClick={() => openDialog({ kind: "bookmark", bookmarkId: null })}
          />
          <IconButton
            icon="more"
            size={26}
            label={t("hosts.moreActions")}
            aria-haspopup="menu"
            onClick={(e) => {
              const button = e.currentTarget;
              const rect = button.getBoundingClientRect();
              const items: MenuItem[] = [BOOKMARK_IMPORT, BOOKMARK_EXPORT].map((id) => ({
                id,
                label: t(`action.${id}`),
                disabled: !actionEnabled(id),
                onSelect: () => runAction(id),
              }));
              useMenuStore.getState().open({ x: rect.left, y: rect.bottom + 2, items, restoreFocus: () => button.focus() });
            }}
          />
        </div>
      </div>
      <div
        ref={listRef}
        className={`bookmarks-panel__list${dragging ? " bookmarks-panel__list--dragging" : ""}`}
        role="group"
        aria-label={t("hosts.list")}
        onKeyDown={onKeyDown}
      >
        {sections.map(renderSection)}
        {dragging && dragMove && dragging.lineTop !== null ? (
          <div
            className="drop-line"
            data-testid="drop-line"
            aria-hidden="true"
            style={{ top: dragging.lineTop }}
          />
        ) : null}
      </div>
      {dragging ? (
        <div className="drag-ghost" aria-hidden="true" style={{ left: dragging.x + 12, top: dragging.y + 8 }}>
          {displayName(dragging.item.label)}
        </div>
      ) : null}
      {readOnly ? (
        <p className="bookmarks-panel__hint" role="status">
          {t("hosts.readOnlyHint")}
        </p>
      ) : null}
      {showHint && configRows.length > 0 ? <p className="bookmarks-panel__hint">{t("hosts.sshConfigHint", { file: configFile })}</p> : null}
    </div>
  );
}

/**
 * What a drop means for the backend (`index` = final position in the target list), or null when nothing changes
 * (dropped where it already is, or onto something that is not a bookmark list).
 */
export function resolveMove(
  dto: BookmarksDto,
  item: DragItem,
  drop: DropTarget,
): { kind: "group"; index: number } | { kind: "bookmark"; groupId: string | null; index: number } | null {
  if (item.kind === "group") {
    if (drop.kind !== "group") return null;
    const from = sortedGroups(dto).findIndex((g) => g.id === item.id);
    if (from < 0) return null;
    const index = finalIndexInSameList(from, drop.slot);
    return index === from ? null : { kind: "group", index };
  }
  if (drop.kind !== "bookmark") return null;
  const bookmark = dto.bookmarks.find((b) => b.id === item.id);
  if (!bookmark) return null;
  const list = bookmarksOfGroup(dto, drop.groupId);
  const from = list.findIndex((b) => b.id === item.id);
  if (from < 0) return { kind: "bookmark", groupId: drop.groupId, index: drop.slot };
  const index = finalIndexInSameList(from, drop.slot);
  return index === from ? null : { kind: "bookmark", groupId: drop.groupId, index };
}
