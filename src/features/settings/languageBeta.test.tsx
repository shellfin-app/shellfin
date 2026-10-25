// The language picker's beta marker (#17): "(beta)" in the current UI language after the untranslated endonym, and
// a hint with a feedback link that opens through app_open_external (the native confirmation, no new origin).
import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { LANGUAGE_STATUS } from "../../i18n";
import { setBackend } from "../../ipc";
import { MockBackend } from "../../ipc/mock/mockBackend";
import { HELP_LINKS } from "../../lib/helpLinks";
import { GeneralSection, isBetaLanguage } from "./GeneralSection";
import type { SectionProps } from "./settingsFields";

const original = { ...LANGUAGE_STATUS };

afterEach(() => {
  Object.assign(LANGUAGE_STATUS, original);
  setBackend(null);
  vi.restoreAllMocks();
});

function renderSection(locale: string) {
  const form = {
    locale,
    confirmCloseWithTerminals: false,
    confirmQuit: "always",
    onStartup: "welcome",
    notifyOnDisconnect: false,
    autoUpdate: false,
  } as unknown as SectionProps["form"];
  render(<GeneralSection {...({ form, patch: () => {} } as unknown as SectionProps)} />);
}

describe("language picker: beta languages", () => {
  it("shows no marker and no hint while every language is complete", () => {
    renderSection("uk");
    expect(screen.getByRole("option", { name: "Українська" })).toBeTruthy();
    expect(screen.queryByText(/not yet reviewed/)).toBeNull();
  });

  it("marks a beta language in the picker and shows the hint when it is selected or reached through System", () => {
    LANGUAGE_STATUS.uk = "beta";
    vi.spyOn(navigator, "languages", "get").mockReturnValue(["uk-UA"]);
    expect(isBetaLanguage("uk")).toBe(true);
    expect(isBetaLanguage("system")).toBe(true); // "System" resolves to a beta language too (no opt-in gate)
    expect(isBetaLanguage("en")).toBe(false);
    renderSection("system");
    const option = screen.getByRole("option", { name: "Українська (beta)" });
    expect(option.hasAttribute("lang")).toBe(false); // mixed text: no lang on the option
    expect(screen.getByRole("option", { name: "English" }).getAttribute("lang")).toBe("en");
    expect(screen.getByText(/This translation is new and not yet reviewed/)).toBeTruthy();
  });

  it("the feedback link opens through app_open_external with the confirmation texts", () => {
    LANGUAGE_STATUS.uk = "beta";
    const backend = new MockBackend();
    setBackend(backend);
    const open = vi.spyOn(backend, "appOpenExternal");
    renderSection("uk");
    fireEvent.click(screen.getByRole("button", { name: "Report wording problems on GitHub…" }));
    expect(open).toHaveBeenCalledWith(HELP_LINKS.translationFeedback, "Open this page in your browser?", HELP_LINKS.translationFeedback);
    expect(HELP_LINKS.translationFeedback.startsWith("https://github.com/shellfin-app/shellfin/")).toBe(true);
  });
});
