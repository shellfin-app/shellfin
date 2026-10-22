// Updater (ARCHITECTURE §9.8): the update dialog, the "Check for updates…" action, the quit-flow hand-off and
// the settings rows, against the MockBackend (which models the backend contract of src-tauri/src/updater.rs).
import { readFileSync } from "node:fs";
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
    expect(notes?.textContent).toContain("line one <img src=x onerror=alert(1)>");
    expect(notes?.textContent).toContain("\\x1b[2J⟨U+202E⟩evil");
    expect(notes?.querySelector("img")).toBeNull();
    expect(Array.from(notes?.textContent ?? "").length).toBeLessThanOrEqual(NOTES_DISPLAY_LIMIT);
    expect(within(dialog).getByText(/The notes are shortened/)).toBeTruthy();
    for (const name of ["Update and restart", "Later", "Skip this version"]) {
      expect(within(dialog).getByRole("button", { name })).toBeTruthy();
    }
  });

  it("the notes are rendered as headings, lists, code and bold (the real 0.1.0 CHANGELOG section)", async () => {
    const changelog = readFileSync("CHANGELOG.md", "utf8").replace(/\r\n/g, "\n");
    const start = changelog.indexOf("## [0.1.0]");
    const body = changelog.slice(changelog.indexOf("\n", start) + 1, changelog.indexOf("\n", start) + 1 + 3000);
    await setupWithUpdate({ ...makeMockUpdate(), notes: `${body}\n\nA **bold** word.` });
    await checkNow();
    const notes = (await screen.findByRole("dialog", { name: /is available/ })).querySelector(".update__notes");
    expect(notes?.querySelectorAll("h4").length).toBeGreaterThan(1);
    expect(notes?.querySelector("h4")?.textContent).toBe("Connections");
    expect(notes?.querySelectorAll("ul > li").length).toBeGreaterThan(3);
    expect(notes?.querySelector("li code")?.textContent).toBe("ProxyJump");
    expect(notes?.querySelector("strong")?.textContent).toBe("bold");
    // Wrapped bullets are one item with the continuation joined by a space.
    const item = Array.from(notes?.querySelectorAll("li") ?? []).find((li) => li.textContent?.startsWith("Automatic reconnect"));
    expect(item?.textContent).toContain("terminal scrollback; manual disconnect and reconnect;");
    expect(item?.textContent).not.toContain("\n");
  });

  it("controls and bidi characters are escaped in headings, items, code and link text", async () => {
    const bad = "\u001b[2J‮";
    await setupWithUpdate({
      ...makeMockUpdate(),
      notes: `### head${bad}\n\n- item${bad}\n- \`code${bad}\`\n- [link${bad}](https://example.com/)`,
    });
    await checkNow();
    const notes = (await screen.findByRole("dialog", { name: /is available/ })).querySelector(".update__notes");
    const text = notes?.textContent ?? "";
    expect(text).not.toContain("\u001b");
    expect(text).not.toContain("‮");
    expect(notes?.querySelector("h4")?.textContent).toBe("head\\x1b[2J⟨U+202E⟩");
    expect(notes?.querySelector("code")?.textContent).toBe("code\\x1b[2J⟨U+202E⟩");
    expect(text.match(/\\x1b\[2J⟨U\+202E⟩/g)).toHaveLength(4);
  });

  it("hostile markup creates no elements and only allowlisted links are clickable", async () => {
    const notes = [
      "<img src=x onerror=alert(1)> <b>x</b>",
      "[x](javascript:alert(1))",
      "[httpGh](http://github.com/shellfin-app/shellfin)",
      "[evilHost](https://github.com.evil.example/shellfin-app/shellfin)",
      "[cred](https://user@github.com/shellfin-app/shellfin)",
      "[port](https://github.com:444/shellfin-app/shellfin)",
      "[sibling](https://github.com/shellfin-app/shellfin-evil)",
      "[c](http://shellfin.app/) [e](https://shellfin.app.evil.example/) [f](https://user@shellfin.app/)",
      "[g](https://shellfin.app:8443/) [h](https://evil.shellfin.app/) [i](https://www.shellfin.app.evil.example/)",
      "[ok](https://github.com/shellfin-app/shellfin/releases/tag/v0.1.1)",
      "[a](https://shellfin.app/) [b](https://shellfin.app/docs#x) [d](https://www.shellfin.app/)",
    ].join("\n\n");
    const { user, backend } = await setupWithUpdate({ ...makeMockUpdate(), notes });
    const open = vi.spyOn(backend, "appOpenExternal").mockResolvedValue(true);
    await checkNow();
    const dialog = await screen.findByRole("dialog", { name: /is available/ });
    const box = dialog.querySelector(".update__notes");
    expect(box?.querySelector("img, b, a, script")).toBeNull();
    expect(box?.textContent).toContain("<img src=x onerror=alert(1)> <b>x</b>");
    const links = Array.from(box?.querySelectorAll("button.update__link") ?? []).map((b) => b.textContent);
    expect(links).toEqual(["ok", "a", "b", "d"]);
    // Rejected links show their text only, never the URL.
    expect(box?.textContent).toContain("evilHost");
    expect(box?.textContent).not.toContain("evil.example");
    expect(box?.textContent).not.toContain("javascript:");
    // The URL ends at the balancing parenthesis: no stray ")" is left behind as text.
    expect(Array.from(box?.querySelectorAll("p") ?? []).some((p) => p.textContent === "x")).toBe(true);

    await user.click(within(dialog).getByRole("button", { name: "ok" }));
    expect(open).toHaveBeenCalledWith(
      "https://github.com/shellfin-app/shellfin/releases/tag/v0.1.1",
      expect.any(String),
      expect.any(String),
    );
    await user.click(within(dialog).getByRole("button", { name: "b" }));
    expect(open).toHaveBeenLastCalledWith("https://shellfin.app/docs#x", expect.any(String), expect.any(String));
    expect(open).toHaveBeenCalledTimes(2);
    // Keyboard: focusable, Space activates.
    const d = within(dialog).getByRole("button", { name: "d" });
    d.focus();
    await user.keyboard(" ");
    expect(open).toHaveBeenLastCalledWith("https://www.shellfin.app/", expect.any(String), expect.any(String));
  });

  it("GitHub links are limited to the maintainers' paths; ambiguous URL text is never clickable", async () => {
    const gh = "https://github.com/shellfin-app/shellfin";
    const notes = [
      `[rel](${gh}/releases) [asset](${gh}/releases/download/v0.1.1/x) [file](${gh}/blob/main/CHANGELOG.md)`,
      `[tree](${gh}/tree/main/src) [issue](${gh}/issues/12) [pr](${gh}/pull/7) [root](${gh})`,
      `[raw](${gh}/raw/0123abc/x) [commit](${gh}/commit/0123abc) [blob](${gh}/blob/0123abc/x) [dev](${gh}/tree/dev)`,
      `[bad1](${gh}/issues/12abc) [bad2](${gh}/issues) [bad3](${gh}/pull/7/files)`,
      // After displayServerText: a control character becomes a visible backslash escape, which is rejected.
      `[ctl](${gh}/releases\u0001) [bidi](${gh}/releases‮)`,
      "[bs1](https:\\\\github.com\\shellfin-app\\shellfin) [bs2](https://github.com\\shellfin-app\\shellfin\\@evil.example)",
      `[enc1](${gh}/..%2F..%2Fevil) [enc2](${gh}/releases/..%5Cevil)`,
    ].join("\n\n");
    const { backend } = await setupWithUpdate({ ...makeMockUpdate(), notes });
    vi.spyOn(backend, "appOpenExternal").mockResolvedValue(true);
    await checkNow();
    const dialog = await screen.findByRole("dialog", { name: /is available/ });
    const links = Array.from(dialog.querySelectorAll(".update__notes button.update__link")).map((b) => b.textContent);
    expect(links).toEqual(["rel", "asset", "file", "tree", "issue", "pr", "root"]);
    // The rejected ones are still shown, as text.
    expect(dialog.querySelector(".update__notes")?.textContent).toContain("bs2");
  });

  it("opens the canonical URL that was checked, and shows it in the confirmation", async () => {
    const { user, backend } = await setupWithUpdate({
      ...makeMockUpdate(),
      notes: "[rel](https://GITHUB.COM:443/shellfin-app/shellfin/releases)",
    });
    const open = vi.spyOn(backend, "appOpenExternal").mockResolvedValue(true);
    await checkNow();
    const dialog = await screen.findByRole("dialog", { name: /is available/ });
    await user.click(within(dialog).getByRole("button", { name: "rel" }));
    const canonical = "https://github.com/shellfin-app/shellfin/releases";
    expect(open).toHaveBeenCalledWith(canonical, expect.any(String), expect.stringContaining(canonical));
  });

  it("notes within the backend's limit that exceed the display limit once escaped show the shortened hint", async () => {
    const notes = "\u0001".repeat(3000); // 3000 characters, 12 000 once escaped
    expect(notes.length).toBeLessThanOrEqual(NOTES_DISPLAY_LIMIT);
    await setupWithUpdate({ ...makeMockUpdate(), notes, notesTruncated: false });
    await checkNow();
    const dialog = await screen.findByRole("dialog", { name: /is available/ });
    expect(within(dialog).getByText(/The notes are shortened/)).toBeTruthy();
  });

  it("short notes show no shortened hint", async () => {
    await setupWithUpdate({ ...makeMockUpdate(), notes: "- a\n- b", notesTruncated: false });
    await checkNow();
    const dialog = await screen.findByRole("dialog", { name: /is available/ });
    expect(within(dialog).queryByText(/The notes are shortened/)).toBeNull();
  });

  it("pathological notes render quickly and stay bounded", async () => {
    const notes = [
      "**".repeat(1500),
      "`".repeat(1500),
      "x".repeat(2000),
      Array.from({ length: 2000 }, (_, i) => `${" ".repeat(2 + (i % 40))}- n${i}`).join("\n"),
    ].join("\n\n");
    await setupWithUpdate({ ...makeMockUpdate(), notes });
    const t0 = performance.now();
    await checkNow();
    const dialog = await screen.findByRole("dialog", { name: /is available/ });
    expect(performance.now() - t0).toBeLessThan(5000);
    const box = dialog.querySelector(".update__notes");
    expect(Array.from(box?.textContent ?? "").length).toBeLessThanOrEqual(NOTES_DISPLAY_LIMIT);
    expect(box?.querySelectorAll("*").length).toBeLessThan(5000);
  });

  it("notes cut in the middle of markup show the leftovers as text", async () => {
    await setupWithUpdate({ ...makeMockUpdate(), notes: "- **bold and `co\n\n[half](https://github.com/shellfin-app/shell" });
    await checkNow();
    const box = (await screen.findByRole("dialog", { name: /is available/ })).querySelector(".update__notes");
    expect(box?.textContent).toContain("**bold and `co");
    expect(box?.textContent).toContain("[half](https://github.com/shellfin-app/shell");
    expect(box?.querySelector("button, strong, code")).toBeNull();
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
  it("has one row per setting: automatic checks only, saved with the rest", async () => {
    const { user, backend } = setupApp();
    const update = vi.spyOn(backend, "settingsUpdate");
    await user.click(screen.getByRole("button", { name: "Settings" }));
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
    await user.click(within(dialog).getByRole("tab", { name: "General" }));
    const auto = within(dialog).getByRole("checkbox", { name: "Check for updates automatically" });
    expect(auto).toHaveProperty("checked", true);
    expect(within(dialog).queryByLabelText("Update channel")).toBeNull();
    await user.click(auto);
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(update).toHaveBeenCalledOnce());
    expect(update.mock.calls[0]?.[0].general).toMatchObject({ autoUpdate: false });
    expect(update.mock.calls[0]?.[0].general).not.toHaveProperty("updateChannel");
  });
});
