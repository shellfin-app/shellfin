import { Command } from "cmdk";
import { useEffect, useMemo, useState, type KeyboardEvent } from "react";
import { useTranslation } from "react-i18next";
import { getBackend } from "../../ipc";
import { ACTION_IDS, shortcutLabel, type ActionId } from "../../lib/actionIds";
import { parsePaletteQuery } from "../../lib/paletteQuery";
import { guard } from "../../lib/reportError";
import { basename } from "../../lib/remotePath";
import { displayName, displayPath } from "../../lib/serverText";
import { bookmarkTarget, sshConfigTarget } from "../../lib/targets";
import { useStore } from "../../state/store";
import { actionEnabled, actionLabel, runAction } from "../actions/registry";
import { openFile } from "../editor/editorActions";
import { goToFolder } from "../files/fileActions";
import { requestOpenSession } from "../session/sessionActions";
import "./palette.css";

/** Actions the palette lists: all of them except the ones that act on whatever has the focus (Edit menu) and itself. */
const NOT_IN_PALETTE: readonly ActionId[] = ["undo", "redo", "cut", "copy", "paste", "find", "replace", "selectAll", "commandPalette"];
const PALETTE_ACTIONS = ACTION_IDS.filter((id) => !NOT_IN_PALETTE.includes(id));
const MAX_FILE_RESULTS = 50;
const PATH_ITEM = "palette:go-to-path";
/** Marks file items for the filter (their value is the full path, which ranks below a file name match). */
const FILE_KEYWORD = "palette:file";

/** Orders files that contain `needle`: name matches before path-only matches, open files before the others; capped. */
function rankFiles(files: readonly { path: string; open: boolean }[], needle: string): { path: string; open: boolean }[] {
  const hits = files
    .map((f) => ({ f, rank: (nameMatches(f.path, needle) ? 0 : 2) + (f.open ? 0 : 1) }));
  hits.sort((a, b) => a.rank - b.rank);
  return hits.slice(0, MAX_FILE_RESULTS).map((x) => x.f);
}

/** Does the file NAME (not the whole path) contain the search text? */
function nameMatches(path: string, needle: string): boolean {
  return basename(path).toLowerCase().includes(needle);
}

/** `/path`: go to that remote folder (the same action as Connection > Go to folder). */
function PathGroup({ sessionId, path, onDone }: { sessionId: string | null; path: string; onDone: () => void }) {
  const { t } = useTranslation();
  return (
    <Command.Group heading={t("palette.path")}>
      {sessionId ? (
        <Command.Item
          value={PATH_ITEM}
          onSelect={() => {
            onDone();
            guard(goToFolder(sessionId, path));
          }}
        >
          {t("palette.goToPath")}
          <span className="palette__hint mono">
            <bdi>{displayPath(path)}</bdi>
          </span>
        </Command.Item>
      ) : (
        <div className="palette__empty">{t("palette.pathNeedsSession")}</div>
      )}
    </Command.Group>
  );
}

/**
 * Command palette (§8.7 Mod+K): hosts, files and commands with their shortcuts. "files" mode lists open buffers and
 * already-loaded tree entries; in "all" mode the files appear once something is typed. `>` limits the list to
 * commands, `@` to hosts, `/` goes to a remote path.
 */
export function CommandPalette({ mode }: { mode: "all" | "files" }) {
  const { t } = useTranslation();
  const close = useStore((s) => s.closePalette);
  const sshConfig = useStore((s) => s.sshConfig);
  const bookmarks = useStore((s) => s.bookmarks);
  const activeId = useStore((s) => s.activeSessionId);
  const workspace = useStore((s) => (s.activeSessionId ? s.workspaces[s.activeSessionId] : undefined));
  const buffers = useStore((s) => s.buffers);
  const platform = useStore((s) => s.platform);

  // Closing the palette returns focus to where it was (a focused terminal keeps receiving keys), unless the
  // chosen action already moved focus somewhere useful (a new dialog, a new terminal).
  // Captured during the first render: child effects (the input's autofocus) run before this component's effects.
  const [previous] = useState(() => (document.activeElement instanceof HTMLElement ? document.activeElement : null));
  useEffect(() => {
    return () => {
      setTimeout(() => {
        const active = document.activeElement;
        if (previous?.isConnected && (active === null || active === document.body)) previous.focus();
      }, 0);
    };
  }, [previous]);

  const run = (fn: () => void) => () => {
    close();
    fn();
  };

  const fileItems = useMemo(() => {
    if (!activeId || !workspace) return [];
    const seen = new Set<string>();
    const out: { path: string; open: boolean }[] = [];
    for (const id of workspace.editorTabs) {
      const b = buffers[id];
      if (b && !seen.has(b.path)) {
        seen.add(b.path);
        out.push({ path: b.path, open: true });
      }
    }
    for (const cached of Object.values(workspace.tree.listings)) {
      for (const e of cached.listing.entries) {
        if (e.kind !== "dir" && !seen.has(e.path)) {
          seen.add(e.path);
          out.push({ path: e.path, open: false });
        }
      }
    }
    return out;
  }, [activeId, workspace, buffers]);

  const [query, setQuery] = useState("");
  // `>` = commands only, `@` = hosts only. The prefix is not part of the search text.
  const parsed = parsePaletteQuery(mode === "all" ? query : "");
  const prefix = parsed.prefix;
  const showHosts = prefix === "" || prefix === "hosts";
  const showCommands = prefix === "" || prefix === "commands";
  const showFiles = mode === "all" && prefix === "" && parsed.text !== "";
  const showPath = mode === "all" && prefix === "path";
  const canGoToPath = useStore((s) => s.activeSessionId !== null && s.sessionsById[s.activeSessionId]?.state.phase === "connected" && s.workspaces[s.activeSessionId]?.tree.root != null);
  const needle = parsed.text;

  // Files only appear once something is typed (a loaded tree can be large): open files first, capped.
  const visibleFiles = useMemo(() => {
    if (!showFiles) return [];
    // File name matches first (a path like /home/tester/... must not outrank test.txt for "test"), then open files.
    return rankFiles(fileItems.filter((f) => f.path.toLowerCase().includes(needle)), needle);
  }, [fileItems, needle, showFiles]);

  const filter = (value: string, search: string, keywords?: string[]) => {
    if (mode === "all" && parsePaletteQuery(search).prefix === "path") return value === PATH_ITEM ? 1 : 0;
    const text = mode === "all" ? parsePaletteQuery(search).text : search.trim().toLowerCase();
    if (text === "") return 1;
    if (!`${value} ${(keywords ?? []).join(" ")}`.toLowerCase().includes(text)) return 0;
    // Files (their value is the full path) rank by file name first.
    return keywords?.includes(FILE_KEYWORD) === true && !nameMatches(value, text) ? 0.5 : 1;
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    }
  };

  const fileList = (
    <Command.Group heading={t("palette.files")}>
      {mode === "files" && activeId === null ? <div className="palette__empty">{t("files.empty")}</div> : null}
      {(mode === "files" ? fileItems : visibleFiles).map((f) => (
        <Command.Item key={f.path} value={f.path} keywords={[FILE_KEYWORD]} onSelect={run(() => activeId && guard(openFile(activeId, f.path)))}>
          <bdi>{displayName(basename(f.path))}</bdi>
          <span className="palette__hint mono">
            <bdi>{displayPath(f.path)}</bdi>
          </span>
        </Command.Item>
      ))}
    </Command.Group>
  );

  return (
    <div className="dialog-backdrop palette-backdrop" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <Command
        className="palette"
        label={t("palette.title")}
        onKeyDown={onKeyDown}
        loop
        role="dialog"
        aria-modal="true"
        aria-label={t("palette.title")}
        filter={filter}
      >
        <Command.Input
          className="palette__input"
          autoFocus
          value={query}
          onValueChange={setQuery}
          placeholder={mode === "files" ? t("palette.filesPlaceholder") : t("palette.placeholder")}
          aria-label={t("shell.search.label")}
        />
        <Command.List className="palette__list">
          <Command.Empty className="palette__empty">{t("palette.empty")}</Command.Empty>

          {showPath ? (
            <PathGroup sessionId={canGoToPath ? activeId : null} path={parsed.text} onDone={close} />
          ) : mode === "files" ? (
            fileList
          ) : (
            <>
              {showHosts ? (
                <Command.Group heading={t("palette.hosts")}>
                  {(sshConfig?.hosts ?? []).map((h) => (
                    <Command.Item
                      key={`ssh:${h.alias}`}
                      value={`${h.alias} ${h.hostname}`}
                      onSelect={run(() => guard(requestOpenSession(sshConfigTarget(h.alias), h.alias)))}
                    >
                      <bdi>{displayName(h.alias)}</bdi>
                      <span className="palette__hint mono">
                        <bdi>{displayName(h.hostname)}</bdi>
                      </span>
                    </Command.Item>
                  ))}
                  {(bookmarks?.bookmarks ?? []).map((b) => (
                    <Command.Item
                      key={`bm:${b.id}`}
                      value={`${b.name} ${b.host}`}
                      onSelect={run(() => guard(requestOpenSession(bookmarkTarget(b.id), b.name)))}
                    >
                      <bdi>{displayName(b.name)}</bdi>
                      <span className="palette__hint mono">
                        <bdi>{displayName(b.host)}</bdi>
                      </span>
                    </Command.Item>
                  ))}
                </Command.Group>
              ) : null}
              {showFiles ? fileList : null}
              {showCommands ? (
                <Command.Group heading={t("palette.commands")}>
                  {PALETTE_ACTIONS.map((id) => {
                    const label = actionLabel(id);
                    const shortcut = shortcutLabel(id, platform);
                    return (
                      <Command.Item key={id} value={label} disabled={!actionEnabled(id)} onSelect={run(() => runAction(id))}>
                        {label}
                        {shortcut ? <kbd className="palette__kbd">{shortcut}</kbd> : null}
                      </Command.Item>
                    );
                  })}
                  <Command.Item
                    value={t("palette.reloadConfig")}
                    onSelect={run(() =>
                      guard(
                        getBackend()
                          .sshConfigReload()
                          .then((snap) => useStore.getState().setSshConfig(snap)),
                      ),
                    )}
                  >
                    {t("palette.reloadConfig")}
                  </Command.Item>
                  <Command.Item value={t("welcome.manageKeys")} onSelect={run(() => useStore.getState().openDialog({ kind: "keys" }))}>
                    {t("welcome.manageKeys")}
                  </Command.Item>
                </Command.Group>
              ) : null}
            </>
          )}
        </Command.List>
      </Command>
    </div>
  );
}
