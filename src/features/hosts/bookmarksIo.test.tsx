// Bookmark import / export (ARCHITECTURE §7.7, §8.10): actions, options dialogs, native dialog calls, summaries.
import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import { ACTION_IDS, MENUS } from "../../lib/actionIds";
import { settleApp, setupApp } from "../../test/harness";
import { useStore } from "../../state/store";
import { actionEnabled, runAction } from "../actions/registry";
import { useMenuStore } from "../menu/menuStore";

afterEach(async () => {
  await settleApp(60);
  useMenuStore.getState().close();
  setBackend(null);
  vi.restoreAllMocks();
});

describe("actions", () => {
  it("are in the registry and in the File menu", () => {
    expect(ACTION_IDS).toContain("importBookmarks");
    expect(ACTION_IDS).toContain("exportBookmarks");
    expect(MENUS.file).toContain("importBookmarks");
    expect(MENUS.file).toContain("exportBookmarks");
  });

  it("File menu and the Bookmarks panel header menu both offer them", async () => {
    const { user } = setupApp();
    const bar = screen.getByRole("menubar", { name: "Application menu" });
    await user.click(within(bar).getByRole("menuitem", { name: "File" }));
    const fileMenu = await screen.findByRole("menu", { name: "File" });
    expect(within(fileMenu).getByRole("menuitem", { name: /Import bookmarks…/ })).toBeTruthy();
    expect(within(fileMenu).getByRole("menuitem", { name: /Export bookmarks…/ })).toBeTruthy();
    await user.keyboard("{Escape}");

    await user.click(screen.getByRole("button", { name: "More bookmark actions" }));
    await user.click(await screen.findByRole("menuitem", { name: "Export bookmarks…" }));
    expect(await screen.findByRole("dialog", { name: "Export bookmarks" })).toBeTruthy();
  });

  it("import is disabled while the bookmark store is read-only", () => {
    setupApp();
    const dto = useStore.getState().bookmarks;
    if (!dto) throw new Error("no bookmarks");
    act(() => useStore.getState().setBookmarks({ ...dto, readOnly: true }));
    expect(actionEnabled("importBookmarks")).toBe(false);
    expect(actionEnabled("exportBookmarks")).toBe(true);
  });
});

describe("import", () => {
  it("asks for the conflict strategy first, then opens the native dialog and summarises", async () => {
    const { user, backend } = setupApp();
    const existing = useStore.getState().bookmarks?.bookmarks[0];
    if (!existing) throw new Error("no bookmark");
    backend.importFile = {
      bookmarks: [
        { name: existing.name, host: "other.example.com" },
        { name: "brand-new", host: "new.example.com" },
      ],
      invalid: [
        { entry: "bookmark", index: 4, field: "host", code: "missing" },
        { entry: "group", index: 0, field: "name", code: "invalid" },
      ],
    };
    act(() => runAction("importBookmarks"));
    const dialog = await screen.findByRole("dialog", { name: "Import bookmarks" });
    expect(backend.importCalls).toHaveLength(0); // nothing opens before the strategy is chosen

    expect(within(dialog).getByText(/The existing bookmark is kept/)).toBeTruthy();
    await user.selectOptions(within(dialog).getByLabelText("If a bookmark with the same name exists"), "Keep both");
    expect(within(dialog).getByText(/“prod \(2\)”/)).toBeTruthy();
    expect(within(dialog).getByText(/Passwords are never imported/)).toBeTruthy();
    await user.click(within(dialog).getByRole("button", { name: "Choose file…" }));

    expect(backend.importCalls).toEqual([{ strategy: "duplicate", dialogTitle: "Import bookmarks" }]);
    const summary = await screen.findByRole("dialog", { name: "Import finished" });
    expect(summary.textContent).toContain("2 bookmarks added");
    expect(summary.textContent).toContain("0 bookmarks replaced");
    expect(summary.textContent).toContain("0 bookmarks skipped");
    expect(summary.textContent).toContain("2 entries rejected");
    expect(summary.textContent).toContain("Bookmark 5: host is missing");
    expect(summary.textContent).toContain("Group 1: name has an invalid value");
    // The store follows the result without a reload.
    const names = useStore.getState().bookmarks?.bookmarks.map((b) => b.name) ?? [];
    expect(names).toContain("brand-new");
    expect(names).toContain(`${existing.name} (2)`);
  });

  it("skip leaves the existing bookmark and counts it; replace overwrites it", async () => {
    const { user, backend } = setupApp();
    const existing = useStore.getState().bookmarks?.bookmarks[0];
    if (!existing) throw new Error("no bookmark");
    backend.importFile = { bookmarks: [{ name: existing.name, host: "changed.example.com" }] };

    act(() => runAction("importBookmarks"));
    let dialog = await screen.findByRole("dialog", { name: "Import bookmarks" });
    await user.click(within(dialog).getByRole("button", { name: "Choose file…" }));
    let summary = await screen.findByRole("dialog", { name: "Import finished" });
    expect(summary.textContent).toContain("1 bookmark skipped");
    expect(useStore.getState().bookmarks?.bookmarks.find((b) => b.id === existing.id)?.host).toBe(existing.host);
    await user.click(within(summary).getByRole("button", { name: "Close" }));

    act(() => runAction("importBookmarks"));
    dialog = await screen.findByRole("dialog", { name: "Import bookmarks" });
    await user.selectOptions(within(dialog).getByLabelText("If a bookmark with the same name exists"), "Replace the existing one");
    await user.click(within(dialog).getByRole("button", { name: "Choose file…" }));
    summary = await screen.findByRole("dialog", { name: "Import finished" });
    expect(summary.textContent).toContain("1 bookmark replaced");
    expect(useStore.getState().bookmarks?.bookmarks.find((b) => b.id === existing.id)?.host).toBe("changed.example.com");
  });

  it("a cancelled native dialog shows no summary and leaves the options open", async () => {
    const { user, backend } = setupApp();
    backend.importFile = null;
    act(() => runAction("importBookmarks"));
    const dialog = await screen.findByRole("dialog", { name: "Import bookmarks" });
    await user.click(within(dialog).getByRole("button", { name: "Choose file…" }));
    await waitFor(() => expect(backend.importCalls).toHaveLength(1));
    expect(screen.queryByRole("dialog", { name: "Import finished" })).toBeNull();
    expect(screen.getByRole("dialog", { name: "Import bookmarks" })).toBeTruthy();
  });

  it("a refused file shows the reason in the dialog and imports nothing", async () => {
    const { user, backend } = setupApp();
    const before = useStore.getState().bookmarks?.bookmarks.length;
    backend.importFile = { code: "importInvalidFile", params: { reason: "notJson" } };
    act(() => runAction("importBookmarks"));
    const dialog = await screen.findByRole("dialog", { name: "Import bookmarks" });
    await user.click(within(dialog).getByRole("button", { name: "Choose file…" }));
    await within(dialog).findByText("This file cannot be imported (it is not a JSON file).");
    expect(useStore.getState().bookmarks?.bookmarks.length).toBe(before);
  });
});

describe("export", () => {
  it("explains that passwords are never exported, passes the format and summarises", async () => {
    const { user, backend } = setupApp();
    act(() => runAction("exportBookmarks"));
    const dialog = await screen.findByRole("dialog", { name: "Export bookmarks" });
    expect(within(dialog).getByText(/Passwords are never exported/)).toBeTruthy();
    await user.selectOptions(within(dialog).getByLabelText("Format"), "OpenSSH config");
    expect(within(dialog).getByText(/Groups, tags, colors, start folders and speed limits cannot be stored/)).toBeTruthy();
    backend.exportOutcome = {
      fileName: "shellfin-ssh-config",
      count: 0,
      skipped: [
        { name: "prod", field: "color", code: "notRepresentable" },
        { name: "dev", field: "startDir", code: "unsafeValue" },
      ],
    };
    await user.click(within(dialog).getByRole("button", { name: "Export…" }));

    expect(backend.exportCalls).toEqual([{ format: "sshConfig", dialogTitle: "Export bookmarks" }]);
    const summary = await screen.findByRole("dialog", { name: "Bookmarks exported" });
    expect(summary.textContent).toMatch(/\d+ bookmarks? w(as|ere) saved to shellfin-ssh-config/);
    expect(summary.textContent).toContain("Passwords are never exported");
    expect(summary.textContent).toContain("prod: color cannot be stored in this format");
    expect(summary.textContent).toContain("dev: startDir contains characters that are not allowed");
  });

  it("shows names with bidi or control characters escaped, never as markup", async () => {
    const { user, backend } = setupApp();
    act(() => runAction("exportBookmarks"));
    const dialog = await screen.findByRole("dialog", { name: "Export bookmarks" });
    backend.exportOutcome = {
      fileName: "a‮b.json",
      count: 1,
      skipped: [{ name: "<img src=x onerror=1>‮\u0007", field: "color", code: "notRepresentable" }],
    };
    await user.click(within(dialog).getByRole("button", { name: "Export…" }));
    const summary = await screen.findByRole("dialog", { name: "Bookmarks exported" });
    expect(summary.textContent).not.toContain("‮");
    expect(summary.textContent).not.toContain("\u0007");
    expect(summary.textContent).toContain("⟨U+202E⟩");
    expect(summary.querySelector("img")).toBeNull();
  });

  it("a cancelled save dialog shows no summary", async () => {
    const { user, backend } = setupApp();
    backend.exportOutcome = null;
    act(() => runAction("exportBookmarks"));
    const dialog = await screen.findByRole("dialog", { name: "Export bookmarks" });
    await user.click(within(dialog).getByRole("button", { name: "Export…" }));
    await waitFor(() => expect(backend.exportCalls).toHaveLength(1));
    expect(screen.queryByRole("dialog", { name: "Bookmarks exported" })).toBeNull();
  });
});
