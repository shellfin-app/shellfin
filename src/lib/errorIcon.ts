import type { AppError } from "../ipc/types";
import type { IconName } from "./iconPaths";

/** Glyph of the failure: network trouble is the offline icon; credentials, trust and everything else are not. */
export function errorIcon(error: AppError | null): IconName {
  switch (error?.code) {
    case "dnsFailed":
    case "connectRefused":
    case "connectTimeout":
    case "networkUnreachable":
      return "wifiOff";
    case "authFailed":
    case "tooManyAuthFailures":
    case "passphraseIncorrect":
    case "keyLoadFailed":
    case "agentNoIdentities":
    case "agentUnavailable":
      return "lock";
    case "hostKeyChanged":
    case "hostKeyMismatchOnReconnect":
    case "hostKeyRejected":
      return "shield";
    default:
      return "alert";
  }
}
