import { waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Backend } from "../../ipc/backend";
import { setBackend } from "../../ipc";
import { MockBackend } from "../../ipc/mock/mockBackend";
import type { SessionEvent } from "../../ipc/types";
import { sshConfigTarget } from "../../lib/targets";
import { resetStore, useStore } from "../../state/store";
import { bufferIdFor, closeBuffer, openFile } from "../editor/editorActions";
import { getBufferState } from "../editor/bufferRegistry";
import { closeSession, openSession } from "./sessionActions";

/** Waits until the session is connected (instead of sleeping a fixed time, which flakes under CPU load). */
const connected = (id: string) =>
  waitFor(() => expect(useStore.getState().sessionsById[id]?.state.phase).toBe("connected"), { timeout: 5000 });

describe("openSession", () => {
  beforeEach(() => resetStore());
  afterEach(() => setBackend(null));

  it("queues events that arrive before the command resolves", async () => {
    const mock = new MockBackend();
    const early: Backend = Object.create(mock) as Backend;
    // Deliver the first events synchronously, before `openSession` resolves with the id.
    early.openSession = async (req, onEvent) => {
      const id = await mock.openSession(req, onEvent);
      const hop = { index: 0, label: "x", user: "u", host: "h", port: 22 };
      const e: SessionEvent = { type: "state", state: { phase: "connecting", hop } };
      onEvent(e);
      return id;
    };
    setBackend(early);
    const id = await openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
    expect(useStore.getState().sessionsById[id]?.state.phase).toBe("connecting");
  });

  it("does not open two sessions for a double click", async () => {
    setBackend(new MockBackend({ latencyMs: 20 }));
    const [a, b] = await Promise.all([
      openSession(sshConfigTarget("pi"), "pi").catch(() => ""),
      openSession(sshConfigTarget("pi"), "pi").catch(() => ""),
    ]);
    expect(a).toBe(b);
    const again = await openSession(sshConfigTarget("prod-web-01"), "p");
    const twice = await openSession(sshConfigTarget("prod-web-01"), "p");
    expect(again).toBe(twice);
    expect(useStore.getState().sessionOrder.filter((id) => id === again)).toHaveLength(1);
  });
});

describe("closing a tab right after it connected", () => {
  beforeEach(() => resetStore());
  afterEach(() => setBackend(null));

  it("raises no error toasts and opens no terminal for a session the user just closed", async () => {
    const mock = new MockBackend();
    setBackend(mock);
    const id = await openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
    const opened = vi.spyOn(mock, "openTerminal");
    // The listing is held until the test lets it fail (no timer to race against).
    let failListing: () => void = () => undefined;
    const list = vi.spyOn(mock, "sftpList").mockImplementation(
      () =>
        new Promise((_resolve, reject) => {
          failListing = () => reject({ code: "notConnected", params: { sessionId: id } });
        }),
    );
    await waitFor(() => expect(list).toHaveBeenCalled(), { timeout: 5000 });
    useStore.getState().removeSession(id); // the user closes the tab while the root listing loads
    failListing();
    for (let i = 0; i < 20; i += 1) await Promise.resolve(); // let every continuation of the failed listing run
    expect(useStore.getState().notices).toEqual([]);
    expect(opened).not.toHaveBeenCalled();
  });
});

describe("editor buffers vs closed tabs", () => {
  beforeEach(() => resetStore());
  afterEach(() => setBackend(null));

  it("closing a buffer or the session never leaves its EditorState behind", async () => {
    setBackend(new MockBackend());
    const id = await openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
    await connected(id);
    useStore.getState().ensureWorkspace(id);
    await openFile(id, "/srv/app/docker-compose.yml");
    await openFile(id, "/srv/app/deploy.sh");
    const a = bufferIdFor(id, "/srv/app/docker-compose.yml");
    const b = bufferIdFor(id, "/srv/app/deploy.sh");
    expect(getBufferState(a)).toBeDefined();
    expect(getBufferState(b)).toBeDefined();

    closeBuffer(id, b);
    expect(getBufferState(b)).toBeUndefined();

    await closeSession(id);
    expect(getBufferState(a)).toBeUndefined();
  });

  it("a file read that finishes after the tab was closed does not resurrect the buffer", async () => {
    setBackend(new MockBackend({ latencyMs: 30 }));
    const id = await openSession(sshConfigTarget("prod-web-01"), "prod-web-01");
    await connected(id);
    useStore.getState().ensureWorkspace(id);
    const pending = openFile(id, "/srv/app/README.md");
    closeBuffer(id, bufferIdFor(id, "/srv/app/README.md"));
    await pending;
    expect(getBufferState(bufferIdFor(id, "/srv/app/README.md"))).toBeUndefined();
    expect(useStore.getState().buffers[bufferIdFor(id, "/srv/app/README.md")]).toBeUndefined();
  });
});
