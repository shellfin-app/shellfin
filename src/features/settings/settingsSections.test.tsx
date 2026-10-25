// Settings (ARCHITECTURE §8.11): every setting is in its section with help text, saved, and validated.
import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import { useStore } from "../../state/store";
import { settleApp, setupApp } from "../../test/harness";

afterEach(async () => {
  await settleApp(60);
  setBackend(null);
  vi.restoreAllMocks();
});

async function openSettings(platform?: "macos") {
  const ctx = setupApp();
  if (platform) act(() => useStore.setState({ platform }));
  await ctx.user.click(screen.getByRole("button", { name: "Settings" }));
  const dialog = await screen.findByRole("dialog", { name: "Settings" });
  const tab = (name: string) => ctx.user.click(within(dialog).getByRole("tab", { name }));
  return { ...ctx, dialog, tab };
}

describe("settings: sections and saving", () => {
  it("General: quit confirmation, startup, disconnect notification", async () => {
    const { user, backend, dialog, tab } = await openSettings();
    const update = vi.spyOn(backend, "settingsUpdate");
    await tab("General");
    expect(within(dialog).getByLabelText("Confirm before quitting")).toHaveProperty("value", "withConnections");
    await user.selectOptions(within(dialog).getByLabelText("Confirm before quitting"), "Always");
    await user.selectOptions(within(dialog).getByLabelText("On startup"), "Reopen the previous connections");
    await user.click(within(dialog).getByRole("checkbox", { name: "Notify when a connection drops" }));
    // Help text is next to the control.
    expect(within(dialog).getByText(/Unsaved editor changes are always asked about/)).toBeTruthy();
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(update).toHaveBeenCalledOnce());
    expect(update.mock.calls[0]?.[0].general).toMatchObject({ confirmQuit: "always", onStartup: "restore", notifyOnDisconnect: true });
  });

  it("Terminal has a Screen reader support row (one setting per row) that saves terminal.screenReaderMode", async () => {
    const { user, backend, dialog, tab } = await openSettings();
    const update = vi.spyOn(backend, "settingsUpdate");
    await tab("Terminal");
    const row = within(dialog).getByRole("checkbox", { name: "Screen reader support" });
    expect(row).toHaveProperty("checked", false);
    await user.click(row);
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(update).toHaveBeenCalledOnce());
    expect(update.mock.calls[0]?.[0].terminal.screenReaderMode).toBe(true);
    expect(useStore.getState().settings?.terminal.screenReaderMode).toBe(true);
  });

  it("Edits survive a switch to another section and back, and two quick edits both land", async () => {
    const { user, backend, dialog, tab } = await openSettings();
    const update = vi.spyOn(backend, "settingsUpdate");
    await tab("Terminal");
    const spacing = within(dialog).getByLabelText("Letter spacing (px)");
    await user.clear(spacing);
    await user.type(spacing, "3");
    await tab("Editor");
    await tab("Terminal");
    expect(within(dialog).getByLabelText("Letter spacing (px)")).toHaveProperty("value", "3");
    // Two number edits in one act: the second must not overwrite the first with a stale form.
    const speed = within(dialog).getByLabelText("Scroll speed");
    await user.clear(speed);
    await user.type(speed, "7");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(update).toHaveBeenCalledOnce());
    expect(update.mock.calls[0]?.[0].terminal).toMatchObject({ letterSpacing: 3, scrollSensitivity: 7 });
  });

  it("Files: double-click, size units, dates, permissions, auto-refresh", async () => {
    const { user, backend, dialog, tab } = await openSettings();
    const update = vi.spyOn(backend, "settingsUpdate");
    await tab("Files");
    await user.selectOptions(within(dialog).getByLabelText("Double-click a file"), "Download it");
    await user.selectOptions(within(dialog).getByLabelText("File sizes"), "Decimal (KB, MB)");
    await user.selectOptions(within(dialog).getByLabelText("Dates"), "Absolute (date and time)");
    await user.selectOptions(within(dialog).getByLabelText("Refresh expanded folders"), "Every 30 s");
    await user.click(within(dialog).getByRole("checkbox", { name: "Show permissions, owner and group" }));
    await user.click(within(dialog).getByRole("checkbox", { name: "Show size and date columns" }));
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(update).toHaveBeenCalledOnce());
    expect(update.mock.calls[0]?.[0].files).toMatchObject({
      doubleClick: "download",
      sizeUnits: "decimal",
      dateFormat: "absolute",
      autoRefresh: 30,
      showPermissions: true,
      columns: false,
    });
    expect(useStore.getState().settings?.files.autoRefresh).toBe(30);
  });

  it("Terminal: look and behavior, with the sound bell", async () => {
    const { user, backend, dialog, tab } = await openSettings();
    const update = vi.spyOn(backend, "settingsUpdate");
    await tab("Terminal");
    const set = async (label: string, value: string) => {
      const input = within(dialog).getByLabelText(label);
      await user.clear(input);
      await user.type(input, value);
    };
    await set("Letter spacing (px)", "2");
    await set("Scroll speed", "5");
    await set("Minimum contrast ratio", "4.5");
    await set("Word separators", " /");
    await user.selectOptions(within(dialog).getByLabelText("Bell"), "Play a beep");
    await user.click(within(dialog).getByRole("checkbox", { name: "Let programs use the mouse" }));
    await user.click(within(dialog).getByRole("checkbox", { name: "Hide the mouse pointer while typing" }));
    await user.click(within(dialog).getByRole("checkbox", { name: "Warn before pasting several lines" }));
    await user.click(within(dialog).getByRole("checkbox", { name: "Open a terminal when connecting" }));
    await user.selectOptions(within(dialog).getByLabelText("When the shell ends"), "Close it if the shell ended normally");
    await user.selectOptions(within(dialog).getByLabelText("New terminal opens in"), "The folder open in the file tree");
    await user.selectOptions(within(dialog).getByLabelText("Terminal type (TERM)"), "vt100");
    // The mouse help names the Shift+drag escape hatch.
    expect(within(dialog).getByText(/hold Shift while dragging to select/)).toBeTruthy();
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(update).toHaveBeenCalledOnce());
    expect(update.mock.calls[0]?.[0].terminal).toMatchObject({
      letterSpacing: 2,
      scrollSensitivity: 5,
      minimumContrastRatio: 4.5,
      wordSeparators: " /",
      bell: "sound",
      mouseReporting: false,
      hideMousePointerWhileTyping: true,
      multilinePasteWarning: false,
      openTerminalOnConnect: false,
      onExit: "closeOnSuccess",
      newTerminalCwd: "currentFolder",
      termType: "vt100",
    });
  });

  it("Terminal: the Option-as-Meta switch exists on macOS only", async () => {
    const win = await openSettings();
    await win.tab("Terminal");
    expect(within(win.dialog).queryByRole("checkbox", { name: "Use Option as Meta" })).toBeNull();
    win.unmount();
    setBackend(null);

    const mac = await openSettings("macos");
    await mac.tab("Terminal");
    await mac.user.click(within(mac.dialog).getByRole("checkbox", { name: "Use Option as Meta" }));
    const update = vi.spyOn(mac.backend, "settingsUpdate");
    await mac.user.click(within(mac.dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(update).toHaveBeenCalledOnce());
    expect(update.mock.calls[0]?.[0].terminal.altIsMeta).toBe(true);
  });

  it("Terminal: scrollback accepts 10 to 1 000 000 and shows a memory hint above 100 000", async () => {
    const { user, backend, dialog, tab } = await openSettings();
    const update = vi.spyOn(backend, "settingsUpdate");
    await tab("Terminal");
    const scrollback = within(dialog).getByLabelText("Scrollback lines");
    expect(within(dialog).queryByText(/of memory per terminal/)).toBeNull();
    await user.clear(scrollback);
    await user.type(scrollback, "500000");
    expect(within(dialog).getByText(/can use about .* of memory per terminal/)).toBeTruthy();
    await user.clear(scrollback);
    await user.type(scrollback, "5");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(update).not.toHaveBeenCalled();
    expect(scrollback.getAttribute("aria-invalid")).toBe("true");
    await user.clear(scrollback);
    await user.type(scrollback, "1000000");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(update).toHaveBeenCalledOnce());
    expect(update.mock.calls[0]?.[0].terminal.scrollback).toBe(1_000_000);
  });

  it("Terminal: rejects a contrast ratio, spacing or separators outside the rules", async () => {
    const { user, backend, dialog, tab } = await openSettings();
    const update = vi.spyOn(backend, "settingsUpdate");
    await tab("Terminal");
    const contrast = within(dialog).getByLabelText("Minimum contrast ratio");
    await user.clear(contrast);
    await user.type(contrast, "30");
    const spacing = within(dialog).getByLabelText("Letter spacing (px)");
    await user.clear(spacing);
    await user.type(spacing, "9");
    const separators = within(dialog).getByLabelText("Word separators");
    await user.clear(separators);
    await user.type(separators, "é");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(update).not.toHaveBeenCalled();
    expect(within(dialog).getByText("Enter a number from 1 to 21.")).toBeTruthy();
    expect(spacing.getAttribute("aria-invalid")).toBe("true");
    expect(within(dialog).getByText(/Use at most 64 printable ASCII characters/)).toBeTruthy();
  });

  it("Save with an invalid field in another section shows that section", async () => {
    const { user, backend, dialog, tab } = await openSettings();
    const update = vi.spyOn(backend, "settingsUpdate");
    await tab("Terminal");
    const spacing = within(dialog).getByLabelText("Letter spacing (px)");
    await user.clear(spacing);
    await user.type(spacing, "9");
    await tab("General");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(update).not.toHaveBeenCalled();
    expect(within(dialog).getByRole("tab", { name: "Terminal" }).getAttribute("aria-selected")).toBe("true");
    expect(within(dialog).getByLabelText("Letter spacing (px)").getAttribute("aria-invalid")).toBe("true");
  });

  it("Editor: warns above 50 MiB, accepts up to 256 and saves the editing switches", async () => {
    const { user, backend, dialog, tab } = await openSettings();
    const update = vi.spyOn(backend, "settingsUpdate");
    await tab("Editor");
    const max = within(dialog).getByLabelText("Max file size (MiB)");
    expect(within(dialog).queryByText(/can make the editor slow/)).toBeNull();
    await user.clear(max);
    await user.type(max, "200");
    expect(within(dialog).getByText(/Opening files larger than 50 MiB can make the editor slow/)).toBeTruthy();
    await user.click(within(dialog).getByRole("checkbox", { name: "Detect indentation" }));
    await user.click(within(dialog).getByRole("checkbox", { name: "Close brackets and quotes automatically" }));
    await user.click(within(dialog).getByRole("checkbox", { name: "Highlight matching brackets" }));
    const whitespace = within(dialog).getByRole("combobox", { name: "Show whitespace" }) as HTMLSelectElement;
    expect(whitespace.value).toBe("none");
    expect([...whitespace.options].map((o) => [o.value, o.textContent])).toEqual([
      ["all", "All"],
      ["activeLine", "Active line only"],
      ["none", "None"],
    ]);
    await user.selectOptions(whitespace, "Active line only");
    await user.click(within(dialog).getByRole("checkbox", { name: "Code folding" }));
    expect((within(dialog).getByRole("checkbox", { name: "Font ligatures" }) as HTMLInputElement).checked).toBe(false);
    expect((within(dialog).getByLabelText("Editor font preview") as HTMLElement).style.fontVariantLigatures).toBe("none");
    await user.click(within(dialog).getByRole("checkbox", { name: "Font ligatures" }));
    expect((within(dialog).getByLabelText("Editor font preview") as HTMLElement).style.fontVariantLigatures).toBe("normal");
    await user.selectOptions(within(dialog).getByLabelText("Line endings of new files"), "CRLF (Windows)");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(update).toHaveBeenCalledOnce());
    expect(update.mock.calls[0]?.[0].editor).toMatchObject({
      maxFileSizeMiB: 200,
      detectIndentation: false,
      autoCloseBrackets: true,
      bracketMatching: false,
      showWhitespace: "activeLine",
      codeFolding: false,
      fontLigatures: true,
      newFileEol: "crlf",
    });
    expect(update.mock.calls[0]?.[0].terminal).not.toHaveProperty("fontLigatures");
  });

  it("Editor: auto-close brackets is off by default", async () => {
    const { dialog, tab } = await openSettings();
    await tab("Editor");
    expect((within(dialog).getByRole("checkbox", { name: "Close brackets and quotes automatically" }) as HTMLInputElement).checked).toBe(false);
  });

  it("Transfers and Connection: new switches and the default user", async () => {
    const { user, backend, dialog, tab } = await openSettings();
    const update = vi.spyOn(backend, "settingsUpdate");
    await tab("Transfers");
    await user.click(within(dialog).getByRole("checkbox", { name: "Keep file modification times" }));
    await user.click(within(dialog).getByRole("checkbox", { name: "Notify when transfers finish" }));
    await tab("Connection");
    await user.type(within(dialog).getByLabelText("Default user"), "deploy");
    await user.click(within(dialog).getByRole("checkbox", { name: "Read ~/.ssh/config" }));
    await user.click(within(dialog).getByRole("checkbox", { name: "Offer to save passwords" }));
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(update).toHaveBeenCalledOnce());
    const saved = update.mock.calls[0]?.[0];
    expect(saved?.transfers).toMatchObject({ preserveTimestamps: false, notifyOnComplete: true });
    expect(saved?.connection).toMatchObject({ defaultUser: "deploy", readSshConfig: false, rememberPasswords: false });
  });

  it("Connection: a default user with a space is refused", async () => {
    const { user, backend, dialog, tab } = await openSettings();
    const update = vi.spyOn(backend, "settingsUpdate");
    await tab("Connection");
    await user.type(within(dialog).getByLabelText("Default user"), "a b");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(update).not.toHaveBeenCalled();
    expect(within(dialog).getByText(/Use 1 to 255 characters without spaces/)).toBeTruthy();
  });

  it("Reset puts the newer fields back to the defaults", async () => {
    const { user, dialog, tab } = await openSettings();
    await tab("General");
    await user.selectOptions(within(dialog).getByLabelText("Confirm before quitting"), "Never");
    await tab("Advanced");
    await user.click(within(dialog).getByRole("button", { name: "Reset all settings…" }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Reset everything" }));
    await tab("General");
    await waitFor(() => expect(within(dialog).getByLabelText("Confirm before quitting")).toHaveProperty("value", "withConnections"));
  });
});
