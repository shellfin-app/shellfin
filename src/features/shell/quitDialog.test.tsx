import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import { useStore } from "../../state/store";
import { setupApp } from "../../test/harness";
import { openConnectedMockSession, registerEditedBuffer } from "../../test/editorFixtures";

afterEach(() => setBackend(null));

function dirty(sessionId: string, path: string) {
  const id = `${sessionId}:${path}`;
  useStore.getState().addBuffer({
    id, sessionId, path, load: "ready", dirty: true, encoding: "utf-8", eol: "lf", etag: null, savedAtMs: null,
  });
  registerEditedBuffer(id, path, "changed");
  return id;
}

describe("QuitConfirmDialog (§8.9)", () => {
  it("app:quit-requested with dirty buffers: lists the files; Cancel keeps the app running", async () => {
    const { user, backend } = setupApp();
    const sessionId = await backend.openSession({ target: { kind: "sshConfig", alias: "prod-web-01" }, probeOnly: false }, () => undefined);
    useStore.getState().addSession(sessionId, { kind: "sshConfig", alias: "prod-web-01" }, "prod-web-01");
    dirty(sessionId, "/srv/app/docker-compose.yml");
    dirty(sessionId, "/srv/app/deploy.sh");
    const ack = vi.spyOn(backend, "appQuitAck");
    const cancel = vi.spyOn(backend, "appQuitCancel");
    const quit = vi.spyOn(backend, "appQuit");
    const { installQuitFlow } = await import("../session/quitFlow");
    await installQuitFlow();

    act(() => backend.emit("app:quit-requested", {}));
    const dialog = await screen.findByRole("alertdialog", { name: "Unsaved changes" });
    expect(ack).toHaveBeenCalledOnce();
    expect(dialog.textContent).toContain("2 files have unsaved changes: docker-compose.yml and deploy.sh");
    expect(quit).not.toHaveBeenCalled();

    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(cancel).toHaveBeenCalledOnce());
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(quit).not.toHaveBeenCalled();
  });

  it("Save all saves, then quits with the tree roots; Discard quits immediately", async () => {
    const { user, backend } = setupApp();
    const sessionId = await openConnectedMockSession(backend);
    useStore.getState().setTreeRoot(sessionId, "/srv/app");
    const bufferId = dirty(sessionId, "/srv/app/README.md");
    // A real etag: saving with `expected: null` onto an existing file is a conflict now.
    useStore.getState().patchBuffer(bufferId, { etag: (await backend.sftpReadText(sessionId, "/srv/app/README.md", null)).etag });
    const write = vi.spyOn(backend, "sftpWriteText");
    const quit = vi.spyOn(backend, "appQuit");
    const { handleQuitRequested } = await import("../session/quitFlow");

    await act(async () => handleQuitRequested());
    await user.click(await screen.findByRole("button", { name: "Save all" }));
    await waitFor(() => expect(quit).toHaveBeenCalledWith({ [sessionId]: "/srv/app" }));
    expect(write).toHaveBeenCalledOnce();
    expect(useStore.getState().buffers[bufferId]?.dirty).toBe(false);

    quit.mockClear();
    registerEditedBuffer(bufferId, "/srv/app/README.md", "changed again");
    useStore.getState().patchBuffer(bufferId, { dirty: true });
    await act(async () => handleQuitRequested());
    await user.click(await screen.findByRole("button", { name: "Discard changes" }));
    await waitFor(() => expect(quit).toHaveBeenCalledOnce());
    expect(write).toHaveBeenCalledOnce();
  });

  it("a repeated quit request moves focus back into the open dialog", async () => {
    const { backend } = setupApp();
    const sessionId = "s1";
    useStore.getState().addSession(sessionId, { kind: "sshConfig", alias: "a" }, "a");
    dirty(sessionId, "/x");
    const { handleQuitRequested } = await import("../session/quitFlow");
    await act(async () => handleQuitRequested());
    const dialog = await screen.findByRole("alertdialog");
    (within(dialog).getByRole("button", { name: "Cancel" }) as HTMLElement).focus();
    await act(async () => handleQuitRequested());
    await waitFor(() => expect(dialog.contains(document.activeElement)).toBe(true));
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Save all" }));
    void backend;
  });
});
