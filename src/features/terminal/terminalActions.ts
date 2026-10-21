import { confirmCloseTerminalsDialog } from "../../lib/dialogs";
import { SEL_TERMINAL_HOST } from "../../lib/domIds";
import { useStore } from "../../state/store";
import { sendStartupCommand } from "../session/startupCommand";
import { disposeTerminalIfLoaded, loadTerminalRegistry } from "./terminalLoader";

/**
 * Where a new terminal starts (`terminal.newTerminalCwd`). "home" sends no folder. "currentFolder" sends the root
 * folder of the connection's file tree with `cwdFallback`, so the backend lands in the home folder whenever that
 * folder cannot be used (not POSIX, gone, no access). Without a tree root there is nothing to send either.
 */
export function newTerminalStart(sessionId: string): { cwd: string | null; fallback: boolean } {
  const s = useStore.getState();
  if (s.settings?.terminal.newTerminalCwd !== "currentFolder") return { cwd: null, fallback: false };
  const root = s.workspaces[sessionId]?.tree.root ?? null;
  return root === null ? { cwd: null, fallback: false } : { cwd: root, fallback: true };
}

/**
 * Opens a new terminal tab in a session's panel. Resolves to the termId, or `null` if the session was closed
 * while the terminal was opening. An explicit `cwd` ("Open terminal here") is strict; without one the setting
 * `terminal.newTerminalCwd` decides.
 */
export async function openTerminalTab(sessionId: string, cwd: string | null = null): Promise<string | null> {
  const registry = await loadTerminalRegistry();
  const start = cwd === null ? newTerminalStart(sessionId) : { cwd, fallback: false };
  const termId = await registry.createTerminal(sessionId, start.cwd, start.fallback);
  if (termId === null) return null;
  const s = useStore.getState();
  // The shell may already have ended and `terminal.onExit` closed it while its first output was replayed.
  if (!s.sessionsById[sessionId] || !registry.getTerminal(termId)) {
    registry.disposeTerminal(termId);
    s.forgetTerminal(termId);
    return s.sessionsById[sessionId] ? termId : null;
  }
  s.addTerminalTab(sessionId, termId);
  s.setPanelOpen(sessionId, true);
  sendStartupCommand(sessionId, termId);
  return termId;
}

/**
 * Right-click inside a terminal with `terminal.rightClick = "paste"`: pastes the clipboard (through the same
 * sanitiser and multi-line confirmation as every paste) and returns true, so the caller shows no menu.
 */
export function rightClickPastes(target: Element): boolean {
  if (useStore.getState().settings?.terminal.rightClick !== "paste") return false;
  const termId = target.closest<HTMLElement>(SEL_TERMINAL_HOST)?.dataset["termId"];
  if (!termId) return false;
  void loadTerminalRegistry().then((registry) => registry.pasteClipboardInto(termId)).catch(() => undefined);
  return true;
}

/** Terminals with a live shell, optionally only those of one connection or of the given ids. */
export function runningTerminals(sessionId?: string, termIds?: readonly string[]): string[] {
  const { terminals } = useStore.getState();
  return Object.values(terminals)
    .filter((m) => m.status === "running" && (sessionId === undefined || m.sessionId === sessionId))
    .filter((m) => termIds === undefined || termIds.includes(m.termId))
    .map((m) => m.termId);
}

/** `general.confirmCloseWithTerminals` (on while the settings are unknown). */
export function confirmCloseWithTerminals(): boolean {
  return useStore.getState().settings?.general.confirmCloseWithTerminals ?? true;
}

/**
 * User asked to close terminal tabs: tabs with a running shell are confirmed first (setting
 * `general.confirmCloseWithTerminals`), otherwise they close at once. Plain `closeTerminalTab` is for flows that decided.
 */
export function requestCloseTerminalTabs(sessionId: string, termIds: readonly string[]): void {
  if (confirmCloseWithTerminals() && runningTerminals(sessionId, termIds).length > 0) {
    useStore.getState().openDialog(confirmCloseTerminalsDialog(sessionId, [...termIds]));
    return;
  }
  for (const id of termIds) closeTerminalTab(sessionId, id);
}

/** Closes a terminal tab: backend terminal, xterm instance and store entries. */
export function closeTerminalTab(sessionId: string, termId: string): void {
  disposeTerminalIfLoaded(termId);
  useStore.getState().removeTerminalTab(sessionId, termId);
}
