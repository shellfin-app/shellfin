/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** "mock": build the MockBackend in (dev server / `--mode smoke` only). "tauri": force TauriBackend in dev. */
  readonly VITE_BACKEND?: "mock" | "tauri";
}
