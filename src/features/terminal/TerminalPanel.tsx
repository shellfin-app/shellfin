import { type KeyboardEvent } from "react";
import { useTranslation } from "react-i18next";
import { useShallow } from "zustand/react/shallow";
import { Button } from "../../components/Button";
import { ErrorBoundary } from "../../components/ErrorBoundary";
import { Icon } from "../../components/Icon";
import { IconButton } from "../../components/IconButton";
import { InlineRename } from "../../components/InlineRename";
import { A } from "../../lib/actionIds";
import { isMac } from "../../lib/keybindings";
import { basename } from "../../lib/remotePath";
import { guard } from "../../lib/reportError";
import { displayServerText } from "../../lib/serverText";
import { useStore } from "../../state/store";
import { runAction } from "../actions/registry";
import { ReconnectOverlay } from "../session/ReconnectOverlay";
import { TerminalFindBar } from "./TerminalFindBar";
import { XtermView } from "./XtermView";
import { openTerminalTab, requestCloseTerminalTabs } from "./terminalActions";
import "./terminal.css";

const MAX_NAME_LENGTH = 64;

/** Bottom panel: terminal tabs + the active terminal (Main.dc "terminal panel"). */
export function TerminalPanel({ sessionId }: { sessionId: string }) {
  const { t } = useTranslation();
  const tabs = useStore((s) => s.workspaces[sessionId]?.terminalTabs);
  const active = useStore((s) => s.workspaces[sessionId]?.activeTerminal ?? null);
  // Only this session's tab metas (stable object identities), so another terminal's title change does not re-render.
  const metas = useStore(useShallow((s) => (s.workspaces[sessionId]?.terminalTabs ?? []).map((id) => s.terminals[id])));
  const searchOpen = useStore((s) => s.terminalSearch);
  const setActiveTerminal = useStore((s) => s.setActiveTerminal);
  const patchTerminal = useStore((s) => s.patchTerminal);
  const setTerminalSearch = useStore((s) => s.setTerminalSearch);
  const renaming = useStore((s) => s.renamingTerminal);
  const setRenaming = useStore((s) => s.setRenamingTerminal);
  const maximized = useStore((s) => s.workspaces[sessionId]?.panelMaximized === true);
  const platform = useStore((s) => s.platform);
  const findShortcut = t(isMac(platform) ? "shortcut.terminalFindMac" : "shortcut.terminalFind");

  const ids = tabs ?? [];
  const onTabKeyDown = (e: KeyboardEvent<HTMLElement>, termId: string, index: number) => {
    if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
      const next = ids[(index + (e.key === "ArrowRight" ? 1 : ids.length - 1)) % ids.length];
      if (next) {
        e.preventDefault();
        setActiveTerminal(sessionId, next);
      }
    } else if (e.key === "F2") {
      e.preventDefault();
      setRenaming(termId);
    }
  };

  return (
    <section className="terminal-panel" aria-label={t("terminal.panel")}>
      <div className="terminal-panel__bar">
        <span className="sec terminal-panel__title">{t("terminal.title")}</span>
        <div className="terminal-panel__tabs" role="tablist" aria-label={t("terminal.tabs")}>
          {ids.map((termId, i) => {
            const meta = metas[i];
            if (!meta) return null;
            // `meta.title` is server-controlled (OSC title): sanitised and bounded before display (§9.5).
            const title =
              meta.name !== null
                ? displayServerText(meta.name, MAX_NAME_LENGTH)
                : meta.title !== null
                  ? displayServerText(meta.title, 256)
                  : // No title from the shell (OSC 0/2): the folder it was opened in, else the generic name.
                    meta.cwd && basename(meta.cwd) !== ""
                    ? displayServerText(basename(meta.cwd), MAX_NAME_LENGTH)
                    : t("terminal.defaultTitle");
            // The number only helps tell tabs apart: a lone terminal is just its title.
            const label = ids.length >= 2 ? t("terminal.tabTitle", { index: i + 1, title }) : title;
            const selected = termId === active;
            const ended = meta.status !== "running";
            return (
              <span
                key={termId}
                data-terminal-tab={termId}
                data-session-id={sessionId}
                className={`terminal-tab${selected ? " terminal-tab--active" : ""}${ended ? " terminal-tab--ended" : ""}`}
                onAuxClick={(e) => {
                  if (e.button === 1) {
                    e.preventDefault();
                    requestCloseTerminalTabs(sessionId, [termId]);
                  }
                }}
              >
                {renaming === termId ? (
                  <InlineRename
                    className="terminal-tab__rename"
                    label={t("terminal.renameLabel")}
                    initial={meta.name ?? ""}
                    onDone={(name) => {
                      setRenaming(null);
                      if (name !== undefined) patchTerminal(termId, { name });
                    }}
                  />
                ) : (
                  <button
                    type="button"
                    role="tab"
                    aria-selected={selected}
                    tabIndex={selected ? 0 : -1}
                    className="terminal-tab__main"
                    title={ended ? t("terminal.endedHint") : t("terminal.renameHint")}
                    onClick={() => setActiveTerminal(sessionId, termId)}
                    onDoubleClick={() => setRenaming(termId)}
                    onKeyDown={(e) => onTabKeyDown(e, termId, i)}
                  >
                    <Icon name="terminal" size={13} />
                    {label}
                  </button>
                )}
                <IconButton
                  icon="close"
                  iconSize={11}
                  size={20}
                  label={t("terminal.closeTab", { name: label })}
                  className="terminal-tab__close"
                  onClick={() => requestCloseTerminalTabs(sessionId, [termId])}
                />
              </span>
            );
          })}
        </div>
        <IconButton icon="plus" label={t("terminal.new")} onClick={() => guard(openTerminalTab(sessionId))} />
        <span className="terminal-panel__spacer" />
        <IconButton
          icon="search"
          iconSize={14}
          label={t("terminal.findWithKey", { shortcut: findShortcut })}
          disabled={active === null}
          active={searchOpen !== null && searchOpen === active}
          onClick={() => setTerminalSearch(searchOpen === active ? null : active)}
        />
        {/* Split panes arrive in v1.1 (§1.1). */}
        <IconButton icon="split" iconSize={15} label={t("terminal.split")} disabled />
        <IconButton
          icon={maximized ? "chevronDown" : "chevronUp"}
          iconSize={14}
          label={maximized ? t("terminal.restorePanel") : t("terminal.maximizePanel")}
          aria-pressed={maximized}
          onClick={() => runAction(A.toggleMaximizePanel)}
        />
      </div>
      <div className="terminal-panel__body">
        {active && searchOpen === active ? <TerminalFindBar key={active} termId={active} /> : null}
        <ErrorBoundary>
          {active ? (
            <XtermView key={active} termId={active} />
          ) : (
            <div className="terminal-panel__empty">
              <p>{t("terminal.empty")}</p>
              <Button icon="plus" onClick={() => guard(openTerminalTab(sessionId))}>
                {t("terminal.new")}
              </Button>
            </div>
          )}
        </ErrorBoundary>
      </div>
      <ReconnectOverlay sessionId={sessionId} />
    </section>
  );
}
