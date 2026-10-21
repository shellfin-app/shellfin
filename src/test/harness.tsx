import { render, screen, waitFor } from "@testing-library/react";
import { expect } from "vitest";
import userEvent from "@testing-library/user-event";
import { setBackend } from "../ipc";
import { makeAgent, makeBootstrap, makeEnv } from "../ipc/mock/fixtures";
import { MockBackend, type MockBackendOptions } from "../ipc/mock/mockBackend";
import { App } from "../App";
import { installDropHandlers } from "../features/files/dropFiles";
import { openSession } from "../features/session/sessionActions";
import { sshConfigTarget } from "../lib/targets";
import { resetStore, useStore } from "../state/store";

/** Renders the whole app (shell + dialogs + notices + shortcuts) against a MockBackend scenario. */
export function setupApp(options: MockBackendOptions = {}) {
  resetStore();
  const backend = new MockBackend(options);
  setBackend(backend);
  const now = Date.now();
  const s = useStore.getState();
  s.setBootstrap(makeBootstrap(now));
  s.setEnv(makeEnv(now));
  s.setAgent(makeAgent());
  void installDropHandlers(); // what bootstrap does; MockBackend.listen registers synchronously
  const view = render(<App />);
  return { backend, user: userEvent.setup(), ...view };
}

/** The (first) session's current state phase. */
export function firstPhase(): string | undefined {
  const { sessionsById, sessionOrder } = useStore.getState();
  const id = sessionOrder[0];
  return id ? sessionsById[id]?.state.phase : undefined;
}

export function firstSessionId(): string {
  const id = useStore.getState().sessionOrder[0];
  if (!id) throw new Error("no session");
  return id;
}

/** `setupApp` + connect to prod-web-01 and wait for the file tree (set `tree: false` when there is none). */
export async function setupConnected(options: MockBackendOptions & { tree?: boolean } = {}) {
  const { tree = true, ...backendOptions } = options;
  const ctx = setupApp(backendOptions);
  await openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
  await waitFor(() => expect(firstPhase()).toBe("connected"));
  const sessionId = firstSessionId();
  if (tree) await screen.findByRole("treeitem", { name: /docker-compose\.yml/ });
  return { ...ctx, sessionId };
}

/** Lets in-flight async work of the finished test (terminal opens, listings ...) land before the next test resets the store. */
export async function settleApp(ms = 120): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, ms));
}
