import { observeElementRect, useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useId, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent } from "react";
import { useTranslation } from "react-i18next";
import { useShallow } from "zustand/react/shallow";
import { Icon } from "../../components/Icon";
import { goToFolderDialog } from "../../lib/dialogs";
import { SEL_TREE_ITEM } from "../../lib/domIds";
import { isImeKeyEvent } from "../../lib/ime";
import { normKey } from "../../lib/keybindings";
import { enterable, identityOf, writability } from "../../lib/permissions";
import { dirname } from "../../lib/remotePath";
import { guard, reportError } from "../../lib/reportError";
import { displayName, displayServerText } from "../../lib/serverText";
import { groupText, modeString, ownerText } from "../../lib/entryDetails";
import { errorMessage } from "../../i18n/errorMessage";
import { formatBytes, formatFileDate, formatFileDateShort, formatList } from "../../i18n/format";
import type { FileEntry } from "../../ipc/types";
import { downloadFile } from "../transfers/transferActions";
import { toIpcError } from "../../ipc/call";
import { useStore } from "../../state/store";
import { openFile } from "../editor/editorActions";
import {
  cancelTreeEdit,
  createEntry,
  isOpenable,
  lookupEntry,
  nameProblem,
  renameEntry,
  requestDelete,
  startRename,
  toggleDirectory,
  type NameProblem,
} from "./fileActions";
import { entryRows, flattenTree, type EntryRow, type TreeRow } from "./flattenTree";
import { FileTreeHeader } from "./FileTreeHeader";
import { useMinuteTick } from "./useMinuteTick";
import "./files.css";

const ROW_HEIGHT = 28;
const INDENT_BASE = 10;
const INDENT_STEP = 18;
/** Width of the chevron slot: files align with sibling folder names. */
const CHEVRON_SLOT = 20;
/** Height assumed while the scroll container reports none (first paint, tests without layout). */
const FALLBACK_HEIGHT = 560;

/** Inline name input for a new entry or a rename. Enter commits, Escape or leaving the input cancels. */
function TreeNameInput({
  initial,
  label,
  onCommit,
  onCancel,
  onEscape,
}: {
  initial: string;
  label: string;
  onCommit: (name: string) => Promise<void>;
  /** The input was left (click elsewhere): just stop editing. */
  onCancel: () => void;
  /** Escape: stop editing and give the keyboard back to the tree. */
  onEscape: () => void;
}) {
  const { t } = useTranslation();
  const ref = useRef<HTMLInputElement>(null);
  const [value, setValue] = useState(initial);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<NameProblem | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  useEffect(() => {
    const input = ref.current;
    if (!input) return;
    input.focus();
    const dot = initial.lastIndexOf(".");
    input.setSelectionRange(0, dot > 0 ? dot : initial.length); // rename: select the name, keep the extension
  }, [initial]);

  const commit = () => {
    const found = nameProblem(value);
    setProblem(found);
    if (found || busy) return;
    setBusy(true);
    setFailure(null);
    onCommit(value).catch((e: unknown) => {
      const error = toIpcError(e).error;
      setFailure(errorMessage(error));
      reportError(e);
      setBusy(false);
      ref.current?.focus();
    });
  };

  let message: string | null = failure;
  switch (problem) {
    case "empty":
      message = t("files.nameProblem.empty");
      break;
    case "slash":
      message = t("files.nameProblem.slash");
      break;
    case "dots":
      message = t("files.nameProblem.dots");
      break;
    case "nul":
      message = t("files.nameProblem.nul");
      break;
    case null:
      break;
  }

  return (
    <>
      <input
        ref={ref}
        className="file-row__input"
        value={value}
        aria-label={label}
        aria-invalid={message !== null}
        spellCheck={false}
        autoComplete="off"
        disabled={busy}
        onChange={(e) => {
          setValue(e.target.value);
          setProblem(null);
          setFailure(null);
        }}
        onKeyDown={(e) => {
          e.stopPropagation(); // the tree's own keys (arrows, Delete, F2) must not act while typing
          if (isImeKeyEvent(e)) return; // Enter / Escape belong to the IME conversion
          if (e.key === "Enter") {
            e.preventDefault();
            commit();
          } else if (e.key === "Escape") {
            e.preventDefault();
            onEscape();
          }
        }}
        onBlur={() => {
          if (!busy) onCancel();
        }}
      />
      {message ? (
        <span className="sr-only" role="alert">
          {message}
        </span>
      ) : null}
    </>
  );
}

/** `rwxr-xr-x owner group` for the row (`files.showPermissions`); numeric ids when the server gave no names. */
function permissionsSummary(e: FileEntry): string | null {
  const parts = [modeString(e), ownerText(e), groupText(e)].filter((p): p is string => p !== null);
  return parts.length === 0 ? null : displayServerText(parts.join(" "), 160);
}

function rowOpenable(row: EntryRow): boolean {
  return isOpenable({ kind: row.kind, linkTargetKind: row.linkTargetKind, lossyName: row.lossy });
}

function rowId(prefix: string, row: TreeRow): string {
  return `${prefix}-${row.type === "entry" ? encodeURIComponent(row.path) : row.key}`;
}

/** Remote file tree (Main.dc "Files"): virtualized, lazy, keyboard accessible, with inline rename / create. */
export function FileTree({ sessionId }: { sessionId: string }) {
  const { t, i18n } = useTranslation();
  const prefix = useId();
  const tree = useStore((s) => s.workspaces[sessionId]?.tree);
  const showHidden = useStore((s) => s.settings?.files.showHidden ?? false);
  // The dirty file paths of this session (strings: shallow-equal while nothing becomes dirty or clean).
  const dirtyList = useStore(
    useShallow((s) => Object.values(s.buffers).flatMap((b) => (b.sessionId === sessionId && b.dirty ? [b.path] : []))),
  );
  const dropTarget = useStore((s) => s.dropTarget);
  const selectTreeEntry = useStore((s) => s.selectTreeEntry);
  const filesStatus = useStore((s) => s.workspaces[sessionId]?.filesStatus ?? "ok");
  const sftpAvailable = useStore((s) => s.sessionsById[sessionId]?.info?.sftpAvailable ?? true);
  const phase = useStore((s) => s.sessionsById[sessionId]?.state.phase);
  const mac = useStore((s) => s.platform === "macos");
  const scrollRef = useRef<HTMLDivElement>(null);
  const sessionInfo = useStore((s) => s.sessionsById[sessionId]?.info);
  const identity = useMemo(() => identityOf(sessionInfo), [sessionInfo]);

  const foldersFirst = useStore((s) => s.settings?.files.foldersFirst ?? true);
  const showPermissions = useStore((s) => s.settings?.files.showPermissions ?? false);
  const sizeUnits = useStore((s) => s.settings?.files.sizeUnits ?? "binary");
  const dateFormat = useStore((s) => s.settings?.files.dateFormat ?? "relative");
  const downloadOnDoubleClick = useStore((s) => s.settings?.files.doubleClick === "download");
  // Size and date columns (§8.11): the setting; the sidebar width decides what shows (files.css).
  const columns = useStore((s) => s.settings?.files.columns ?? true);
  const sort = useStore((s) => s.filesSort);
  const now = useMinuteTick(columns);
  const rows = useMemo(
    () => (tree ? flattenTree(tree, { showHidden, foldersFirst, locale: i18n.language, sort }) : []),
    [tree, showHidden, foldersFirst, i18n.language, sort],
  );
  const hasTree = tree !== undefined;
  const [scrollbar, setScrollbar] = useState(0);
  // The header's right padding follows the tree's vertical scrollbar (`scrollbar-gutter` is not reliable everywhere).
  useEffect(() => {
    const el = scrollRef.current;
    if (!columns || !el || typeof ResizeObserver === "undefined") return undefined;
    const observer = new ResizeObserver(() => setScrollbar(Math.max(0, el.offsetWidth - el.clientWidth)));
    observer.observe(el);
    return () => observer.disconnect();
  }, [columns, hasTree, sftpAvailable, filesStatus]);
  const dirtyPaths = useMemo(() => new Set(dirtyList), [dirtyList]);

  // eslint-disable-next-line react-hooks/incompatible-library -- TanStack Virtual returns unmemoizable functions; used as documented
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 12,
    initialRect: { width: 280, height: FALLBACK_HEIGHT },
    // Without layout (first paint, jsdom) the container reports a height of 0: assume a usable one.
    observeElementRect: (instance, cb) => observeElementRect(instance, (rect) => cb(rect.height > 0 ? rect : { width: rect.width, height: FALLBACK_HEIGHT })),
    getItemKey: (index) => {
      const row = rows[index];
      return row ? (row.type === "entry" ? row.path : row.key) : index;
    },
  });

  const selection = tree?.selection ?? null;
  const selectedIndex = selection === null ? -1 : rows.findIndex((r) => r.type === "entry" && r.path === selection);
  const edit = tree?.edit ?? null;
  const editIndex = edit ? rows.findIndex((r) => (edit.kind === "new" ? r.type === "new" : r.type === "entry" && r.path === edit.path)) : -1;

  // Keep the selected row and the row being edited in view (keyboard navigation, new entries).
  useEffect(() => {
    if (selectedIndex >= 0) virtualizer.scrollToIndex(selectedIndex, { align: "auto" });
  }, [selectedIndex, virtualizer]);
  useEffect(() => {
    if (editIndex >= 0) virtualizer.scrollToIndex(editIndex, { align: "auto" });
  }, [editIndex, virtualizer]);

  if (!sftpAvailable) return <p className="file-tree__note">{t("files.sftpUnavailable")}</p>;
  if (filesStatus === "failed") return <p className="file-tree__note">{t("files.loadFailed")}</p>;
  if (!tree) return null;

  const online = phase === "connected";
  const entries = entryRows(rows);

  /** Tooltip lines: size and modification time (following the size and date settings), then the permissions. */
  const detailLines = (e: FileEntry): string[] => {
    const lines: string[] = [];
    if (e.kind === "file" && e.size !== null) lines.push(t("files.meta.size", { size: formatBytes(e.size, sizeUnits) }));
    const modified = e.mtime === null ? "" : formatFileDate(e.mtime, columns ? now : Date.now(), dateFormat);
    if (modified !== "") lines.push(t("files.meta.modified", { time: modified }));
    if (showPermissions) {
      const mode = modeString(e);
      const owner = ownerText(e);
      const group = groupText(e);
      if (mode !== null) lines.push(t("files.meta.permissions", { mode }));
      if (owner !== null) lines.push(t("files.meta.owner", { owner: displayServerText(owner, 64) }));
      if (group !== null) lines.push(t("files.meta.group", { group: displayServerText(group, 64) }));
    }
    return lines;
  };

  const activate = (row: EntryRow, focusEditor: boolean) => {
    selectTreeEntry(sessionId, row.path);
    if (row.lossy) return;
    if (row.dirLike) guard(toggleDirectory(sessionId, row.path));
    else if (rowOpenable(row)) guard(openFile(sessionId, row.path, { focus: focusEditor }));
  };
  const select = (row: EntryRow | undefined) => {
    if (row) selectTreeEntry(sessionId, row.path);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.target instanceof HTMLInputElement) return;
    // Ctrl+L / Cmd+L: "Go to folder…" (layout-independent, like the global shortcuts).
    const goTo = (mac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey) && !e.altKey && !e.shiftKey && normKey(e) === "l";
    if (goTo) {
      e.preventDefault();
      if (online) useStore.getState().openDialog(goToFolderDialog(sessionId));
      return;
    }
    if (e.altKey || e.ctrlKey || e.metaKey) return;
    const index = entries.findIndex((r) => r.path === selection);
    const current = entries[index];
    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        select(entries[Math.min(entries.length - 1, index + 1)]);
        break;
      case "ArrowUp":
        e.preventDefault();
        select(entries[Math.max(0, index - 1)]);
        break;
      case "Home":
        e.preventDefault();
        select(entries[0]);
        break;
      case "End":
        e.preventDefault();
        select(entries[entries.length - 1]);
        break;
      case "ArrowRight":
        if (!current) break;
        e.preventDefault();
        if (current.dirLike && !current.lossy && !current.expanded) guard(toggleDirectory(sessionId, current.path));
        else if (current.expanded) {
          const next = entries[index + 1];
          if (next && next.depth === current.depth + 1) select(next);
        }
        break;
      case "ArrowLeft":
        if (!current) break;
        e.preventDefault();
        if (current.expanded) guard(toggleDirectory(sessionId, current.path));
        else {
          for (let i = index - 1; i >= 0; i -= 1) {
            const candidate = entries[i];
            if (candidate && candidate.depth < current.depth) {
              select(candidate);
              break;
            }
          }
        }
        break;
      case "Enter":
        if (!current) break;
        e.preventDefault();
        activate(current, true);
        break;
      case " ":
        if (!current) break;
        e.preventDefault();
        activate(current, false);
        break;
      case "F2":
        if (current && !current.isRoot && !current.lossy && online) {
          e.preventDefault();
          startRename(sessionId, current.path);
        }
        break;
      case "Delete":
        if (current && !current.isRoot && !current.lossy && online) {
          e.preventDefault();
          requestDelete(sessionId, current.path, current.dirLike ? "dir" : "file");
        }
        break;
      case "ContextMenu":
        openMenuAtSelection(e.currentTarget, selection);
        e.preventDefault();
        break;
      case "F10":
        if (e.shiftKey) {
          openMenuAtSelection(e.currentTarget, selection);
          e.preventDefault();
        }
        break;
    }
  };

  return (
    <div className="file-tree-wrap" style={columns ? ({ "--file-tree-scrollbar": `${scrollbar}px` } as CSSProperties) : undefined}>
      {online ? null : <p className="file-tree__note file-tree__note--warn">{t("files.offline")}</p>}
      {columns && tree.root !== null ? <FileTreeHeader /> : null}
      <div
        ref={scrollRef}
        className={`file-tree${columns ? " file-tree--columns" : ""}${dropTarget === tree.root ? " file-tree--drop" : ""}`}
        role="tree"
        tabIndex={0}
        aria-label={t("files.tree")}
        aria-activedescendant={selectedIndex >= 0 && rows[selectedIndex] ? rowId(prefix, rows[selectedIndex]) : undefined}
        data-drop-zone=""
        data-tree-session={sessionId}
        data-tree-path={tree.root ?? undefined}
        data-remote-dir={tree.root ?? undefined}
        onKeyDown={onKeyDown}
        onFocus={(e) => {
          // Keyboard users arrive on the tree: give them a current row.
          if (e.target === e.currentTarget && selection === null && tree.root !== null) selectTreeEntry(sessionId, tree.root);
        }}
      >
        <div className="file-tree__canvas" style={{ height: virtualizer.getTotalSize() }}>
          {virtualizer.getVirtualItems().map((item) => {
            const row = rows[item.index];
            if (!row) return null;
            const style = { height: ROW_HEIGHT, transform: `translateY(${item.start}px)` };
            if (row.type === "note") {
              const indent = INDENT_BASE + INDENT_STEP * row.depth + CHEVRON_SLOT;
              return (
                <div key={row.key} id={rowId(prefix, row)} role="treeitem" aria-level={row.depth + 1} aria-disabled="true" className="file-row file-row--note" style={{ ...style, paddingLeft: indent }}>
                  {row.note === "loading" ? t("files.loading") : null}
                  {row.note === "empty" ? t("files.emptyFolder") : null}
                  {row.note === "truncated" ? t("files.truncatedCount", { count: row.count }) : null}
                </div>
              );
            }
            if (row.type === "new") {
              const indent = INDENT_BASE + INDENT_STEP * row.depth + CHEVRON_SLOT;
              return (
                <div key={row.key} id={rowId(prefix, row)} role="treeitem" aria-level={row.depth + 1} className="file-row file-row--edit" style={{ ...style, paddingLeft: indent }}>
                  <Icon name={row.entry === "dir" ? "folderPlus" : "filePlus"} size={15} strokeWidth={1.8} className="file-row__file" />
                  <TreeNameInput
                    initial=""
                    label={row.entry === "dir" ? t("files.newFolderName") : t("files.newFileName")}
                    onCommit={(name) =>
                      createEntry(sessionId, row.parent, row.entry, name).then(() => {
                        if (row.entry === "dir") scrollRef.current?.focus(); // a new file takes the focus to the editor
                      })
                    }
                    onCancel={() => cancelTreeEdit(sessionId)}
                    onEscape={() => {
                      cancelTreeEdit(sessionId);
                      scrollRef.current?.focus();
                    }}
                  />
                </div>
              );
            }
            const selected = selection === row.path;
            const indent = INDENT_BASE + INDENT_STEP * row.depth + (row.dirLike ? 0 : CHEVRON_SLOT);
            const symlink = row.kind === "symlink";
            // Permission hint: dim files that the mode bits say we cannot write (saving is still allowed) and
            // folders we cannot open. A hint only, nothing is blocked.
            const entryInfo = row.isRoot ? undefined : lookupEntry(tree, row.path);
            const noAccessHint = row.kind === "dir" && !row.isRoot && enterable(entryInfo, identity) === "no";
            const readOnlyHint = noAccessHint || (!row.dirLike && !row.isRoot && writability(entryInfo, identity) === "no");
            const hintText = row.lossy ? t("files.lossyName") : noAccessHint ? t("files.probablyNoAccess") : readOnlyHint ? t("files.probablyReadOnly") : null;
            const details = entryInfo && !row.lossy ? detailLines(entryInfo) : [];
            const described = [...(hintText === null ? [] : [hintText]), ...details];
            // With columns the name may be cut off: the full name leads the tooltip.
            const rowTitle = [...(columns && !row.renaming ? [displayName(row.name)] : []), ...described].join("\n") || undefined;
            const perms = showPermissions && entryInfo && !row.lossy ? permissionsSummary(entryInfo) : null;
            const cellInfo = columns && entryInfo && !row.lossy ? entryInfo : undefined;
            const sizeText = cellInfo?.kind === "file" && cellInfo.size !== null ? formatBytes(cellInfo.size, sizeUnits) : "";
            const dateText = cellInfo && cellInfo.mtime !== null ? formatFileDateShort(cellInfo.mtime, now, dateFormat) : "";
            const descriptionId = described.length > 0 ? `${rowId(prefix, row)}-details` : undefined;
            // One slot (`display: contents` without columns, a fixed-width box with them: files.css).
            const marks = (
              <span className="file-row__marks">
                {row.lossy ? <Icon name="alert" size={12} className="file-row__warn" /> : null}
                {readOnlyHint ? <Icon name="lock" size={11} className="file-row__lock" /> : null}
                {dirtyPaths.has(row.path) ? (
                  <span className="file-row__dirty" role="img" aria-label={t("editor.unsaved")} title={t("editor.unsaved")} />
                ) : null}
              </span>
            );
            return (
              <div
                key={row.path}
                id={rowId(prefix, row)}
                aria-describedby={descriptionId}
                role="treeitem"
                aria-level={row.depth + 1}
                aria-posinset={row.posInSet}
                aria-setsize={row.setSize}
                aria-expanded={row.dirLike ? row.expanded : undefined}
                aria-selected={selected}
                aria-disabled={row.lossy ? true : undefined}
                title={rowTitle}
                className={[
                  "file-row",
                  selected ? "file-row--selected" : "",
                  row.lossy ? "file-row--lossy" : "",
                  symlink ? "file-row--link" : "",
                  readOnlyHint ? "file-row--readonly" : "",
                  row.dirLike && dropTarget === row.path ? "file-row--drop" : "",
                ]
                  .filter(Boolean)
                  .join(" ")}
                style={{ ...style, paddingLeft: indent }}
                data-tree-path={row.path}
                data-tree-session={sessionId}
                data-remote-dir={row.dirLike ? row.path : dirname(row.path)}
                onClick={() => {
                  // `files.doubleClick = "download"`: a click on a file only selects it; opening is a menu / Enter action.
                  if (downloadOnDoubleClick && !row.dirLike && !row.lossy) selectTreeEntry(sessionId, row.path);
                  else activate(row, false);
                }}
                onDoubleClick={() => {
                  if (row.dirLike || row.lossy) return;
                  if (downloadOnDoubleClick) {
                    if (row.kind === "file" || row.kind === "symlink") guard(downloadFile(sessionId, row.path));
                  } else if (rowOpenable(row)) guard(openFile(sessionId, row.path, { focus: true }));
                }}
              >
                {row.dirLike ? <Icon name={row.expanded ? "chevronDown" : "chevronRight"} size={12} className="file-row__chevron" /> : null}
                <Icon name={row.dirLike ? "folder" : "file"} size={15} strokeWidth={1.8} className={row.dirLike ? "file-row__folder" : "file-row__file"} />
                {row.renaming ? (
                  <TreeNameInput
                    initial={row.name}
                    label={t("files.renameName")}
                    onCommit={(name) => renameEntry(sessionId, row.path, name).then(() => scrollRef.current?.focus())}
                    onCancel={() => cancelTreeEdit(sessionId)}
                    onEscape={() => {
                      cancelTreeEdit(sessionId);
                      scrollRef.current?.focus();
                    }}
                  />
                ) : (
                  <bdi className="file-row__name">{displayName(row.name)}</bdi>
                )}
                {perms !== null ? (
                  // Also in the description below: the row's accessible name stays the file name.
                  <span className="file-row__meta" aria-hidden="true">
                    <bdi>{perms}</bdi>
                  </span>
                ) : null}
                {/* The same order with and without columns (files.css relies on it); the cells come last, so they
                    keep one right edge under the header. */}
                {marks}
                {columns ? (
                  <>
                    <span className="file-row__col file-row__col--size" aria-hidden="true">
                      {sizeText}
                    </span>
                    <span className="file-row__col file-row__col--date" aria-hidden="true">
                      {dateText}
                    </span>
                  </>
                ) : null}
                {descriptionId !== undefined ? (
                  // Referenced by aria-describedby (which reads hidden text); hidden so it stays out of the row's name.
                  <span id={descriptionId} className="sr-only" aria-hidden="true">
                    {formatList(described, "unit")}
                  </span>
                ) : null}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

/** Shift+F10 / the menu key: asks for the context menu at the selected row (the app's own menu). */
function openMenuAtSelection(tree: HTMLElement, selection: string | null): void {
  const row = [...tree.querySelectorAll<HTMLElement>(SEL_TREE_ITEM)].find((el) => el.dataset["treePath"] === selection) ?? tree;
  const box = row.getBoundingClientRect();
  row.dispatchEvent(
    new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: box.left + 24, clientY: box.top + box.height / 2 }),
  );
}
