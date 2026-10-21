import { useTranslation } from "react-i18next";
import { Icon } from "../../components/Icon";
import { A, shortcutLabel } from "../../lib/actionIds";
import { useStore } from "../../state/store";
import { runAction } from "../actions/registry";

/**
 * Sidebar and terminal-panel toggles in the status bar: a hidden panel can always be brought back with the
 * mouse, and the tooltips teach the shortcuts.
 */
export function PanelToggles({ withTerminal }: { withTerminal: boolean }) {
  const { t } = useTranslation();
  const platform = useStore((s) => s.platform);
  const sidebarOpen = useStore((s) => s.sidebarOpen);
  const panelOpen = useStore((s) => {
    const id = s.activeSessionId;
    return id ? (s.workspaces[id]?.panelOpen ?? false) : false;
  });
  const sidebarHint = shortcutLabel(A.toggleSidebar, platform);
  const terminalHint = shortcutLabel(A.toggleTerminalPanel, platform);
  const sidebarLabel = t("status.toggleSidebar", { shortcut: sidebarHint });
  const terminalLabel = t("status.toggleTerminal", { shortcut: terminalHint });

  return (
    <span className="status-bar__toggles">
      <button
        type="button"
        className={`status-bar__button status-bar__toggle${sidebarOpen ? " status-bar__toggle--on" : ""}`}
        aria-pressed={sidebarOpen}
        aria-label={sidebarLabel}
        title={sidebarLabel}
        onClick={() => runAction(A.toggleSidebar)}
      >
        <Icon name="panelLeft" size={14} />
      </button>
      {withTerminal ? (
        <button
          type="button"
          className={`status-bar__button status-bar__toggle${panelOpen ? " status-bar__toggle--on" : ""}`}
          aria-pressed={panelOpen}
          aria-label={terminalLabel}
          title={terminalLabel}
          onClick={() => runAction(A.toggleTerminalPanel)}
        >
          <Icon name="panelBottom" size={14} />
        </button>
      ) : null}
    </span>
  );
}
