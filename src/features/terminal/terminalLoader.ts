// xterm.js and its addons are loaded lazily (docs/ARCHITECTURE.md §8.6): a failure of the terminal library can
// then never take down the shell, and the entry chunk stays small. Everything outside this folder reaches the
// registry only through this facade.
import type * as RegistryModule from "./terminalRegistry";

type Registry = typeof RegistryModule;

let loaded: Registry | null = null;

/** Loads (once) the terminal registry chunk. A failed load is not cached, so the next call retries. */
export async function loadTerminalRegistry(): Promise<Registry> {
  loaded ??= await import("./terminalRegistry");
  return loaded;
}

/** The registry if its chunk was already loaded (find bar, shortcuts: a visible terminal implies it is). */
export function loadedTerminalRegistry(): Registry | null {
  return loaded;
}

/** Disposes a terminal if the registry was ever loaded (otherwise there is nothing to dispose). */
export function disposeTerminalIfLoaded(termId: string, closeOnBackend = true): void {
  loaded?.disposeTerminal(termId, closeOnBackend);
}
