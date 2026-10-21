import { isolateHistory, undo } from "@codemirror/commands";
import { EditorView } from "@codemirror/view";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import type { FileEntry } from "../../ipc/types";
import { conflictDialog } from "../../lib/dialogs";
import { useStore } from "../../state/store";
import { settleApp, setupConnected } from "../../test/harness";
import { runPendingSaves } from "./editorActions";

afterEach(async () => {
  await settleApp();
  setBackend(null);
  vi.restoreAllMocks();
});

const view = (): EditorView => {
  const el = document.querySelector<HTMLElement>(".cm-editor");
  const found = el ? EditorView.findFromDOM(el) : null;
  if (!found) throw new Error("no editor view");
  return found;
};
const insert = (text: string) =>
  act(() => view().dispatch({ changes: { from: 0, insert: text }, userEvent: "input.type", annotations: isolateHistory.of("full") }));
const buffer = (sessionId: string, path: string) => useStore.getState().buffers[`${sessionId}:${path}`];
interface Fs {
  remove(p: string): void;
  has(p: string): boolean;
}
const fsOf = (backend: unknown) => (backend as { fs: Fs }).fs;

async function openReadme(user: Awaited<ReturnType<typeof setupConnected>>["user"]) {
  await user.click(screen.getByRole("treeitem", { name: /README\.md/ }));
  await waitFor(() => expect(document.querySelector(".cm-editor")).toBeTruthy());
}

describe("A file deleted on the server", () => {
  it("has its own text, no Reload, and 'Save again' recreates the file", async () => {
    const { user, backend } = await setupConnected();
    await openReadme(user);
    insert("mine\n");
    fsOf(backend).remove("/srv/app/README.md");
    act(() => view().focus());
    await user.keyboard("{Control>}s{/Control}");
    const dialog = await screen.findByRole("alertdialog", { name: "README.md was deleted on the server" });
    expect(within(dialog).queryByRole("button", { name: "Reload from server" })).toBeNull();
    expect(dialog.textContent).not.toContain("1970");
    await user.click(within(dialog).getByRole("button", { name: "Save again" }));
    await waitFor(() => expect(fsOf(backend).has("/srv/app/README.md")).toBe(true));
  });

  it("a failed Reload snoozes the conflict instead of re-opening the dialog", async () => {
    const { user, backend } = await setupConnected({ scenario: "saveConflict" });
    await openReadme(user);
    insert("mine\n");
    act(() => view().focus());
    await user.keyboard("{Control>}s{/Control}");
    const dialog = await screen.findByRole("alertdialog", { name: "README.md changed on the server" });
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    vi.spyOn(backend, "sftpReadText").mockRejectedValue({ code: "permissionDenied", params: { path: "/srv/app/README.md" } });
    await user.click(within(dialog).getByRole("button", { name: "Reload from server" }));
    await waitFor(() => expect(useStore.getState().notices.length).toBeGreaterThan(0));
    await settleApp(200);
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });
});

describe("The queued-save flag does not outlive the queue", () => {
  it("undoing back to the saved text while offline clears it", async () => {
    const { user, backend, sessionId } = await setupConnected();
    await openReadme(user);
    insert("x");
    backend.loseConnection(sessionId);
    act(() => view().focus());
    await user.keyboard("{Control>}s{/Control}");
    await waitFor(() => expect(buffer(sessionId, "/srv/app/README.md")?.pendingSave).toBe(true));
    act(() => {
      undo(view());
    });
    await waitFor(() => expect(buffer(sessionId, "/srv/app/README.md")).toMatchObject({ dirty: false, pendingSave: false }));
    expect(screen.queryByText("Will save after reconnecting")).toBeNull();
  });

  it("a queued save that fails with another error is no longer queued", async () => {
    const { user, backend, sessionId } = await setupConnected();
    await openReadme(user);
    insert("x");
    backend.loseConnection(sessionId);
    act(() => view().focus());
    await user.keyboard("{Control>}s{/Control}");
    await waitFor(() => expect(buffer(sessionId, "/srv/app/README.md")?.pendingSave).toBe(true));
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    vi.spyOn(backend, "sftpWriteText").mockRejectedValue({ code: "permissionDenied", params: { path: "/srv/app/README.md" } });
    backend.restoreConnection(sessionId);
    await runPendingSaves(sessionId);
    expect(buffer(sessionId, "/srv/app/README.md")).toMatchObject({ pendingSave: false, dirty: true });
  });

  it("A conflict found by a queued save never replaces an open dialog; it opens once that dialog is closed", async () => {
    const { user, backend, sessionId } = await setupConnected();
    await openReadme(user);
    insert("x");
    backend.loseConnection(sessionId);
    act(() => view().focus());
    await user.keyboard("{Control>}s{/Control}");
    await waitFor(() => expect(buffer(sessionId, "/srv/app/README.md")?.pendingSave).toBe(true));
    vi.spyOn(backend, "sftpWriteText").mockRejectedValue({ code: "remoteChanged", params: { size: 3, mtime: 4 } });
    backend.restoreConnection(sessionId);
    act(() => useStore.getState().openDialog({ kind: "settings" })); // the user is busy in Settings
    await runPendingSaves(sessionId);
    expect(useStore.getState().dialog).toEqual({ kind: "settings" });
    expect(buffer(sessionId, "/srv/app/README.md")?.conflict).not.toBeNull();
    act(() => useStore.getState().closeDialog());
    await waitFor(() => expect(useStore.getState().dialog).toEqual(conflictDialog(`${sessionId}:/srv/app/README.md`)));
  });
});

describe("SFTP that only works after a reconnect", () => {
  it("sets the tree root from the new info and loads it", async () => {
    const { backend, sessionId } = await setupConnected();
    act(() => {
      useStore.setState((s) => {
        const tree = s.workspaces[sessionId]?.tree;
        if (tree) {
          tree.root = null;
          tree.listings = {};
          tree.expanded = {};
        }
      });
    });
    expect(screen.queryByRole("treeitem", { name: /docker-compose/ })).toBeNull();
    backend.loseConnection(sessionId);
    backend.restoreConnection(sessionId);
    await screen.findByRole("treeitem", { name: /docker-compose\.yml/ });
    expect(useStore.getState().workspaces[sessionId]?.tree.root).toBe("/srv/app");
  });
});

describe("Encoding names follow the backend", () => {
  it("offers windows-1252 once, and the mock answers iso-8859-1 as windows-1252", async () => {
    const { user, backend, sessionId } = await setupConnected();
    await openReadme(user);
    const doc = await backend.sftpReadText(sessionId, "/srv/app/README.md", "ISO-8859-1");
    expect(doc.encoding).toBe("windows-1252");
    await user.click(within(screen.getByRole("contentinfo")).getByRole("button", { name: "Encoding: UTF-8" }));
    const picker = await screen.findByRole("dialog", { name: "Encoding of README.md" });
    expect(within(picker).queryByRole("radio", { name: "ISO-8859-1" })).toBeNull();
    expect(within(picker).getAllByRole("radio", { name: "WINDOWS-1252" })).toHaveLength(1);
  });
});

describe("Symbolic links are called out", () => {
  it("the conflict dialog warns when the path is a symlink in the tree", async () => {
    const { sessionId } = await setupConnected();
    const link: FileEntry = {
      name: "current.conf",
      path: "/srv/app/current.conf",
      kind: "symlink",
      linkTargetKind: "file",
      size: 1,
      mtime: 1,
      permissions: 0o777,
      uid: 0,
      gid: 0,
      owner: null,
      group: null,
      lossyName: false,
    };
    const s = useStore.getState();
    s.setListing(sessionId, "/srv/app", { path: "/srv/app", entries: [link], truncated: false }, Date.now());
    s.addBuffer({ id: `${sessionId}:${link.path}`, sessionId, path: link.path, load: "ready", conflict: { size: 5, mtime: 5 } });
    act(() => useStore.getState().openDialog(conflictDialog(`${sessionId}:${link.path}`)));
    const dialog = await screen.findByRole("alertdialog", { name: "current.conf changed on the server" });
    expect(dialog.textContent).toContain("This path is a symbolic link");
  });
});

describe("Host key changed during the outage", () => {
  it("the reconnect overlay shows file:line and the ssh-keygen -R hint with a copy button", async () => {
    const { sessionId } = await setupConnected();
    act(() => {
      useStore.getState().applySessionEvent(sessionId, {
        type: "state",
        state: {
          phase: "disconnected",
          canRetry: false,
          error: {
            code: "hostKeyChanged",
            params: {
              host: "h",
              port: 22,
              keyType: "ssh-ed25519",
              fingerprint: "SHA256:abc",
              knownHostsFile: "~/.ssh/known_hosts",
              line: 7,
              removeHint: "ssh-keygen -R h -f ~/.ssh/known_hosts",
              removeHintShell: "posix",
            },
          },
        },
      });
    });
    const overlay = await screen.findByRole("group", { name: "Connection to prod-web-01 lost" });
    expect(overlay.textContent).toContain("~/.ssh/known_hosts:7");
    expect(within(overlay).getByText("ssh-keygen -R h -f ~/.ssh/known_hosts")).toBeTruthy();
    expect(within(overlay).getByRole("button", { name: "Copy" })).toBeTruthy();
    fireEvent.keyDown(document.body, { key: "Escape" });
  });
});
