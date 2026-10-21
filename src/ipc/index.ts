// Backend selection (docs/ARCHITECTURE.md §8.8).
//  - Inside Tauri: TauriBackend.
//  - Dev server in a plain browser: MockBackend (`VITE_BACKEND=tauri` forces Tauri).
//  - Smoke builds (`vite build --mode smoke`, VITE_BACKEND=mock): MockBackend even inside Tauri.
//  - Release builds: TauriBackend only. `MOCK_ALLOWED` is a build-time constant, so the dynamic import below
//    is dead code and the mock chunk is not emitted; `npm run build` asserts this (scripts/assert-no-mock.mjs).
import type { Backend } from "./backend";
import { TauriBackend } from "./tauriBackend";

export type { Backend, BackendEventName, BackendEvents, TermMessage, Unlisten } from "./backend";
export { isRawBytes } from "./backend";
export { IpcError } from "./call";

const MOCK_ALLOWED = import.meta.env.DEV || import.meta.env.VITE_BACKEND === "mock";

let current: Backend | null = null;

function isTauri(): boolean {
  return "__TAURI_INTERNALS__" in globalThis;
}

/** Selects the backend. Must be awaited once before the UI renders (main.tsx). */
export async function initBackend(): Promise<Backend> {
  if (current) return current;
  const forced = import.meta.env.VITE_BACKEND;
  if (MOCK_ALLOWED && (forced === "mock" || (forced !== "tauri" && !isTauri()))) {
    const [{ MockBackend }, { scenarioFromSearch }] = await Promise.all([
      import("./mock/mockBackend"),
      import("./mock/scenarios"),
    ]);
    const search = typeof location === "undefined" ? "" : location.search;
    current = new MockBackend({ scenario: scenarioFromSearch(search), latencyMs: 60 });
  } else {
    current = new TauriBackend();
  }
  return current;
}

/** The process-wide backend (after `initBackend`, or `setBackend` in tests). */
export function getBackend(): Backend {
  if (!current) throw new Error("backend not initialised: call initBackend() first");
  return current;
}

/** Tests: install a specific backend (or `null` to reset). */
export function setBackend(backend: Backend | null): void {
  if (current && current !== backend) (current as { dispose?: () => void }).dispose?.();
  current = backend;
}
