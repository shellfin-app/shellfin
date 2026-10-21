import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import { IpcError } from "../../ipc/call";
import type { SaveRequest } from "../../ipc/types";
import { MockBackend } from "../../ipc/mock/mockBackend";
import { useStore } from "../../state/store";
import { settleApp, setupConnected } from "../../test/harness";
import { openFile, saveBuffer } from "./editorActions";
import { getBufferState, setBufferState } from "./bufferRegistry";

afterEach(async () => {
  await settleApp();
  setBackend(null);
  vi.restoreAllMocks();
});

interface Fs {
  addLink(p: string, t: string): void;
  repointLink(p: string, t: string): void;
  externalEdit(p: string, t: string): void;
  read(p: string, enc: string | null, limit: number): { text: string };
}
const fsOf = (backend: unknown) => (backend as { fs: Fs }).fs;
const LINK = "/srv/app/current.conf";
const REAL = "/srv/app/nginx/nginx.conf";
const OTHER = "/srv/app/nginx/mime.types";

function edit(sessionId: string, path: string, text: string) {
  const state = getBufferState(`${sessionId}:${path}`);
  if (!state) throw new Error("no buffer state");
  setBufferState(`${sessionId}:${path}`, state.update({ changes: { from: 0, insert: text } }).state);
  useStore.getState().patchBuffer(`${sessionId}:${path}`, { dirty: true });
}

describe("MockBackend: symlink targets and expected: null", () => {
  const created: MockBackend[] = [];
  afterEach(() => {
    for (const b of created.splice(0)) b.dispose();
  });
  const req = (over: Partial<SaveRequest>): SaveRequest => ({
    path: LINK,
    text: "x\n",
    encoding: "utf-8",
    hasBom: false,
    eol: "lf",
    expected: null,
    force: false,
    ...over,
  });
  async function connected() {
    const backend = new MockBackend();
    created.push(backend);
    let onConnected: () => void = () => undefined;
    const connectedEvent = new Promise<void>((resolve) => (onConnected = resolve));
    const id = await backend.openSession({ target: { kind: "sshConfig", alias: "prod-web-01" }, probeOnly: false }, (e) => {
      if (e.type === "state" && e.state.phase === "connected") onConnected();
    });
    await connectedEvent;
    fsOf(backend).addLink(LINK, REAL);
    return { backend, id };
  }

  it("reads through a link and reports resolvedPath only for links", async () => {
    const { backend, id } = await connected();
    expect((await backend.sftpReadText(id, LINK, null)).resolvedPath).toBe(REAL);
    expect((await backend.sftpReadText(id, REAL, null)).resolvedPath).toBeUndefined();
  });

  it("expected: null onto an existing file is remoteChanged unless forced", async () => {
    const { backend, id } = await connected();
    await expect(backend.sftpWriteText(id, req({ path: REAL }))).rejects.toMatchObject({ error: { code: "remoteChanged" } });
    await expect(backend.sftpWriteText(id, req({ path: REAL, force: true }))).resolves.toBeTruthy();
    await expect(backend.sftpWriteText(id, req({ path: "/srv/app/brand-new.txt" }))).resolves.toBeTruthy();
  });

  it("a re-pointed link is remoteChanged with the target; force writes to the new target", async () => {
    const { backend, id } = await connected();
    const doc = await backend.sftpReadText(id, LINK, null);
    fsOf(backend).repointLink(LINK, OTHER);
    const save = req({ expected: doc.etag, expectedTarget: doc.resolvedPath });
    const err = await backend.sftpWriteText(id, save).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(IpcError);
    expect((err as IpcError).error).toMatchObject({ code: "remoteChanged", params: { path: LINK, target: OTHER } });
    await backend.sftpWriteText(id, { ...save, force: true });
    expect((await backend.sftpReadText(id, OTHER, null)).text).toBe("x\n");
  });

  it("an unchanged link saves through to its target", async () => {
    const { backend, id } = await connected();
    const doc = await backend.sftpReadText(id, LINK, null);
    await backend.sftpWriteText(id, req({ expected: doc.etag, expectedTarget: doc.resolvedPath }));
    expect((await backend.sftpReadText(id, REAL, null)).text).toBe("x\n");
  });
});

describe("expectedTarget in save requests", () => {
  it("is kept from open and sent by save, save-all and queued saves; absent for plain files", async () => {
    const { backend, sessionId } = await setupConnected();
    fsOf(backend).addLink(LINK, REAL);
    const write = vi.spyOn(backend, "sftpWriteText");
    const id = `${sessionId}:${LINK}`;

    await openFile(sessionId, LINK);
    await waitFor(() => expect(useStore.getState().buffers[id]?.load).toBe("ready"));
    expect(useStore.getState().buffers[id]?.resolvedPath).toBe(REAL);

    edit(sessionId, LINK, "a\n");
    await act(() => saveBuffer(id));
    expect(write.mock.calls.at(-1)?.[1]).toMatchObject({ path: LINK, expectedTarget: REAL });

    // queued save after a reconnect
    edit(sessionId, LINK, "b\n");
    backend.loseConnection(sessionId);
    await act(() => saveBuffer(id));
    expect(useStore.getState().buffers[id]?.pendingSave).toBe(true);
    backend.restoreConnection(sessionId);
    await waitFor(() => expect(write.mock.calls.length).toBe(2));
    expect(write.mock.calls.at(-1)?.[1]).toMatchObject({ expectedTarget: REAL });

    // save all
    edit(sessionId, LINK, "c\n");
    const { saveAllBuffers } = await import("./editorActions");
    await act(() => saveAllBuffers({ interactive: true }));
    expect(write.mock.calls.at(-1)?.[1]).toMatchObject({ expectedTarget: REAL });

    // a plain file never sends it
    const plain = `${sessionId}:${OTHER}`;
    await openFile(sessionId, OTHER);
    await waitFor(() => expect(useStore.getState().buffers[plain]?.load).toBe("ready"));
    expect(useStore.getState().buffers[plain]?.resolvedPath).toBeNull();
    edit(sessionId, OTHER, "z\n");
    await act(() => saveBuffer(plain));
    expect(write.mock.calls.at(-1)?.[1]).not.toHaveProperty("expectedTarget");
  });

  it("a re-pointed link opens the conflict dialog with the actual target path", async () => {
    const { backend, sessionId } = await setupConnected();
    fsOf(backend).addLink(LINK, REAL);
    const id = `${sessionId}:${LINK}`;
    await openFile(sessionId, LINK);
    await waitFor(() => expect(useStore.getState().buffers[id]?.load).toBe("ready"));
    fsOf(backend).repointLink(LINK, OTHER);
    edit(sessionId, LINK, "mine\n");
    await act(() => saveBuffer(id));
    expect(useStore.getState().buffers[id]?.conflict).toMatchObject({ target: OTHER });
    const dialog = await screen.findByRole("alertdialog", { name: "current.conf changed on the server" });
    expect(dialog.textContent).toContain("Overwrite writes to this file instead");
    expect(within(dialog).getByText(OTHER)).toBeTruthy();
    expect(dialog.textContent).not.toContain("Saving writes to the file it points to");
  });
});
