import type { TransferDecision } from "../ipc/types";

/** `transfer_decide` values (identifiers, not UI copy). */
export const DECISION = {
  merge: "merge",
  keepBoth: "keepBoth",
  proceed: "proceed",
  cancel: "cancel",
  replaceAll: "replaceAll",
  skipAll: "skipAll",
  replaceOlder: "replaceOlder",
} as const satisfies Record<TransferDecision, TransferDecision>;

/** The answers to deferred conflicts (they apply to every conflicting file of the job, also later ones). */
export const CONFLICT_DECISIONS: readonly TransferDecision[] = [DECISION.replaceAll, DECISION.skipAll, DECISION.replaceOlder];
