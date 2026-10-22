// Settings (ARCHITECTURE §8.10): every setting is in its section, saved, and acts where it should.
import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import { settleApp, setupApp, setupConnected } from "../../test/harness";
import { useStore } from "../../state/store";
import { requestDelete } from "../files/fileActions";
import { flattenTree } from "../files/flattenTree";
import { disposeAllTerminals } from "../terminal/terminalRegistry";
import { EditorView } from "@codemirror/view";
import { undo } from "@codemirror/commands";

afterEach(async () => {
  await settleApp(60);
  disposeAllTerminals();
  setBackend(null);
  vi.restoreAllMocks();
});

async function openSettings() {
  const ctx = setupApp();
  await ctx.user.click(screen.getByRole("button", { name: "Settings" }));
  const dialog = await screen.findByRole("dialog", { name: "Settings" });
  const tab = (name: string) => ctx.user.click(within(dialog).getByRole("tab", { name }));
  return { ...ctx, dialog, tab };
}

describe("settings dialog: new fields", () => {
  it("General, Files, Terminal, Editor, Transfers and Advanced fields are saved together", async () => {
    const { user, backend, dialog, tab } = await openSettings();
    const update = vi.spyOn(backend, "settingsUpdate");

    await tab("General");
    await user.click(within(dialog).getByRole("checkbox", { name: "Confirm before closing running terminals" }));

    await tab("Files");
    await user.click(within(dialog).getByRole("checkbox", { name: "Folders first" }));
    await user.click(within(dialog).getByRole("checkbox", { name: "Confirm before deleting" }));

    await tab("Terminal");
    await user.selectOptions(within(dialog).getByLabelText("Cursor style"), "Bar");
    await user.selectOptions(within(dialog).getByLabelText("Bell"), "Flash the terminal");
    await user.selectOptions(within(dialog).getByLabelText("Right-click"), "Paste the clipboard");
    const lineHeight = within(dialog).getByLabelText("Line height");
    await user.clear(lineHeight);
    await user.type(lineHeight, "1.5");
    expect((within(dialog).getByLabelText("Terminal preview") as HTMLElement).style.lineHeight).toBe("1.5");

    await tab("Editor");
    await user.click(within(dialog).getByRole("checkbox", { name: "Show line numbers" }));
    await user.click(within(dialog).getByRole("checkbox", { name: "Highlight the active line" }));
    await user.click(within(dialog).getByRole("checkbox", { name: "Indent with tabs" }));
    await user.click(within(dialog).getByRole("checkbox", { name: "Trim trailing whitespace on save" }));
    await user.click(within(dialog).getByRole("checkbox", { name: "Insert a final newline on save" }));

    await tab("Transfers");
    const parallel = within(dialog).getByLabelText("Parallel transfers per connection");
    await user.clear(parallel);
    await user.type(parallel, "5");
    await user.selectOptions(within(dialog).getByLabelText("When the file already exists"), "Skip");

    await tab("Advanced");
    await user.selectOptions(within(dialog).getByLabelText("Log level"), "Debug");

    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(update).toHaveBeenCalledOnce());
    const saved = update.mock.calls[0]?.[0];
    expect(saved?.general).toEqual({
      confirmCloseWithTerminals: false,
      confirmQuit: "withConnections",
      onStartup: "welcome",
      notifyOnDisconnect: false,
      autoUpdate: true,
    });
    expect(saved?.files).toMatchObject({ foldersFirst: false, confirmDelete: false });
    expect(saved?.terminal).toMatchObject({ cursorStyle: "bar", bell: "visual", rightClick: "paste", lineHeight: 1.5 });
    expect(saved?.editor).toMatchObject({
      lineNumbers: false,
      highlightActiveLine: false,
      indentWithTabs: true,
      trimTrailingWhitespace: true,
      insertFinalNewline: true,
    });
    expect(saved?.transfers).toMatchObject({ maxConcurrent: 5, onConflict: "skip" });
    expect(saved?.advanced).toEqual({ logLevel: "debug" });
    expect(useStore.getState().settings?.terminal.cursorStyle).toBe("bar");
  });

  it("refuses a line height outside 1.0 to 2.0 and a parallel-transfer count outside 1 to 64", async () => {
    const { user, backend, dialog, tab } = await openSettings();
    const update = vi.spyOn(backend, "settingsUpdate");
    await tab("Terminal");
    const lineHeight = within(dialog).getByLabelText("Line height");
    await user.clear(lineHeight);
    await user.type(lineHeight, "2.5");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(within(dialog).getByText("Enter a number from 1.0 to 2.0.")).toBeTruthy();
    await user.clear(lineHeight);
    await user.type(lineHeight, "1.0");

    await tab("Transfers");
    const parallel = within(dialog).getByLabelText("Parallel transfers per connection");
    await user.clear(parallel);
    await user.type(parallel, "65");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(update).not.toHaveBeenCalled();
    expect(parallel.getAttribute("aria-invalid")).toBe("true");
  });

  it("Editor has its own font picker with the shared validation and a live preview", async () => {
    const { user, backend, dialog, tab } = await openSettings();
    const update = vi.spyOn(backend, "settingsUpdate");
    await tab("Editor");
    const select = within(dialog).getByLabelText("Font") as HTMLSelectElement;
    expect([...select.options].map((o) => o.text)).toContain("JetBrains Mono (bundled)");
    await user.selectOptions(select, "Custom…");
    const name = within(dialog).getByLabelText("Font name");
    await user.type(name, "Fira Code");
    const preview = within(dialog).getByLabelText("Editor font preview") as HTMLElement;
    expect(preview.style.fontFamily).toContain("Fira Code");
    // The terminal preview is not affected.
    await tab("Terminal");
    expect((within(dialog).getByLabelText("Terminal preview") as HTMLElement).style.fontFamily).not.toContain("Fira Code");
    await tab("Editor");

    await user.clear(within(dialog).getByLabelText("Font name"));
    await user.type(within(dialog).getByLabelText("Font name"), "bad;name");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(update).not.toHaveBeenCalled();
    expect(within(dialog).getByText(/Use font names with letters/)).toBeTruthy();

    await user.clear(within(dialog).getByLabelText("Font name"));
    await user.type(within(dialog).getByLabelText("Font name"), "Fira Code");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(update).toHaveBeenCalledOnce());
    expect(update.mock.calls[0]?.[0].editor.fontFamily).toBe("Fira Code");
  });

  it("Advanced: Clear recent connections asks first; Open logs folder and Reset all settings live here", async () => {
    const { user, backend, dialog, tab } = await openSettings();
    const clear = vi.spyOn(backend, "recentsClear");
    const logs = vi.spyOn(backend, "appOpenLogsDir");
    expect(useStore.getState().recents.length).toBeGreaterThan(0);
    await tab("Advanced");
    expect(within(dialog).getByRole("button", { name: "Reset all settings…" })).toBeTruthy();
    await user.click(within(dialog).getByRole("button", { name: "Open logs folder" }));
    expect(logs).toHaveBeenCalledOnce();

    await user.click(within(dialog).getByRole("button", { name: "Clear recent connections…" }));
    const confirm = await screen.findByRole("alertdialog", { name: "Clear recent connections?" });
    await user.click(within(confirm).getByRole("button", { name: "Cancel" }));
    expect(clear).not.toHaveBeenCalled();

    await user.click(within(dialog).getByRole("button", { name: "Clear recent connections…" }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Clear list" }));
    await waitFor(() => expect(clear).toHaveBeenCalledOnce());
    await waitFor(() => expect(useStore.getState().recents).toEqual([]));
    expect(within(dialog).getByText("Recent connections cleared.")).toBeTruthy();
  });
});

describe("Files settings act on the tree", () => {
  it("foldersFirst off sorts folders and files together", async () => {
    const { sessionId } = await setupConnected();
    const tree = useStore.getState().workspaces[sessionId]?.tree;
    if (!tree) throw new Error("no tree");
    const names = (foldersFirst: boolean) =>
      flattenTree(tree, { showHidden: true, foldersFirst })
        .filter((r) => r.type === "entry" && !r.isRoot)
        .map((r) => (r.type === "entry" ? r.name : ""));
    const first = names(true);
    const mixed = names(false);
    expect(mixed).not.toEqual(first);
    expect([...mixed].sort((a, b) => a.localeCompare(b, "en", { numeric: true, sensitivity: "base" }))).toEqual(mixed);
  });

  it("confirmDelete off deletes at once; on opens the confirmation", async () => {
    const { backend, sessionId } = await setupConnected();
    const del = vi.spyOn(backend, "sftpDelete");
    requestDelete(sessionId, "/srv/app/deploy.sh", "file");
    expect(await screen.findByRole("alertdialog")).toBeTruthy();
    expect(del).not.toHaveBeenCalled();
    act(() => useStore.getState().closeDialog());

    const settings = useStore.getState().settings;
    if (!settings) throw new Error("no settings");
    act(() => useStore.getState().setSettings({ ...settings, files: { ...settings.files, confirmDelete: false } }));
    requestDelete(sessionId, "/srv/app/deploy.sh", "file");
    await waitFor(() => expect(del).toHaveBeenCalledWith(sessionId, "/srv/app/deploy.sh"));
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });
});

describe("Editor settings act on the editor", () => {
  const view = (): EditorView => {
    const el = document.querySelector<HTMLElement>(".cm-editor");
    const found = el ? EditorView.findFromDOM(el) : null;
    if (!found) throw new Error("no editor view");
    return found;
  };

  it("follows font family, size, line numbers and active line live", async () => {
    const { user } = await setupConnected();
    await user.click(screen.getByRole("treeitem", { name: /README\.md/ }));
    await waitFor(() => expect(document.querySelector(".cm-editor")).toBeTruthy());
    expect(document.querySelector(".cm-lineNumbers")).toBeTruthy();
    expect(document.querySelector(".cm-activeLine")).toBeTruthy();

    const settings = useStore.getState().settings;
    if (!settings) throw new Error("no settings");
    act(() =>
      useStore.getState().setSettings({
        ...settings,
        editor: { ...settings.editor, fontFamily: "Hack", fontSize: 18, lineNumbers: false, highlightActiveLine: false },
      }),
    );
    const host = document.querySelector<HTMLElement>(".code-editor");
    expect(host?.style.getPropertyValue("--editor-font-family")).toContain("Hack");
    expect(host?.style.getPropertyValue("--editor-font-size")).toBe("18px");
    await waitFor(() => expect(document.querySelector(".cm-lineNumbers")).toBeNull());
    expect(document.querySelector(".cm-activeLine")).toBeNull();

    // Ligatures are explicitly off by default and follow the setting live.
    expect(host?.style.getPropertyValue("--editor-font-ligatures")).toBe("none");
    const current = useStore.getState().settings;
    if (!current) throw new Error("no settings");
    act(() => useStore.getState().setSettings({ ...current, editor: { ...current.editor, fontLigatures: true } }));
    expect(host?.style.getPropertyValue("--editor-font-ligatures")).toBe("normal");
  });

  it("trim trailing whitespace and final newline are applied on save as one undoable change, dirty tracking intact", async () => {
    const { user, backend, sessionId } = await setupConnected();
    await user.click(screen.getByRole("treeitem", { name: /README\.md/ }));
    await waitFor(() => expect(document.querySelector(".cm-editor")).toBeTruthy());
    const settings = useStore.getState().settings;
    if (!settings) throw new Error("no settings");
    act(() =>
      useStore.getState().setSettings({
        ...settings,
        editor: { ...settings.editor, trimTrailingWhitespace: true, insertFinalNewline: true },
      }),
    );
    const id = `${sessionId}:/srv/app/README.md`;
    const before = useStore.getState().buffers[id]?.etag;
    act(() => view().dispatch({ changes: { from: 0, to: view().state.doc.length, insert: "title   \nbody \t\nlast  " }, userEvent: "input.type" }));
    const write = vi.spyOn(backend, "sftpWriteText");
    act(() => view().focus());
    await user.keyboard("{Control>}s{/Control}");
    await waitFor(() => expect(write).toHaveBeenCalledOnce());

    const request = write.mock.calls[0]?.[1];
    expect(request?.text).toBe("title\nbody\nlast\n");
    expect(request?.expected).toBe(before); // the conflict check uses the same etag as without transforms
    await waitFor(() => expect(useStore.getState().buffers[id]?.dirty).toBe(false));
    await waitFor(() => expect(view().state.doc.toString()).toBe("title\nbody\nlast\n")); // the editor shows what was saved
    expect(useStore.getState().buffers[id]?.etag).not.toBe(before);

    // Undo brings the whitespace back and the buffer is dirty again.
    act(() => {
      undo(view());
    });
    await waitFor(() => expect(useStore.getState().buffers[id]?.dirty).toBe(true));
    expect(view().state.doc.toString()).not.toBe("title\nbody\nlast\n");
  });
});
