import type { RecentTarget } from "../ipc/types";

// Constructors for session targets (the discriminant values are protocol data, not UI copy).
export const sshConfigTarget = (alias: string): RecentTarget => ({ kind: "sshConfig", alias });
export const bookmarkTarget = (id: string): RecentTarget => ({ kind: "bookmark", id });
export const quickTarget = (spec: string): RecentTarget => ({ kind: "quick", spec });
