import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import { MockBackend } from "../../ipc/mock/mockBackend";
import { resetStore, useStore } from "../../state/store";
import { IpcError } from "../../ipc";
import { openConnectedMockSession, registerEditedBuffer } from "../../test/editorFixtures";
import { collectLastDirs, handleQuitRequested, quitCancel, quitDiscard, quitSaveAll } from "./quitFlow";

describe("quit flow (§8.9)", () => {
  let backend: MockBackend;
  beforeEach(() => {
    resetStore();
    backend = new MockBackend();
    setBackend(backend);
  });
  afterEach(() => setBackend(null));

  it("acks, then quits with each session's tree root when nothing is dirty", async () => {
    const s = useStore.getState();
    s.addSession("s1", { kind: "sshConfig", alias: "a" }, "a");
    s.ensureWorkspace("s1");
    s.setTreeRoot("s1", "/srv/app");
    s.addSession("s2", { kind: "sshConfig", alias: "b" }, "b");
    s.ensureWorkspace("s2");

    const ack = vi.spyOn(backend, "appQuitAck");
    const quit = vi.spyOn(backend, "appQuit");
    const cancel = vi.spyOn(backend, "appQuitCancel");
    await handleQuitRequested();

    expect(ack).toHaveBeenCalledOnce();
    expect(cancel).not.toHaveBeenCalled();
    expect(quit).toHaveBeenCalledWith({ s1: "/srv/app", s2: null });
    expect(collectLastDirs()).toEqual({ s1: "/srv/app", s2: null });
  });

  const dirtyBuffer = (id: string, sessionId = "s1") => ({
    id, sessionId, path: "/x", load: "ready" as const, dirty: true, encoding: "utf-8", eol: "lf" as const, etag: null,
    savedAtMs: null,
  });

  it("with dirty buffers: acks, shows the QuitConfirmDialog and does not quit; a repeated request refocuses", async () => {
    const s = useStore.getState();
    s.addSession("s1", { kind: "sshConfig", alias: "a" }, "a");
    s.addBuffer(dirtyBuffer("s1:/x"));
    const quit = vi.spyOn(backend, "appQuit");
    const cancel = vi.spyOn(backend, "appQuitCancel");
    await handleQuitRequested();
    expect(useStore.getState().dialog).toEqual({ kind: "quit" });
    expect(useStore.getState().quit).toBe("confirming");
    expect(quit).not.toHaveBeenCalled();
    expect(cancel).not.toHaveBeenCalled();

    const tick = useStore.getState().quitFocusTick;
    await handleQuitRequested();
    expect(useStore.getState().quitFocusTick).toBe(tick + 1);
    expect(useStore.getState().dialog).toEqual({ kind: "quit" });
  });

  it("Cancel -> app_quit_cancel, Discard -> app_quit without saving", async () => {
    const s = useStore.getState();
    s.addSession("s1", { kind: "sshConfig", alias: "a" }, "a");
    s.addBuffer(dirtyBuffer("s1:/x"));
    const quit = vi.spyOn(backend, "appQuit");
    const cancel = vi.spyOn(backend, "appQuitCancel");
    await handleQuitRequested();
    await quitCancel();
    expect(cancel).toHaveBeenCalledOnce();
    expect(useStore.getState().dialog).toBeNull();
    expect(useStore.getState().quit).toBe("idle");

    await handleQuitRequested();
    await quitDiscard();
    expect(quit).toHaveBeenCalledOnce();
  });

  it("Save all writes every dirty buffer, then quits; a failing save keeps the dialog open", async () => {
    const id = await openConnectedMockSession(backend);
    const s = useStore.getState();
    s.addBuffer(dirtyBuffer(`${id}:/srv/app/README.md`, id));
    // A real etag: saving with `expected: null` onto an existing file is a conflict now.
    const { etag } = await backend.sftpReadText(id, "/srv/app/README.md", null);
    useStore.getState().patchBuffer(`${id}:/srv/app/README.md`, { path: "/srv/app/README.md", etag });
    registerEditedBuffer(`${id}:/srv/app/README.md`, "/srv/app/README.md", "edited");
    const write = vi.spyOn(backend, "sftpWriteText");
    const quit = vi.spyOn(backend, "appQuit");

    await handleQuitRequested();
    await quitSaveAll();
    expect(write).toHaveBeenCalledWith(id, expect.objectContaining({ path: "/srv/app/README.md", text: "edited", force: false }));
    expect(useStore.getState().buffers[`${id}:/srv/app/README.md`]?.dirty).toBe(false);
    expect(quit).toHaveBeenCalledOnce();

    // failure path
    quit.mockClear();
    registerEditedBuffer(`${id}:/srv/app/README.md`, "/srv/app/README.md", "edited");
    useStore.getState().patchBuffer(`${id}:/srv/app/README.md`, { dirty: true });
    write.mockRejectedValueOnce(new IpcError({ code: "remoteChanged", params: { path: "/srv/app/README.md", size: 1, mtime: 1 } }));
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    await handleQuitRequested();
    await quitSaveAll();
    expect(quit).not.toHaveBeenCalled();
    expect(useStore.getState().dialog).toEqual({ kind: "quit" });
    expect(useStore.getState().notices.at(-1)?.error?.code).toBe("remoteChanged");
  });
});
