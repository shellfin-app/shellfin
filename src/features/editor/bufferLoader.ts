// CodeMirror is loaded on demand (docs/ARCHITECTURE.md §8.6, same idea as `terminalLoader.ts`): the entry chunk holds
// only the light buffer store, and the editor, its extensions and the language parsers arrive with the first file that
// is opened.
import type * as RegistryModule from "./bufferRegistry";

type BufferRegistry = typeof RegistryModule;

let loaded: BufferRegistry | null = null;

/** Loads (once) the CodeMirror part of the buffers. A failed load is not cached, so the next call retries. */
export async function loadBufferRegistry(): Promise<BufferRegistry> {
  // The view component rides along, so showing the first file does not wait for a second chunk.
  loaded ??= (await Promise.all([import("./bufferRegistry"), import("./CodeEditor")]))[0];
  return loaded;
}

/** The registry if it was loaded already (it was, as soon as any buffer exists). */
export function loadedBufferRegistry(): BufferRegistry | null {
  return loaded;
}
