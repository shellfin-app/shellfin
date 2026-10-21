// Updater (ARCHITECTURE §9.8): the update dialog, the "Check for updates…" action, the quit-flow hand-off and
// the settings rows, against the MockBackend (which models the backend contract of src-tauri/src/updater.rs).
import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import { makeMockUpdate } from "../../ipc/mock/fixtures";
import type { UpdateInfo } from "../../ipc/types";
import { MENUS } from "../../lib/actionIds";
import { useStore } from "../../state/store";
import { settleApp, setupApp } from "../../test/harness";
import { runAction } from "../actions/registry";
import { installQuitFlow } from "../session/quitFlow";
import { NATIVE_MENU } from "../shell/shellEvents";
import { NOTES_DISPLAY_LIMIT } from "./UpdateDialog";
import { installUpdateEvents } from "./updateActions";

afterEach(async () => {
  await settleApp(30);
  setBackend(null);
  vi.restoreAllMocks();
});

async function setupWithUpdate(update: UpdateInfo | null = makeMockUpdate()) {
  const ctx = setupApp();
  ctx.backend.publishedUpdate = update;
  await installQuitFlow();
  await installUpdateEvents();
  return ctx;
}

async function checkNow() {
  act(() => runAction("checkForUpdates"));
}

describe("Check for updates… (manual)", () => {
  it("is in the Help menu, the palette and the macOS native menu", async () => {
    expect(MENUS.help).toContain("checkForUpdates");
    expect(NATIVE_MENU["help.checkForUpdates"]).toBe("checkForUpdates");
    const { user } = setupApp();
    await user.click(within(screen.getByRole("menubar", { name: "Application menu" })).getByRole("menuitem", { name: "Help" }));
    expect(within(await screen.findByRole("menu", { name: "Help" })).getByRole("menuitem", { name: /Check for updates…/ })).toBeTruthy();
    await user.keyboard("{Escape}");
    act(() => useStore.getState().openPalette("all"));
    const palette = await screen.findByRole("dialog", { name: "Command palette" });
    await user.type(within(palette).getByRole("combobox"), "updates");
    expect(within(palette).getByText("Check for updates…")).toBeTruthy();
  });

  it("no update: says \"You're up to date\" and opens no dialog", async () => {
    await setupWithUpdate(null);
    await checkNow();
    expect(await screen.findByText("You're up to date.")).toBeTruthy();
    expect(screen.queryByRole("dialog", { name: /is available/ })).toBeNull();
  });

  it("an error goes through the normal error path (errors.<code>)", async () => {
    const { backend } = await setupWithUpdate(null);
    backend.updateCheckFailure = { code: "updatesDisabled" };
    await checkNow();
    expect(await screen.findByText("Updates are not available in this build.")).toBeTruthy();
  });

  it("an update: version, plain-text notes (escaped, bounded, scrollable) and the three choices", async () => {
    const hostile = {
      ...makeMockUpdate(),
      notes: `line one\n<img src=x onerror=alert(1)>\u001b[2J‮evil\n${"x".repeat(NOTES_DISPLAY_LIMIT + 50)}`,
      notesTruncated: true,
    };
    await setupWithUpdate(hostile);
    await checkNow();
    const dialog = await screen.findByRole("dialog", { name: "Shellfin 0.2.0 is available" });
    expect(within(dialog).getByText("Installed: 0.1.0")).toBeTruthy();
    const notes = dialog.querySelector(".update__notes");
    expect(notes?.textContent).toContain("line one\n<img src=x onerror=alert(1)>");
    expect(notes?.textContent).toContain("\\x1b[2J⟨U+202E⟩evil");
    expect(notes?.querySelector("img")).toBeNull();
    expect(Array.from(notes?.textContent ?? "").length).toBeLessThanOrEqual(NOTES_DISPLAY_LIMIT);
    expect(within(dialog).getByText(/The notes are shortened/)).toBeTruthy();
    for (const name of ["Update and restart", "Later", "Skip this version"]) {
      expect(within(dialog).getByRole("button", { name })).toBeTruthy();
    }
  });

  it("Later closes the dialog without telling the backend anything", async () => {
    const { user, backend } = await setupWithUpdate();
    const skip = vi.spyOn(backend, "updateSkipVersion");
    await checkNow();
    const dialog = await screen.findByRole("dialog", { name: /is available/ });
    await user.click(within(dialog).getByRole("button", { name: "Later" }));
    expect(screen.queryByRole("dialog", { name: /is available/ })).toBeNull();
    expect(skip).not.toHaveBeenCalled();
  });

  it("Skip this version stores it; the automatic announcement then stays silent, a manual check still shows it", async () => {
    const { user, backend } = await setupWithUpdate();
    await checkNow();
    await user.click(within(await screen.findByRole("dialog", { name: /is available/ })).getByRole("button", { name: "Skip this version" }));
    await waitFor(() => expect(backend.skippedUpdateVersion).toBe("0.2.0"));
    expect(screen.queryByRole("dialog", { name: /is available/ })).toBeNull();

    act(() => backend.announceUpdate());
    expect(screen.queryByRole("dialog", { name: /is available/ })).toBeNull();

    await checkNow();
    expect(await screen.findByRole("dialog", { name: "Shellfin 0.2.0 is available" })).toBeTruthy();
  });
});

describe("update:available (automatic check)", () => {
  it("opens the dialog when nothing else is open", async () => {
    const { backend } = await setupWithUpdate();
    act(() => backend.announceUpdate());
    expect(await screen.findByRole("dialog", { name: "Shellfin 0.2.0 is available" })).toBeTruthy();
  });

  it("never replaces another dialog: a notice points to Help instead", async () => {
    const { backend } = await setupWithUpdate();
    act(() => useStore.getState().openDialog({ kind: "about" }));
    act(() => backend.announceUpdate());
    expect(await screen.findByText(/Shellfin 0.2.0 is available. Choose Help → Check for updates…/)).toBeTruthy();
    expect(useStore.getState().dialog?.kind).toBe("about");
  });
});

describe("Update and restart", () => {
  it("downloads with progress, then goes through the quit flow and installs at its end", async () => {
    const { user, backend } = await setupWithUpdate();
    backend.holdDownload = true;
    const install = vi.spyOn(backend, "updateInstall");
    await checkNow();
    const dialog = await screen.findByRole("dialog", { name: /is available/ });
    await user.click(within(dialog).getByRole("button", { name: "Update and restart" }));
    expect(install).toHaveBeenCalledWith("0.2.0", expect.any(Function));
    expect(await within(dialog).findByText(/Downloading… 1(\.0)? MiB of 4(\.0)? MiB/)).toBeTruthy();
    expect(within(dialog).queryByRole("button", { name: "Update and restart" })).toBeNull();

    act(() => backend.releaseDownload());
    // No connections and `confirmQuit = withConnections`: the quit goes straight through.
    await waitFor(() => expect(backend.installedUpdate?.version).toBe("0.2.0"));
    expect(backend.quitCalls).toBe(1);
  });

  it("when quitting asks (unsaved changes, confirmQuit), the quit dialog comes first; Cancel keeps the app", async () => {
    const { user, backend } = await setupWithUpdate();
    act(() => useStore.setState((s) => ({ settings: s.settings ? { ...s.settings, general: { ...s.settings.general, confirmQuit: "always" } } : s.settings })));
    await checkNow();
    await user.click(within(await screen.findByRole("dialog", { name: /is available/ })).getByRole("button", { name: "Update and restart" }));
    const quit = await screen.findByRole("alertdialog");
    await user.click(within(quit).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(useStore.getState().update?.phase).toBe("available"));
    expect(backend.quitCalls).toBe(0);
    expect(backend.installedUpdate).toBeNull();
    // The staged update was dropped: quitting normally later installs nothing.
    await act(async () => backend.appQuit({}));
    expect(backend.installedUpdate).toBeNull();
  });

  it("Cancel during the download returns to the offer", async () => {
    const { user, backend } = await setupWithUpdate();
    backend.holdDownload = true;
    await checkNow();
    const dialog = await screen.findByRole("dialog", { name: /is available/ });
    await user.click(within(dialog).getByRole("button", { name: "Update and restart" }));
    await user.click(await within(dialog).findByRole("button", { name: "Cancel" }));
    expect(await within(dialog).findByRole("button", { name: "Update and restart" })).toBeTruthy();
    expect(backend.quitCalls).toBe(0);
  });

  it("a failed signature check is shown in the dialog with Try again", async () => {
    const { user, backend } = await setupWithUpdate();
    backend.updateDownloadFailure = { code: "updateVerifyFailed" };
    await checkNow();
    const dialog = await screen.findByRole("dialog", { name: /is available/ });
    await user.click(within(dialog).getByRole("button", { name: "Update and restart" }));
    expect(await within(dialog).findByText("The downloaded update failed the signature check, so it was not installed.")).toBeTruthy();
    expect(within(dialog).getByRole("button", { name: "Try again" })).toBeTruthy();
    expect(backend.quitCalls).toBe(0);
  });

  it("update:install-failed reopens the dialog with the error; the app keeps running", async () => {
    const { user, backend } = await setupWithUpdate();
    backend.updateInstallFailure = { code: "updateInstallFailed" };
    await checkNow();
    await user.click(within(await screen.findByRole("dialog", { name: /is available/ })).getByRole("button", { name: "Update and restart" }));
    const dialog = await screen.findByRole("dialog", { name: /is available/ });
    expect(await within(dialog).findByText("The update could not be installed. The current version keeps running.")).toBeTruthy();
    expect(backend.installedUpdate).toBeNull();
  });
});

describe("Linux .deb/.rpm (no self-update)", () => {
  it("offers Download, which opens the release page through the confirmed opener", async () => {
    const { user, backend } = await setupWithUpdate(makeMockUpdate("manual"));
    act(() => useStore.setState({ platform: "linux" }));
    const open = vi.spyOn(backend, "appOpenExternal");
    const install = vi.spyOn(backend, "updateInstall");
    await checkNow();
    const dialog = await screen.findByRole("dialog", { name: /is available/ });
    expect(within(dialog).getByText(/installed from a .deb or .rpm package/)).toBeTruthy();
    expect(within(dialog).queryByRole("button", { name: "Update and restart" })).toBeNull();
    await user.click(within(dialog).getByRole("button", { name: "Download" }));
    expect(open).toHaveBeenCalledWith("https://github.com/shellfin-app/shellfin/releases/tag/v0.2.0", expect.any(String), expect.any(String));
    expect(install).not.toHaveBeenCalled();
  });
});

describe("settings: General", () => {
  it("has one row per setting: automatic checks and the channel, saved with the rest", async () => {
    const { user, backend } = setupApp();
    const update = vi.spyOn(backend, "settingsUpdate");
    await user.click(screen.getByRole("button", { name: "Settings" }));
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
    await user.click(within(dialog).getByRole("tab", { name: "General" }));
    const auto = within(dialog).getByRole("checkbox", { name: "Check for updates automatically" });
    expect(auto).toHaveProperty("checked", true);
    const channel = within(dialog).getByLabelText("Update channel");
    expect(channel).toHaveProperty("value", "stable");
    expect(within(channel).getAllByRole("option").map((o) => o.textContent)).toEqual(["Stable", "Experimental"]);
    await user.click(auto);
    await user.selectOptions(channel, "Experimental");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(update).toHaveBeenCalledOnce());
    expect(update.mock.calls[0]?.[0].general).toMatchObject({ autoUpdate: false, updateChannel: "experimental" });
  });
});
