// Steps shown on the connecting card. Identifiers only; the texts are `connecting.step.<id>`.

export const CONNECT_STEPS = ["resolve", "connect", "hostKey", "auth", "open"] as const;
export type ConnectStep = (typeof CONNECT_STEPS)[number];

/** Which step is running, from the session phase and the pending prompt (a host-key prompt sits between connect and auth). */
export function currentStep(phase: string, promptKind: string | undefined): ConnectStep {
  if (promptKind === "hostKeyUnknown") return "hostKey";
  switch (phase) {
    case "resolving":
      return "resolve";
    case "connecting":
      return "connect";
    case "authenticating":
      return "auth";
    default:
      return "open";
  }
}
