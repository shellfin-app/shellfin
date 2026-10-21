import { useTranslation } from "react-i18next";
import { Icon } from "../../components/Icon";
import type { IconName } from "../../lib/iconPaths";
import type { ActivityView } from "../../state/slices/ui";
import { useStore } from "../../state/store";

interface ActivityButtonProps {
  icon: IconName;
  label: string;
  selected?: boolean;
  disabled?: boolean;
  onClick: () => void;
}

function ActivityButton({ icon, label, selected = false, disabled = false, onClick }: ActivityButtonProps) {
  return (
    <button
      type="button"
      className={`activity-button${selected ? " activity-button--active" : ""}`}
      aria-label={label}
      title={label}
      aria-pressed={selected}
      disabled={disabled}
      onClick={onClick}
    >
      <Icon name={icon} size={20} strokeWidth={1.8} />
    </button>
  );
}

/**
 * Left activity bar (Main.dc / Welcome.dc). Bookmarks / Files / Both switch the sidebar view (and open the sidebar when
 * it is hidden); Search opens the palette in files mode; Keys and Settings open dialogs. Port forwarding is out of
 * scope for v1 (hidden by feature flag, §1.1).
 */
export function ActivityBar() {
  const { t } = useTranslation();
  const activity = useStore((s) => s.activity);
  const setActivity = useStore((s) => s.setActivity);
  const openPalette = useStore((s) => s.openPalette);
  const openDialog = useStore((s) => s.openDialog);
  const setSidebarOpen = useStore((s) => s.setSidebarOpen);
  const sidebarOpen = useStore((s) => s.sidebarOpen);
  const hasSession = useStore((s) => s.activeSessionId !== null);
  // Without a session the Bookmarks view is the effective one (Welcome.dc).
  const effective = hasSession ? activity : "bookmarks";
  const show = (view: ActivityView) => {
    setActivity(view);
    if (!sidebarOpen) setSidebarOpen(true);
  };

  return (
    <nav className="activity-bar" aria-label={t("activity.bar")}>
      <ActivityButton
        icon="servers"
        label={t("activity.bookmarks")}
        selected={sidebarOpen && effective === "bookmarks"}
        onClick={() => show("bookmarks")}
      />
      <ActivityButton
        icon="folder"
        label={t("activity.files")}
        selected={sidebarOpen && effective === "files"}
        disabled={!hasSession}
        onClick={() => show("files")}
      />
      <ActivityButton
        icon="panes"
        label={t("activity.both")}
        selected={sidebarOpen && effective === "both"}
        disabled={!hasSession}
        onClick={() => show("both")}
      />
      <ActivityButton icon="search" label={t("activity.search")} onClick={() => openPalette("files")} />
      <ActivityButton icon="key" label={t("activity.keys")} onClick={() => openDialog({ kind: "keys" })} />
      <div className="activity-bar__spacer" />
      <ActivityButton icon="settings" label={t("activity.settings")} onClick={() => openDialog({ kind: "settings" })} />
    </nav>
  );
}
