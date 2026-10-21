// Settings dialog and bookmark groups: group delete, one-section settings, reset to defaults, terminal font.
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import { makeSettings } from "../../ipc/mock/fixtures";
import { MockBackend } from "../../ipc/mock/mockBackend";
import { detectInstalledFonts, terminalFontStack, validateFontFamily } from "../../lib/fontFamily";
import { setupApp } from "../../test/harness";
import { resetStore, useStore } from "../../state/store";
import { saveLayoutPrefs } from "../../state/layoutPrefs";
import { refreshBookmarks } from "../hosts/hostActions";
import { useMenuStore } from "../menu/menuStore";
import { createTerminal, disposeAllTerminals, getTerminal } from "../terminal/terminalRegistry";

afterEach(async () => {
  await new Promise((r) => setTimeout(r, 100));
  useMenuStore.getState().close();
  disposeAllTerminals();
  setBackend(null);
  vi.restoreAllMocks();
  window.localStorage.clear();
});

async function groups(backend: MockBackend) {
  const work = await backend.groupSave({ id: null, name: "Work", order: 0 });
  const empty = await backend.groupSave({ id: null, name: "Empty", order: 1 });
  const mk = (name: string, groupId: string | null) =>
    backend.bookmarkSave({ id: null, name, host: `${name}.example.com`, port: null, user: null, auth: { method: "auto" }, proxyJump: null, groupId, tags: [], startDir: null, useSshConfig: true });
  const w1 = await mk("w1", work.id);
  const w2 = await mk("w2", work.id);
  await refreshBookmarks();
  return { work, empty, w1, w2 };
}

const header = (id: string) => document.querySelector<HTMLElement>(`[data-group-id="${id}"]`) as HTMLElement;

describe("1. delete group", () => {
  it("an empty group is deleted from its context menu after a plain confirmation", async () => {
    const { backend, user } = setupApp();
    const { empty } = await groups(backend);
    const spy = vi.spyOn(backend, "groupDelete");
    await waitFor(() => expect(header(empty.id)).toBeTruthy());
    act(() => {
      header(empty.id).dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 10, clientY: 10 }));
    });
    await user.click(within(screen.getByRole("menu")).getByRole("menuitem", { name: /Delete group/ }));
    const dialog = await screen.findByRole("alertdialog", { name: "Delete group?" });
    expect(within(dialog).queryByRole("combobox")).toBeNull();
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(useStore.getState().bookmarks?.groups.map((g) => g.name)).toEqual(["Work"]));
    expect(spy).toHaveBeenCalledWith(empty.id);
  });

  it("Delete on a focused non-empty group asks where the bookmarks go; Ungrouped is the default and nothing is lost", async () => {
    const { backend, user } = setupApp();
    const { work, w1, w2 } = await groups(backend);
    await waitFor(() => expect(header(work.id)).toBeTruthy());
    const before = useStore.getState().bookmarks?.bookmarks.length;
    header(work.id).focus();
    await user.keyboard("{Delete}");
    const dialog = await screen.findByRole("alertdialog", { name: "Delete group?" });
    const select = within(dialog).getByLabelText("Move bookmarks to") as HTMLSelectElement;
    expect(select.value).toBe("");
    await user.click(within(dialog).getByRole("button", { name: "Move and delete group" }));
    await waitFor(() => expect(useStore.getState().bookmarks?.groups.some((g) => g.id === work.id)).toBe(false));
    const bms = useStore.getState().bookmarks?.bookmarks ?? [];
    expect(bms.length).toBe(before);
    expect(bms.find((b) => b.id === w1.id)?.groupId).toBeNull();
    expect(bms.find((b) => b.id === w2.id)?.groupId).toBeNull();
  });

  it("can move the bookmarks into another group; Cancel changes nothing", async () => {
    const { backend, user } = setupApp();
    const { work, empty, w1 } = await groups(backend);
    await waitFor(() => expect(header(work.id)).toBeTruthy());
    header(work.id).focus();
    await user.keyboard("{Delete}");
    let dialog = await screen.findByRole("alertdialog", { name: "Delete group?" });
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(useStore.getState().bookmarks?.groups.length).toBe(2);
    header(work.id).focus();
    await user.keyboard("{Delete}");
    dialog = await screen.findByRole("alertdialog", { name: "Delete group?" });
    await user.selectOptions(within(dialog).getByLabelText("Move bookmarks to"), empty.id);
    await user.click(within(dialog).getByRole("button", { name: "Move and delete group" }));
    await waitFor(() => expect(useStore.getState().bookmarks?.groups.map((g) => g.id)).toEqual([empty.id]));
    expect(useStore.getState().bookmarks?.bookmarks.find((b) => b.id === w1.id)?.groupId).toBe(empty.id);
  });
});

async function openSettings() {
  const ctx = setupApp();
  await ctx.user.click(screen.getByRole("button", { name: "Settings" }));
  const dialog = await screen.findByRole("dialog", { name: "Settings" });
  return { ...ctx, dialog };
}

describe("2. one settings section at a time", () => {
  it("shows only the selected section, moves with the arrow keys and remembers the last one", async () => {
    const { user, dialog } = await openSettings();
    await user.click(within(dialog).getByRole("tab", { name: "Terminal" }));
    expect(within(dialog).getByLabelText("Scrollback lines")).toBeTruthy();
    expect(within(dialog).queryByLabelText("Language")).toBeNull();
    expect(within(dialog).queryByLabelText("Upload speed limit")).toBeNull();
    expect(within(dialog).getByRole("tab", { name: "Terminal" }).getAttribute("aria-selected")).toBe("true");

    await user.keyboard("{ArrowDown}");
    expect(within(dialog).getByRole("tab", { name: "Editor" }).getAttribute("aria-selected")).toBe("true");
    expect(within(dialog).getByLabelText("Tab size")).toBeTruthy();
    expect(within(dialog).queryByLabelText("Scrollback lines")).toBeNull();
    await user.keyboard("{ArrowUp}{ArrowUp}");
    expect(within(dialog).getByRole("tab", { name: "Files" }).getAttribute("aria-selected")).toBe("true");

    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Settings" })).toBeNull());
    await user.click(screen.getByRole("button", { name: "Settings" }));
    const again = await screen.findByRole("dialog", { name: "Settings" });
    expect(within(again).getByRole("checkbox", { name: "Show hidden files" })).toBeTruthy();
    await user.click(within(again).getByRole("tab", { name: "General" })); // reset for other tests
  });

  it("keeps the font preview in the terminal section", async () => {
    const { user, dialog } = await openSettings();
    await user.click(within(dialog).getByRole("tab", { name: "Terminal" }));
    const preview = within(dialog).getByLabelText("Terminal preview");
    expect(preview.textContent).toContain("deploy@web-01:~$ ls -la");
    expect(within(dialog).queryByLabelText("Editor font preview")).toBeNull();
    await user.click(within(dialog).getByRole("tab", { name: "Editor" }));
    expect(within(dialog).getByLabelText("Editor font preview").textContent).toBe(
      "The quick brown fox jumps over the lazy dog — 0O 1lI {}[] => != ;:",
    );
    expect(within(dialog).queryByLabelText("Terminal preview")).toBeNull();
  });

  it("keeps a fixed size and the section list while switching sections", async () => {
    const { user, dialog } = await openSettings();
    const box = () => `${dialog.style.width} / ${dialog.style.height}`;
    const first = box();
    expect(first).toBe("860px / 600px");
    const nav = within(dialog).getByRole("tablist");
    for (const name of ["Terminal", "Editor", "Transfers", "General"]) {
      await user.click(within(dialog).getByRole("tab", { name }));
      expect(box()).toBe(first);
      expect(within(dialog).getByRole("tablist")).toBe(nav);
    }
  });
});

describe("3. reset to defaults", () => {
  it("asks first (Cancel is focused), then resets settings and the local layout everywhere", async () => {
    const { user, backend, dialog } = await openSettings();
    const reset = vi.spyOn(backend, "settingsReset");
    useStore.getState().setSettings({ ...makeSettings(), terminal: { ...makeSettings().terminal, fontSize: 20, fontFamily: "Hack" } });
    saveLayoutPrefs({ sidebarWidth: 400 });
    act(() => useStore.getState().setStatusBarOpen(false));

    await user.click(within(dialog).getByRole("tab", { name: "Advanced" })); // Reset lives in Advanced
    await user.click(within(dialog).getByRole("button", { name: "Reset all settings…" }));
    const confirm =await screen.findByRole("alertdialog", { name: "Reset all settings to defaults?" });
    expect(confirm.textContent).toContain("layout");
    expect(document.activeElement).toBe(within(confirm).getByRole("button", { name: "Cancel" }));
    await user.click(within(confirm).getByRole("button", { name: "Cancel" }));
    expect(reset).not.toHaveBeenCalled();
    expect(window.localStorage.getItem("shellfin.layout.v1")).not.toBeNull();

    await user.click(within(dialog).getByRole("button", { name: "Reset all settings…" }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Reset everything" }));
    await waitFor(() => expect(reset).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(useStore.getState().settings?.terminal.fontSize).toBe(13));
    expect(useStore.getState().settings?.terminal.fontFamily).toBeNull();
    expect(useStore.getState().statusBarOpen).toBe(true);
    expect(JSON.parse(window.localStorage.getItem("shellfin.layout.v1") ?? "{}").sidebarWidth).not.toBe(400);
  });
});

describe("4. terminal font", () => {
  it("validates like the backend rule and always appends the bundled font and monospace", () => {
    expect(validateFontFamily("Cascadia Code")).toBeNull();
    expect(validateFontFamily('"Fira Code", monospace')).toBeNull();
    expect(validateFontFamily("")).toBe("empty");
    expect(validateFontFamily(" , ")).toBe("empty");
    expect(validateFontFamily("a".repeat(201))).toBe("tooLong");
    for (const bad of ["url(x)", "a;b", "a{b}", "a\\b", "a@b", "a(b)", "a\nb"]) expect(validateFontFamily(bad)).toBe("badChars");
    // ASCII only, exactly like the backend: a non-ASCII name must be rejected here, not by settings_update.
    expect(validateFontFamily("Fira Código")).toBe("badChars");
    expect(terminalFontStack(null)).toBe('"JetBrains Mono", ui-monospace, monospace');
    expect(terminalFontStack("Cascadia Code")).toBe('"Cascadia Code", "JetBrains Mono", ui-monospace, monospace');
    expect(terminalFontStack('"Fira Code", monospace')).toBe('"Fira Code", monospace, "JetBrains Mono", ui-monospace, monospace');
    expect(terminalFontStack("url(x)")).toBe(terminalFontStack(null));
  });

  it("lists only fonts reported as installed", () => {
    const check = vi.fn((spec: string) => spec.includes("Hack") || spec.includes("Consolas"));
    Object.defineProperty(document, "fonts", { configurable: true, value: { check } });
    expect(detectInstalledFonts()).toEqual(["Consolas", "Hack"]);
    Object.defineProperty(document, "fonts", { configurable: true, value: undefined });
  });

  it("picks an installed font or a custom one, validates it and saves it", async () => {
    Object.defineProperty(document, "fonts", { configurable: true, value: { check: (s: string) => s.includes("Hack"), load: async () => [] } });
    try {
      const { user, backend, dialog } = await openSettings();
      const update = vi.spyOn(backend, "settingsUpdate");
      await user.click(within(dialog).getByRole("tab", { name: "Terminal" }));
      const select = within(dialog).getByLabelText("Font") as HTMLSelectElement;
      expect([...select.options].map((o) => o.text)).toEqual(["JetBrains Mono (bundled)", "Hack", "Custom…"]);
      await user.selectOptions(select, "Hack");
      expect((within(dialog).getByLabelText("Terminal preview") as HTMLElement).style.fontFamily).toContain("Hack");

      await user.selectOptions(select, "Custom…");
      const name = within(dialog).getByLabelText("Font name");
      await user.clear(name);
      await user.type(name, "bad;name");
      await user.click(within(dialog).getByRole("button", { name: "Save" }));
      expect(update).not.toHaveBeenCalled();
      expect(name.getAttribute("aria-invalid")).toBe("true");
      await user.clear(name);
      await user.type(name, "My Mono");
      await user.click(within(dialog).getByRole("button", { name: "Save" }));
      await waitFor(() => expect(update).toHaveBeenCalled());
      expect(update.mock.calls[0]?.[0].terminal.fontFamily).toBe("My Mono");
    } finally {
      Object.defineProperty(document, "fonts", { configurable: true, value: undefined });
    }
  });

  it("applies a changed font to open terminals at once", async () => {
    resetStore();
    const backend = new MockBackend();
    setBackend(backend);
    const sessionId = await backend.openSession({ target: { kind: "sshConfig", alias: "prod-web-01" }, probeOnly: false }, () => undefined);
    useStore.getState().addSession(sessionId, { kind: "sshConfig", alias: "prod-web-01" }, "prod-web-01");
    useStore.getState().ensureWorkspace(sessionId);
    useStore.getState().setSettings(makeSettings());
    const termId = await createTerminal(sessionId);
    const entry = getTerminal(termId ?? "");
    expect(entry?.term.options.fontFamily).toBe(terminalFontStack(null));
    act(() => useStore.getState().setSettings({ ...makeSettings(), terminal: { ...makeSettings().terminal, fontFamily: "Hack" } }));
    expect(entry?.term.options.fontFamily).toBe('"Hack", "JetBrains Mono", ui-monospace, monospace');
    fireEvent.keyDown(document, { key: "Escape" });
  });
});
