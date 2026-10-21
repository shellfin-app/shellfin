import type { SliceCreator } from "./types";

export type TerminalStatus = "running" | "exited" | "closed";

export interface TerminalMeta {
  termId: string;
  sessionId: string;
  /** Terminal-reported title (server text, shown through displayServerText); `null` = default title. */
  title: string | null;
  /** User-chosen tab name (local text); wins over `title`. */
  name: string | null;
  cwd: string | null;
  status: TerminalStatus;
  exitCode: number | null;
}

export interface TerminalsSlice {
  terminals: Record<string, TerminalMeta>;
  /** The terminal whose find bar is open (`null` = none). */
  terminalSearch: string | null;
  /** The terminal whose tab name is being edited (context menu / F2 / double-click). */
  renamingTerminal: string | null;
  addTerminal(meta: TerminalMeta): void;
  patchTerminal(termId: string, patch: Partial<Omit<TerminalMeta, "termId" | "sessionId">>): void;
  setTerminalSearch(termId: string | null): void;
  setRenamingTerminal(termId: string | null): void;
  /** Drops the meta of a terminal that never got a tab. */
  forgetTerminal(termId: string): void;
}

export const createTerminalsSlice: SliceCreator<TerminalsSlice> = (set) => ({
  terminals: {},
  terminalSearch: null,
  renamingTerminal: null,
  addTerminal: (meta) =>
    set((s) => {
      s.terminals[meta.termId] = meta;
    }),
  patchTerminal: (termId, patch) =>
    set((s) => {
      const t = s.terminals[termId];
      if (t) Object.assign(t, patch);
    }),
  setTerminalSearch: (termId) =>
    set((s) => {
      s.terminalSearch = termId;
    }),
  setRenamingTerminal: (termId) =>
    set((s) => {
      s.renamingTerminal = termId;
    }),
  forgetTerminal: (termId) =>
    set((s) => {
      delete s.terminals[termId];
      if (s.terminalSearch === termId) s.terminalSearch = null;
      if (s.renamingTerminal === termId) s.renamingTerminal = null;
    }),
});
