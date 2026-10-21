import type { ButtonHTMLAttributes, ReactNode } from "react";
import type { IconName } from "../lib/iconPaths";
import { Icon } from "./Icon";
import "./components.css";

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: "primary" | "outline" | "ghost";
  icon?: IconName;
  children: ReactNode;
}

export function Button({ variant = "outline", icon, children, className, type = "button", ...rest }: ButtonProps) {
  const cls = ["button", `button--${variant}`, className ?? ""].filter(Boolean).join(" ");
  return (
    <button type={type} className={cls} {...rest}>
      {icon ? <Icon name={icon} size={14} strokeWidth={variant === "primary" ? 2.2 : 1.8} /> : null}
      {children}
    </button>
  );
}
