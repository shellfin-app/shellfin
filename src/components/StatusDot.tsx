import type { SessionState } from "../ipc/types";
import "./components.css";

export type DotState = "connected" | "connecting" | "error" | "idle";

/** The one mapping from a session phase to its dot (undefined = no session). */
export function dotForPhase(phase: SessionState["phase"] | undefined): DotState {
  switch (phase) {
    case undefined:
    case "closed":
      return "idle";
    case "connected":
      return "connected";
    case "disconnected":
      return "error";
    default:
      return "connecting";
  }
}

export interface StatusDotProps {
  state: DotState;
  /** Accessible text (already translated). */
  label: string;
}

/** 7px state dot: filled green = connected, hollow = not connected, amber = in progress, red = error. */
export function StatusDot({ state, label }: StatusDotProps) {
  return <span className={`status-dot status-dot--${state}`} role="img" aria-label={label} title={label} />;
}
