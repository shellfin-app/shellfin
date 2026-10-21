// Settings > Appearance and the terminal background field: live preview, cancel reverts, save persists.
import { screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { setBackend } from "../../ipc";
import { setupApp } from "../../test/harness";
import { useStore } from "../../state/store";
import { disposeAllTerminals } from "../terminal/terminalRegistry";
import { installAppearance } from "./appearance";

afterEach(async () => {
  await new Promise((r) => setTimeout(r, 100));
  disposeAllTerminals();
  setBackend(null);
  window.localStorage.clear();
});

async function openSettings() {
  installAppearance();
  const ctx = setupApp();
  await ctx.user.click(screen.getByRole("button", { name: "Settings" }));
  const dialog = await screen.findByRole("dialog", { name: "Settings" });
  return { ...ctx, dialog };
}

describe("Appearance section", () => {
  it("previews the theme and the accent live, and Cancel restores the saved look", async () => {
    const { user, dialog } = await openSettings();
    await user.click(within(dialog).getByRole("tab", { name: "Appearance" }));
    await user.click(within(dialog).getByRole("radio", { name: "Light" }));
    await user.click(within(dialog).getByRole("radio", { name: "Pink" }));
    expect(document.documentElement.dataset.theme).toBe("light");
    expect(document.documentElement.dataset.accent).toBe("pink");
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(document.documentElement.dataset.theme).toBe("dark"));
    expect(document.documentElement.dataset.accent).toBe("teal");
    expect(useStore.getState().settings?.appearance.theme).toBe("dark");
  });

  it("Save stores the appearance and keeps it applied", async () => {
    const { user, dialog, backend } = await openSettings();
    await user.click(within(dialog).getByRole("tab", { name: "Appearance" }));
    await user.click(within(dialog).getByRole("radio", { name: "Light" }));
    await user.selectOptions(within(dialog).getByLabelText("Interface text size"), "15");
    await user.selectOptions(within(dialog).getByLabelText("Reduce motion"), "on");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Settings" })).toBeNull());
    expect(useStore.getState().settings?.appearance).toEqual({ theme: "light", accent: "teal", uiFontSize: 15, reduceMotion: "on" });
    expect((await backend.settingsGet()).appearance.theme).toBe("light");
    expect(document.documentElement.dataset.theme).toBe("light");
    expect(document.documentElement.dataset.motion).toBe("reduce");
  });
});

describe("Terminal background field", () => {
  it("picks a swatch, validates a custom colour and saves #RRGGBB", async () => {
    const { user, dialog } = await openSettings();
    await user.click(within(dialog).getByRole("tab", { name: "Terminal" }));
    await user.click(within(dialog).getByRole("radio", { name: /Warm paper #FBF9F4/ }));
    expect(document.documentElement.style.getPropertyValue("--term-bg")).toBe("#FBF9F4");
    expect(document.documentElement.dataset.termTone).toBe("light");

    const input = within(dialog).getByLabelText("Custom color") as HTMLInputElement;
    await user.clear(input);
    await user.type(input, "#12");
    expect(within(dialog).getByText(/exactly #RRGGBB/)).toBeTruthy();
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(useStore.getState().settings?.terminal.background).toBeNull(); // not saved while invalid

    await user.clear(input);
    await user.type(input, "#1a1b26");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(useStore.getState().settings?.terminal.background).toBe("#1A1B26"));
  });

  it("Same as theme clears the colour", async () => {
    const { user, dialog } = await openSettings();
    await user.click(within(dialog).getByRole("tab", { name: "Terminal" }));
    await user.click(within(dialog).getByRole("radio", { name: /Black|Near black/ }));
    await user.click(within(dialog).getByRole("radio", { name: "Same as theme" }));
    expect(document.documentElement.style.getPropertyValue("--term-bg")).toBe("");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Settings" })).toBeNull());
    expect(useStore.getState().settings?.terminal.background).toBeNull();
  });
});
