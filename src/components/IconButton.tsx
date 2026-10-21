import type { ButtonHTMLAttributes } from "react";
import type { IconName } from "../lib/iconPaths";
import { Icon } from "./Icon";
import "./components.css";

export interface IconButtonProps extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, "children" | "aria-label"> {
  icon: IconName;
  /** Required accessible name (already translated). Also used as the tooltip. */
  label: string;
  size?: number;
  iconSize?: number;
  active?: boolean;
}

/** Icon-only button. `label` becomes both `aria-label` and `title`. */
export function IconButton({ icon, label, size = 28, iconSize = 14, active = false, className, ...rest }: IconButtonProps) {
  const cls = ["icon-button", active ? "icon-button--active" : "", className ?? ""].filter(Boolean).join(" ");
  return (
    <button
      type="button"
      className={cls}
      aria-label={label}
      title={label}
      style={{ width: size, height: size }}
      {...rest}
    >
      <Icon name={icon} size={iconSize} />
    </button>
  );
}
