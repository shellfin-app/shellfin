import type { AgentStatus, BookmarksDto, EnvInfo, RecentEntry, SshConfigSnapshot } from "../../ipc/types";
import type { SliceCreator } from "./types";

export interface HostsSlice {
  sshConfig: SshConfigSnapshot | null;
  bookmarks: BookmarksDto | null;
  /** Updated by the `recents:changed` event. */
  recents: RecentEntry[];
  agent: AgentStatus | null;
  env: EnvInfo | null;
  setSshConfig(snapshot: SshConfigSnapshot): void;
  setBookmarks(dto: BookmarksDto): void;
  setRecents(recents: RecentEntry[]): void;
  setAgent(agent: AgentStatus): void;
  setEnv(env: EnvInfo): void;
}

export const createHostsSlice: SliceCreator<HostsSlice> = (set) => ({
  sshConfig: null,
  bookmarks: null,
  recents: [],
  agent: null,
  env: null,
  setSshConfig: (snapshot) =>
    set((s) => {
      s.sshConfig = snapshot;
    }),
  setBookmarks: (dto) =>
    set((s) => {
      s.bookmarks = dto;
    }),
  setRecents: (recents) =>
    set((s) => {
      s.recents = recents;
    }),
  setAgent: (agent) =>
    set((s) => {
      s.agent = agent;
    }),
  setEnv: (env) =>
    set((s) => {
      s.env = env;
    }),
});
