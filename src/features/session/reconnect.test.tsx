import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import { MockBackend } from "../../ipc/mock/mockBackend";
import { makeBootstrap } from "../../ipc/mock/fixtures";
import { resetStore, useStore } from "../../state/store";
import { firstSessionId, settleApp, setupApp, setupConnected } from "../../test/harness";
import { openSession } from "./sessionActions";
import { sshConfigTarget } from "../../lib/targets";
import { installResumeProbe, HIDDEN_PROBE_AFTER_MS, SUSPEND_GAP_MS } from "./resumeProbe";

afterEach(async () => {
  vi.useRealTimers();
  await settleApp();
  setBackend(null);
  vi.restoreAllMocks();
});

describe("reconnect wiring against the §5.8 sequence (autoReconnect)", () => {
  it("overlay stages, 'Reconnect now' only while waiting, SFTP failing while down, tree refresh and recovery", async () => {
    const { user, backend } = setupApp({ scenario: "autoReconnect", dropAfterMs: 5, latencyMs: 1 });
    await openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
    await waitFor(() => expect(useStore.getState().sessionsById[firstSessionId()]?.state.phase).toBe("connected"));
    const id = firstSessionId();
    await screen.findByRole("treeitem", { name: /docker-compose\.yml/ });
    const list = vi.spyOn(backend, "sftpList");

    // waiting (attempt 1)
    const overlay = await screen.findByRole("group", { name: "Connection to prod-web-01 lost" });
    await within(overlay).findByText(/Reconnecting in \d+ s/);
    // SFTP calls fail with notConnected while the connection is down (§5.8: in-flight requests fail)
    await expect(backend.sftpList(id, "/srv/app")).rejects.toMatchObject({ error: { code: "notConnected" } });
    const callsWhileDown = list.mock.calls.length;

    await user.click(within(overlay).getByRole("button", { name: "Reconnect now" }));
    // attempt 1 connects, then fails (the mock refuses once): the next wait is attempt 2
    await within(overlay).findByText(/Attempt 2 of 5/, undefined, { timeout: 3000 });
    expect(within(overlay).getByText(/Reconnecting in \d+ s|Connecting to|Authenticating/)).toBeTruthy();

    await waitFor(() => expect(within(overlay).getByRole("button", { name: "Reconnect now" }).hasAttribute("disabled")).toBe(false), {
      timeout: 3000,
    });
    await user.click(within(overlay).getByRole("button", { name: "Reconnect now" }));
    await waitFor(() => expect(useStore.getState().sessionsById[id]?.state.phase).toBe("connected"), { timeout: 3000 });
    // after the recovery the tree refreshed the expanded folders...
    await waitFor(() => expect(list.mock.calls.length).toBeGreaterThan(callsWhileDown));
    expect(screen.queryByRole("group", { name: "Connection to prod-web-01 lost" })).toBeNull();
  });

  it("'Reconnect now' is disabled while an attempt is running", async () => {
    const { backend } = await setupConnected();
    const id = firstSessionId();
    backend.loseConnection(id);
    const overlay = await screen.findByRole("group", { name: "Connection to prod-web-01 lost" });
    expect(within(overlay).getByRole("button", { name: "Reconnect now" }).hasAttribute("disabled")).toBe(false);
    act(() => {
      useStore.getState().applySessionEvent(id, {
        type: "state",
        state: {
          phase: "reconnecting",
          stage: "authenticating",
          hop: { index: 0, label: "x", user: "u", host: "h", port: 22 },
          attempt: 1,
          maxAttempts: 5,
          nextAttemptAtMs: null,
          cause: { code: "disconnected", params: { reason: "keepaliveTimeout", message: null } },
        },
      });
    });
    expect(within(overlay).getByRole("button", { name: "Reconnect now" }).hasAttribute("disabled")).toBe(true);
    backend.restoreConnection(id);
    await waitFor(() => expect(screen.queryByRole("group", { name: "Connection to prod-web-01 lost" })).toBeNull());
  });

  it("terminals: the backend's `reconnected` marker reaches an open terminal and the tab is not duplicated", async () => {
    const { backend } = await setupConnected();
    const id = firstSessionId();
    await waitFor(() => expect(useStore.getState().workspaces[id]?.terminalTabs).toHaveLength(1));
    backend.loseConnection(id);
    backend.restoreConnection(id);
    await waitFor(() => expect(useStore.getState().sessionsById[id]?.state.phase).toBe("connected"));
    await settleApp();
    expect(useStore.getState().workspaces[id]?.terminalTabs).toHaveLength(1); // no extra terminal on reconnect
  });
});

describe("session_probe_all on resume (§5.7)", () => {
  function connectedStore() {
    resetStore();
    const backend = new MockBackend();
    setBackend(backend);
    useStore.getState().setBootstrap(makeBootstrap(Date.now()));
    useStore.getState().addSession("s1", { kind: "sshConfig", alias: "a" }, "a");
    useStore.getState().applySessionEvent("s1", {
      type: "state",
      state: {
        phase: "connected",
        info: {
          sessionId: "s1",
          label: "a",
          hops: [],
          auth: { method: "publickey", keyType: null, keySource: null },
          sftpAvailable: true,
          homeDir: "/",
          startDir: "/",
          remotePathStyle: "posix",
          remoteIdentity: null,
          limitOverrides: { uploadKibps: null, downloadKibps: null },
      color: null,
        },
      },
    });
    return backend;
  }

  it("probes on `online`, at most once per 10 s, and only with a connected session", async () => {
    const backend = connectedStore();
    let now = 1_000_000;
    const uninstall = installResumeProbe({ now: () => now });
    window.dispatchEvent(new Event("online"));
    window.dispatchEvent(new Event("online"));
    expect(backend.probeCount).toBe(1);
    now += 11_000;
    window.dispatchEvent(new Event("online"));
    expect(backend.probeCount).toBe(2);
    now += 11_000;
    useStore.getState().removeSession("s1");
    window.dispatchEvent(new Event("online"));
    expect(backend.probeCount).toBe(2);
    uninstall();
  });

  it("probes when the window becomes visible after more than 60 s hidden, not after a short blink", async () => {
    const backend = connectedStore();
    let now = 1_000_000;
    const state = { value: "visible" };
    Object.defineProperty(document, "visibilityState", { configurable: true, get: () => state.value });
    const uninstall = installResumeProbe({ now: () => now });
    const toggle = (v: string) => {
      state.value = v;
      document.dispatchEvent(new Event("visibilitychange"));
    };
    toggle("hidden");
    now += HIDDEN_PROBE_AFTER_MS - 1000;
    toggle("visible");
    expect(backend.probeCount).toBe(0);
    toggle("hidden");
    now += HIDDEN_PROBE_AFTER_MS + 1000;
    toggle("visible");
    expect(backend.probeCount).toBe(1);
    uninstall();
    // @ts-expect-error restore the jsdom default
    delete document.visibilityState;
  });

  it("probes after a timer gap that only a suspended machine produces", async () => {
    vi.useFakeTimers();
    const backend = connectedStore();
    let now = 1_000_000;
    const uninstall = installResumeProbe({ now: () => now });
    now += 5_000;
    vi.advanceTimersByTime(5_000);
    expect(backend.probeCount).toBe(0); // a normal beat
    now += SUSPEND_GAP_MS + 5_000; // the laptop slept
    vi.advanceTimersByTime(5_000);
    expect(backend.probeCount).toBe(1);
    uninstall();
  });
});
