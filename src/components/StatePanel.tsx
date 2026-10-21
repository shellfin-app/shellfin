import type { ReactNode } from "react";
import type { IconName } from "../lib/iconPaths";
import { Icon } from "./Icon";
import "./statePanel.css";

export interface StatePanelProps {
  icon: IconName;
  /** Visual tone of the icon chip. */
  tone?: "neutral" | "danger" | "warn" | "busy";
  title: ReactNode;
  /** Mono line under the title (target, endpoint). */
  subtitle?: ReactNode;
  /** `alert` for failures, `status` for progress. */
  role?: "alert" | "status";
  children?: ReactNode;
  actions?: ReactNode;
  className?: string;
}

/**
 * One panel for every "this area is in a state" screen (connecting, connection failed, binary file ...):
 * a centred card in the upper third, same placement and wording everywhere.
 */
export function StatePanel({ icon, tone = "neutral", title, subtitle, role = "status", children, actions, className }: StatePanelProps) {
  return (
    <div className="state-panel">
      <div className={["state-panel__card", className ?? ""].filter(Boolean).join(" ")} role={role}>
        <div className="state-panel__head">
          <span className={`state-panel__icon state-panel__icon--${tone}`}>
            <Icon name={icon} size={18} className={tone === "busy" ? "spin" : undefined} />
          </span>
          <div className="state-panel__titles">
            <h2 className="state-panel__title">{title}</h2>
            {subtitle ? <span className="state-panel__sub mono">{subtitle}</span> : null}
          </div>
        </div>
        {children}
        {actions ? <div className="state-panel__actions">{actions}</div> : null}
      </div>
    </div>
  );
}
