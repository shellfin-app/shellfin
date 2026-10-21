import { isolateHistory, redo, undo } from "@codemirror/commands";
import { EditorView } from "@codemirror/view";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import { useStore } from "../../state/store";
import { settleApp, setupConnected } from "../../test/harness";

afterEach(async () => {
  await settleApp();
  setBackend(null);
  vi.restoreAllMocks();
});

const row = (name: RegExp | string) => screen.getByRole("treeitem", { name });
const view = (): EditorView => {
  const el = document.querySelector<HTMLElement>(".cm-editor");
  const found = el ? EditorView.findFromDOM(el) : null;
  if (!found) throw new Error("no editor view");
  return found;
};
const docText = () => view().state.doc.toString();
// Each insert is its own undo step (CodeMirror would otherwise group edits made within 500 ms).
const insert = (text: string, from = 0) =>
  act(() => view().dispatch({ changes: { from, insert: text }, userEvent: "input.type", annotations: isolateHistory.of("full") }));
const bufferOf = (sessionId: string, path: string) => useStore.getState().buffers[`${sessionId}:${path}`];

async function openFromTree(user: Awaited<ReturnType<typeof setupConnected>>["user"], name: RegExp | string, ready = true) {
  await user.click(row(name));
  if (ready) await waitFor(() => expect(document.querySelector(".cm-editor")).toBeTruthy());
}

describe("editor buffers", () => {
  it("opens a file with language, encoding and EOL in the status bar", async () => {
    const { user } = await setupConnected();
    await openFromTree(user, "docker-compose.yml");
    expect(docText()).toContain("services:");
    const status = screen.getByRole("contentinfo");
    expect(within(status).getByText("YAML")).toBeTruthy();
    expect(within(status).getByRole("button", { name: "Encoding: UTF-8" })).toBeTruthy();
    expect(within(status).getByRole("button", { name: "Line endings: LF" })).toBeTruthy();
    expect(within(status).getByText("Ln 1, Col 1")).toBeTruthy();
  });

  it("undo history and selection survive tab switches (§6.5)", async () => {
    const { user, sessionId } = await setupConnected();
    await openFromTree(user, "README.md");
    insert("AAA ");
    insert("BBB ", 4);
    expect(docText().startsWith("AAA BBB # app")).toBe(true);

    await openFromTree(user, "deploy.sh");
    expect(docText()).toContain("#!/bin/sh");
    insert("# note\n");
    expect(bufferOf(sessionId, "/srv/app/README.md")?.dirty).toBe(true);

    // back to the first tab: text intact, then undo steps through ITS history, not the other tab's
    await user.click(screen.getByRole("tab", { name: /README\.md/ }));
    await waitFor(() => expect(docText().startsWith("AAA BBB # app")).toBe(true));
    act(() => {
      undo(view());
    });
    expect(docText().startsWith("AAA # app")).toBe(true);
    act(() => {
      undo(view());
    });
    expect(docText().startsWith("# app")).toBe(true);
    act(() => {
      redo(view());
    });
    expect(docText().startsWith("AAA # app")).toBe(true);

    await user.click(screen.getByRole("tab", { name: /deploy\.sh/ }));
    await waitFor(() => expect(docText().startsWith("# note\n#!/bin/sh")).toBe(true));
    act(() => {
      undo(view());
    });
    expect(docText().startsWith("#!/bin/sh")).toBe(true);
  });

  it("the dirty marker follows the text: typing sets it, undoing back to the saved text clears it", async () => {
    const { user, sessionId } = await setupConnected();
    await openFromTree(user, "README.md");
    const tab = () => screen.getByRole("tab", { name: /README\.md/ });
    expect(within(tab()).queryByRole("img", { name: "Unsaved changes" })).toBeNull();
    insert("x");
    expect(within(tab()).getByRole("img", { name: "Unsaved changes" })).toBeTruthy();
    expect(row(/README.md/).textContent).toContain("M");
    act(() => {
      undo(view());
    });
    await waitFor(() => expect(bufferOf(sessionId, "/srv/app/README.md")?.dirty).toBe(false));
    expect(within(tab()).queryByRole("img", { name: "Unsaved changes" })).toBeNull();
  });

  it("Mod+S saves with the etag, clears the marker and shows 'Saved to server'", async () => {
    const { user, backend, sessionId } = await setupConnected();
    const write = vi.spyOn(backend, "sftpWriteText");
    await openFromTree(user, "README.md");
    const before = bufferOf(sessionId, "/srv/app/README.md")?.etag;
    insert("new line\n");
    act(() => view().focus());
    await user.keyboard("{Control>}s{/Control}");
    await waitFor(() => expect(write).toHaveBeenCalledOnce());
    expect(write.mock.calls[0]?.[1]).toMatchObject({
      path: "/srv/app/README.md",
      encoding: "utf-8",
      eol: "lf",
      expected: before,
      force: false,
    });
    expect(write.mock.calls[0]?.[1].text.startsWith("new line\n# app")).toBe(true);
    await waitFor(() => expect(bufferOf(sessionId, "/srv/app/README.md")?.dirty).toBe(false));
    expect(bufferOf(sessionId, "/srv/app/README.md")?.etag).not.toEqual(before);
    expect(screen.getByText(/Saved to server/)).toBeTruthy();
  });

  it("Mod+Alt+S saves every dirty buffer", async () => {
    const { user, backend } = await setupConnected();
    const write = vi.spyOn(backend, "sftpWriteText");
    await openFromTree(user, "README.md");
    insert("1");
    await openFromTree(user, "deploy.sh");
    insert("2");
    act(() => view().focus());
    await user.keyboard("{Control>}{Alt>}s{/Alt}{/Control}");
    await waitFor(() => expect(write).toHaveBeenCalledTimes(2));
  });

  it("Ctrl+Tab and Ctrl+Shift+Tab switch editor tabs; Mod+W closes the active one", async () => {
    const { user, sessionId } = await setupConnected();
    await openFromTree(user, "README.md");
    await openFromTree(user, "deploy.sh");
    const active = () => useStore.getState().workspaces[sessionId]?.activeBuffer;
    expect(active()).toBe(`${sessionId}:/srv/app/deploy.sh`);
    await user.keyboard("{Control>}{Tab}{/Control}");
    expect(active()).toBe(`${sessionId}:/srv/app/README.md`);
    await user.keyboard("{Control>}{Shift>}{Tab}{/Shift}{/Control}");
    expect(active()).toBe(`${sessionId}:/srv/app/deploy.sh`);
    await user.keyboard("{Control>}w{/Control}");
    expect(useStore.getState().workspaces[sessionId]?.editorTabs).toEqual([`${sessionId}:/srv/app/README.md`]);
  });

  it("changing the encoding or the line endings marks the buffer dirty and the next save sends them", async () => {
    const { user, backend } = await setupConnected();
    const write = vi.spyOn(backend, "sftpWriteText");
    await openFromTree(user, "README.md");
    const status = screen.getByRole("contentinfo");
    await user.click(within(status).getByRole("button", { name: "Line endings: LF" }));
    expect(within(status).getByRole("button", { name: "Line endings: CRLF" })).toBeTruthy();
    expect(within(screen.getByRole("tab", { name: /README\.md/ })).getByRole("img", { name: "Unsaved changes" })).toBeTruthy();

    await user.click(within(status).getByRole("button", { name: "Encoding: UTF-8" }));
    const picker = await screen.findByRole("dialog", { name: "Encoding of README.md" });
    await user.click(within(picker).getByRole("radio", { name: "WINDOWS-1252" }));
    await user.click(within(picker).getByRole("button", { name: "Save with encoding" }));
    expect(within(status).getByRole("button", { name: "Encoding: WINDOWS-1252" })).toBeTruthy();

    act(() => view().focus());
    await user.keyboard("{Control>}s{/Control}");
    await waitFor(() => expect(write).toHaveBeenCalledOnce());
    expect(write.mock.calls[0]?.[1]).toMatchObject({ eol: "crlf", encoding: "windows-1252" });
  });
});

describe("closing with unsaved changes", () => {
  it("asks before closing a dirty tab: Cancel keeps it, Save writes then closes, Don't save discards", async () => {
    const { user, backend, sessionId } = await setupConnected();
    const write = vi.spyOn(backend, "sftpWriteText");
    await openFromTree(user, "README.md");
    insert("x");
    const close = () => user.click(screen.getByRole("button", { name: "Close README.md" }));

    await close();
    let dialog = await screen.findByRole("alertdialog", { name: "Save changes to README.md?" });
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(bufferOf(sessionId, "/srv/app/README.md")).toBeTruthy();

    await close();
    dialog = await screen.findByRole("alertdialog", { name: "Save changes to README.md?" });
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(bufferOf(sessionId, "/srv/app/README.md")).toBeUndefined());
    expect(write).toHaveBeenCalledOnce();

    await openFromTree(user, "README.md");
    insert("y");
    await close();
    dialog = await screen.findByRole("alertdialog", { name: "Save changes to README.md?" });
    await user.click(within(dialog).getByRole("button", { name: "Don't save" }));
    expect(bufferOf(sessionId, "/srv/app/README.md")).toBeUndefined();
    expect(write).toHaveBeenCalledOnce();
  });

  it("a clean tab closes at once; closing a connection with unsaved tabs asks (Discard and close)", async () => {
    const { user, sessionId } = await setupConnected();
    await openFromTree(user, "README.md");
    await user.click(screen.getByRole("button", { name: "Close README.md" }));
    expect(screen.queryByRole("alertdialog")).toBeNull();

    await openFromTree(user, "deploy.sh");
    insert("z");
    await user.click(screen.getByRole("button", { name: "Close connection prod-web-01" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Unsaved changes" });
    expect(dialog.textContent).toContain("deploy.sh has unsaved changes");
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(useStore.getState().sessionsById[sessionId]).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "Close connection prod-web-01" }));
    await user.click(await screen.findByRole("button", { name: "Discard and close" }));
    await waitFor(() => expect(useStore.getState().sessionsById[sessionId]).toBeUndefined());
  });
});

describe("save conflicts (saveConflict)", () => {
  it("remoteChanged opens the conflict dialog with a compare; Cancel keeps the edits and flags the tab", async () => {
    const { user, backend, sessionId } = await setupConnected({ scenario: "saveConflict" });
    const write = vi.spyOn(backend, "sftpWriteText");
    await openFromTree(user, "README.md");
    insert("mine\n");
    act(() => view().focus());
    await user.keyboard("{Control>}s{/Control}");

    const dialog = await screen.findByRole("alertdialog", { name: "README.md changed on the server" });
    await within(dialog).findByText(/lines? only on the server/);
    expect(dialog.textContent).toContain("# changed on the server");
    expect(dialog.textContent).toContain("mine");
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(docText().startsWith("mine\n")).toBe(true);
    expect(bufferOf(sessionId, "/srv/app/README.md")?.conflict).not.toBeNull();
    expect(within(screen.getByRole("tab", { name: /README\.md/ })).getByRole("img", { name: "Changed on the server" })).toBeTruthy();
    expect(write).toHaveBeenCalledOnce();

    // saving again re-opens the dialog (the server still differs)... Overwrite sends force
    act(() => view().focus());
    await user.keyboard("{Control>}s{/Control}");
    const again = await screen.findByRole("alertdialog", { name: "README.md changed on the server" });
    await user.click(within(again).getByRole("button", { name: "Overwrite" }));
    await waitFor(() => expect(write).toHaveBeenCalledTimes(3));
    expect(write.mock.calls[2]?.[1]).toMatchObject({ force: true });
    await waitFor(() => expect(bufferOf(sessionId, "/srv/app/README.md")?.dirty).toBe(false));
    expect(bufferOf(sessionId, "/srv/app/README.md")?.conflict).toBeNull();
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("'Reload from server' replaces the buffer with the server version and drops the edits", async () => {
    const { user, sessionId } = await setupConnected({ scenario: "saveConflict" });
    await openFromTree(user, "README.md");
    insert("mine\n");
    act(() => view().focus());
    await user.keyboard("{Control>}s{/Control}");
    const dialog = await screen.findByRole("alertdialog", { name: "README.md changed on the server" });
    await user.click(within(dialog).getByRole("button", { name: "Reload from server" }));
    await waitFor(() => expect(docText()).toContain("# changed on the server"));
    expect(docText().startsWith("mine")).toBe(false);
    expect(bufferOf(sessionId, "/srv/app/README.md")).toMatchObject({ dirty: false, conflict: null });
  });

  it("a strict 'Save all' (quit) reports a conflict instead of opening the dialog", async () => {
    const { user, sessionId } = await setupConnected({ scenario: "saveConflict" });
    await openFromTree(user, "README.md");
    insert("mine\n");
    const { saveAllBuffers } = await import("./editorActions");
    await expect(saveAllBuffers()).rejects.toMatchObject({ error: { code: "remoteChanged" } });
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(bufferOf(sessionId, "/srv/app/README.md")?.conflict).not.toBeNull();
  });
});

describe("pending saves across a reconnect", () => {
  it("a save requested while the connection is down is queued and runs after the reconnect", async () => {
    const { user, backend, sessionId } = await setupConnected();
    const write = vi.spyOn(backend, "sftpWriteText");
    await openFromTree(user, "README.md");
    insert("offline edit\n");
    backend.loseConnection(sessionId);
    await screen.findByRole("group", { name: "Connection to prod-web-01 lost" });
    act(() => view().focus());
    await user.keyboard("{Control>}s{/Control}");
    await waitFor(() => expect(bufferOf(sessionId, "/srv/app/README.md")?.pendingSave).toBe(true));
    expect(write).not.toHaveBeenCalled();
    expect(screen.getByText("Will save after reconnecting")).toBeTruthy();
    expect(docText().startsWith("offline edit")).toBe(true); // buffers stay untouched during the loss

    backend.restoreConnection(sessionId);
    await waitFor(() => expect(write).toHaveBeenCalledOnce());
    await waitFor(() => expect(bufferOf(sessionId, "/srv/app/README.md")).toMatchObject({ pendingSave: false, dirty: false }));
    expect(screen.queryByText("Will save after reconnecting")).toBeNull();
  });

  it("a queued save whose file changed meanwhile ends in the conflict dialog, not in a silent overwrite", async () => {
    const { user, backend, sessionId } = await setupConnected();
    await openFromTree(user, "README.md");
    insert("offline edit\n");
    backend.loseConnection(sessionId);
    act(() => view().focus());
    await user.keyboard("{Control>}s{/Control}");
    await waitFor(() => expect(bufferOf(sessionId, "/srv/app/README.md")?.pendingSave).toBe(true));
    (backend as unknown as { fs: { externalEdit(p: string, t: string): void } }).fs.externalEdit("/srv/app/README.md", "someone else\n");
    backend.restoreConnection(sessionId);
    await screen.findByRole("alertdialog", { name: "README.md changed on the server" });
    expect(docText().startsWith("offline edit")).toBe(true);
  });
});

describe("files the editor cannot open as text", () => {
  it("binaryFile: explains it and offers Download", async () => {
    const { user, backend } = await setupConnected({ scenario: "binaryFile" });
    const download = vi.spyOn(backend, "transferDownload");
    await openFromTree(user, "data.bin", false);
    const note = await screen.findByText(/data\.bin looks like a binary file/);
    expect(note).toBeTruthy();
    expect(document.querySelector(".cm-editor")).toBeNull();
    expect(useStore.getState().notices).toHaveLength(0); // explained in place, no toast
    await user.click(screen.getByRole("button", { name: "Download…" }));
    await waitFor(() => expect(download).toHaveBeenCalledWith(expect.any(String), "/srv/app/data.bin", "Download file", expect.any(Function)));
  });

  it("largeFile: says how big it is and what the limit is, with Download and a way to the settings", async () => {
    const { user } = await setupConnected({ scenario: "largeFile" });
    await openFromTree(user, "huge.log", false);
    await screen.findByText("huge.log is 80 MiB, which is over the editing limit of 5 MiB. You can download it instead.");
    expect(screen.getByRole("button", { name: "Download…" })).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Open settings" }));
    await screen.findByRole("dialog", { name: "Settings" });
  });

  it("nonUtf8File: 'Reopen with encoding…' decodes the file with the chosen encoding", async () => {
    const { user, backend } = await setupConnected({ scenario: "nonUtf8File" });
    const read = vi.spyOn(backend, "sftpReadText");
    await openFromTree(user, "legacy-cp1251.txt", false);
    await screen.findByText(/legacy-cp1251\.txt is not valid utf-8 text/);
    await user.click(screen.getByRole("button", { name: "Reopen with encoding…" }));
    const picker = await screen.findByRole("dialog", { name: "Encoding of legacy-cp1251.txt" });
    expect(within(picker).queryByRole("button", { name: "Save with encoding" })).toBeNull(); // nothing to save yet
    await user.click(within(picker).getByRole("radio", { name: "WINDOWS-1251" }));
    await user.click(within(picker).getByRole("button", { name: "Reopen with encoding" }));
    await waitFor(() => expect(document.querySelector(".cm-editor")).toBeTruthy());
    expect(docText()).toContain("Привіт");
    expect(read).toHaveBeenLastCalledWith(expect.any(String), "/srv/app/legacy-cp1251.txt", "windows-1251");
    expect(within(screen.getByRole("contentinfo")).getByRole("button", { name: "Encoding: WINDOWS-1251" })).toBeTruthy();
  });

  it("reopening with an encoding that fails keeps the text that is already open", async () => {
    const { user, backend } = await setupConnected();
    await openFromTree(user, "README.md");
    insert("keep me ");
    vi.spyOn(backend, "sftpReadText").mockRejectedValueOnce({ code: "decodeFailed", params: { path: "/srv/app/README.md", encoding: "utf-16le" } });
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    await user.click(within(screen.getByRole("contentinfo")).getByRole("button", { name: "Encoding: UTF-8" }));
    const picker = await screen.findByRole("dialog", { name: "Encoding of README.md" });
    expect(within(picker).getByText("Reopening discards your unsaved changes.")).toBeTruthy();
    await user.click(within(picker).getByRole("radio", { name: "UTF-16LE" }));
    await user.click(within(picker).getByRole("button", { name: "Reopen with encoding" }));
    await waitFor(() => expect(useStore.getState().notices.length).toBe(1));
    expect(docText().startsWith("keep me")).toBe(true);
  });

  it("EOL: a CRLF file shows CRLF; a file with mixed endings raises a notice and the status flags it", async () => {
    const { user } = await setupConnected({ scenario: "nonUtf8File" });
    await openFromTree(user, "windows.ini");
    expect(within(screen.getByRole("contentinfo")).getByRole("button", { name: "Line endings: CRLF" })).toBeTruthy();
    await openFromTree(user, "mixed.txt");
    await screen.findByText(/mixed\.txt has mixed line endings/);
    expect(within(screen.getByRole("contentinfo")).getByRole("button", { name: "Line endings: LF" }).getAttribute("title")).toContain("mixed line endings");
  });

  it("a failed open can be retried from the tab", async () => {
    const { user, backend } = await setupConnected();
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    vi.spyOn(backend, "sftpReadText").mockRejectedValueOnce({ code: "permissionDenied", params: { path: "/srv/app/README.md" } });
    await user.click(row("README.md"));
    await screen.findByText("Could not open README.md.");
    await user.click(screen.getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(document.querySelector(".cm-editor")).toBeTruthy());
    fireEvent.keyDown(document.body, { key: "Escape" });
  });
});
