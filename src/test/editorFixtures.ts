import { createBufferState, initBuffer, setBufferState } from "../features/editor/bufferRegistry";

/** Registers a buffer whose live text (`edited`) differs from what was loaded, i.e. a dirty one. */
export function registerEditedBuffer(bufferId: string, path: string, edited: string): void {
  const options = { tabSize: 2, wordWrap: false };
  initBuffer(bufferId, createBufferState(path, `${edited} (original)`, options));
  setBufferState(bufferId, createBufferState(path, edited, options));
}

import { expect, vi } from "vitest";
import type { MockBackend } from "../ipc/mock/mockBackend";
import { useStore } from "../state/store";

/** Opens a session on the mock backend, registers it in the store like the app does, and waits until it is connected. */
export async function openConnectedMockSession(backend: MockBackend, alias = "prod-web-01"): Promise<string> {
  let id = "";
  id = await backend.openSession({ target: { kind: "sshConfig", alias }, probeOnly: false }, (event) => {
    useStore.getState().applySessionEvent(id, event);
  });
  useStore.getState().addSession(id, { kind: "sshConfig", alias }, alias);
  useStore.getState().ensureWorkspace(id);
  await vi.waitFor(() => expect(useStore.getState().sessionsById[id]?.state.phase).toBe("connected"));
  return id;
}
