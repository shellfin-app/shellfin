import { useRef, useState, type ReactNode, type RefObject } from "react";
import { useTranslation } from "react-i18next";
import { Group, Panel, Separator, type PanelImperativeHandle } from "react-resizable-panels";
import { IconButton } from "../../components/IconButton";
import { useStore } from "../../state/store";
import { loadLayoutPrefs, saveLayoutPrefs } from "../../state/layoutPrefs";
import { FileTree } from "../files/FileTree";
import { FilesBreadcrumb } from "../files/FilesBreadcrumb";
import { FilesToolbar } from "../files/FilesToolbar";
import { BookmarksPanel } from "../hosts/BookmarksPanel";

/** Height of a collapsed pane: just its header. */
const HEADER = 40;
const PANE_MIN = 96;

/** Chevron in a pane header: collapses the pane to its header and expands it again. */
function CollapseToggle({ panelRef, collapsed, name }: { panelRef: RefObject<PanelImperativeHandle | null>; collapsed: boolean; name: string }) {
  const { t } = useTranslation();
  return (
    <IconButton
      icon={collapsed ? "chevronRight" : "chevronDown"}
      iconSize={12}
      size={20}
      label={collapsed ? t("sidebar.expand", { name }) : t("sidebar.collapse", { name })}
      aria-expanded={!collapsed}
      onClick={() => {
        const panel = panelRef.current;
        if (!panel) return;
        if (panel.isCollapsed()) panel.expand();
        else panel.collapse();
      }}
    />
  );
}

function Files({ sessionId, lead }: { sessionId: string; lead?: ReactNode }) {
  const { t } = useTranslation();
  const root = useStore((s) => s.workspaces[sessionId]?.tree.root);
  return (
    <section className="sidebar__files" aria-label={t("files.title")}>
      <div className="sidebar__files-head">
        <div className="panel-header sidebar__files-title">
          <span className="sidebar__title-lead">
            {lead}
            <span className="sec">{t("files.title")}</span>
          </span>
          <FilesToolbar sessionId={sessionId} />
        </div>
        {root ? <FilesBreadcrumb sessionId={sessionId} root={root} /> : null}
      </div>
      <FileTree sessionId={sessionId} />
    </section>
  );
}

/**
 * Sidebar = bookmarks pane and/or (with an active session) the remote file tree (§8.4). With both panes shown the
 * split is resizable, each pane collapses to its header, and sizes and collapse state are remembered.
 */
export function Sidebar() {
  const { t } = useTranslation();
  const activeId = useStore((s) => s.activeSessionId);
  const activity = useStore((s) => s.activity);
  const bookmarksRef = useRef<PanelImperativeHandle | null>(null);
  const filesRef = useRef<PanelImperativeHandle | null>(null);
  // The layout of the previous run (initial sizes); the live collapse flags drive the chevrons.
  const [collapsed, setCollapsed] = useState(() => {
    const first = loadLayoutPrefs();
    return { bookmarks: first.bookmarksCollapsed, files: first.filesCollapsed };
  });
  // Without a session only the bookmarks exist (Welcome.dc).
  const view = activeId === null ? "bookmarks" : activity;

  if (view === "bookmarks" || activeId === null) {
    return (
      <aside className="sidebar" aria-label={t("sidebar.label")}>
        <div className="sidebar__top">
          <BookmarksPanel showHint={activeId === null} />
        </div>
      </aside>
    );
  }
  if (view === "files") {
    return (
      <aside className="sidebar" aria-label={t("sidebar.label")}>
        <Files sessionId={activeId} />
      </aside>
    );
  }

  // Re-read on every (re)mount of the split so a view-mode switch keeps the last dragged share.
  const initial = loadLayoutPrefs();
  return (
    <aside className="sidebar" aria-label={t("sidebar.label")}>
      <Group orientation="vertical" id="sidebar-split" className="sidebar__split">
        <Panel
          id="bookmarks"
          panelRef={bookmarksRef}
          defaultSize={initial.bookmarksCollapsed ? HEADER : `${initial.bookmarksShare}`}
          minSize={PANE_MIN}
          collapsible
          collapsedSize={HEADER}
          onResize={(size) => {
            const isCollapsed = size.inPixels > 0 && size.inPixels <= HEADER + 1;
            setCollapsed((c) => (c.bookmarks === isCollapsed ? c : { ...c, bookmarks: isCollapsed }));
            saveLayoutPrefs(
              isCollapsed ? { bookmarksCollapsed: true } : { bookmarksCollapsed: false, bookmarksShare: Math.round(size.asPercentage) },
              { debounce: true },
            );
          }}
        >
          <div className="sidebar__top sidebar__top--shared">
            <BookmarksPanel
              showHint={false}
              headerLead={<CollapseToggle panelRef={bookmarksRef} collapsed={collapsed.bookmarks} name={t("hosts.title")} />}
            />
          </div>
        </Panel>
        <Separator className="shell__separator shell__separator--h sidebar__separator" />
        <Panel
          id="files"
          panelRef={filesRef}
          defaultSize={initial.filesCollapsed ? HEADER : undefined}
          minSize={PANE_MIN}
          collapsible
          collapsedSize={HEADER}
          onResize={(size) => {
            const isCollapsed = size.inPixels > 0 && size.inPixels <= HEADER + 1;
            setCollapsed((c) => (c.files === isCollapsed ? c : { ...c, files: isCollapsed }));
            saveLayoutPrefs({ filesCollapsed: isCollapsed }, { debounce: true });
          }}
        >
          <Files
            sessionId={activeId}
            lead={<CollapseToggle panelRef={filesRef} collapsed={collapsed.files} name={t("files.title")} />}
          />
        </Panel>
      </Group>
    </aside>
  );
}
