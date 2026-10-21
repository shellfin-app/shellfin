import type { StateCreator } from "zustand";
import type { AppSlice } from "./app";
import type { BuffersSlice } from "./buffers";
import type { HostsSlice } from "./hosts";
import type { PromptsSlice } from "./prompts";
import type { SessionsSlice } from "./sessions";
import type { SettingsSlice } from "./settings";
import type { TerminalsSlice } from "./terminals";
import type { TransfersSlice } from "./transfers";
import type { UiSlice } from "./ui";
import type { UpdatesSlice } from "./updates";
import type { WorkspacesSlice } from "./workspaces";

/** The single root store (docs/ARCHITECTURE.md §8.3): one store of slices with immer. Plain objects/arrays only. */
export type RootState = AppSlice &
  SettingsSlice &
  HostsSlice &
  SessionsSlice &
  WorkspacesSlice &
  BuffersSlice &
  TerminalsSlice &
  PromptsSlice &
  TransfersSlice &
  UiSlice &
  UpdatesSlice;

export type SliceCreator<T> = StateCreator<RootState, [["zustand/immer", never]], [], T>;
