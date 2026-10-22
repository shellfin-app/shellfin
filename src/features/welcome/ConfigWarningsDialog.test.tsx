// ssh config load warnings on the Welcome screen: the "N warnings" chip and the list dialog (§7.3, §7.4).
import { act, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { setLanguage } from "../../i18n";
import { makeSshConfig } from "../../ipc/mock/fixtures";
import { MockBackend } from "../../ipc/mock/mockBackend";
import type { ConfigWarning, SshConfigSnapshot } from "../../ipc/types";
import { useStore } from "../../state/store";
import { settleApp, setupApp } from "../../test/harness";
import { MAX_SHOWN_CONFIG_WARNINGS } from "./ConfigWarningsDialog";

// Hostile characters are built from code points so no invisible character sits in this source.
const RLO = String.fromCodePoint(0x202e);
const ZWSP = String.fromCodePoint(0x200b);
const ESC = String.fromCodePoint(0x1b);
const BACKSLASH = String.fromCharCode(92);

const badLine = (line: number, text = "port", file = "~/.ssh/config"): ConfigWarning => ({
  location: { file, line },
  code: "badLine",
  params: { text },
});

function withWarnings(warnings: ConfigWarning[], droppedWarnings = 0): SshConfigSnapshot {
  return { ...makeSshConfig(Date.now()), warnings, droppedWarnings };
}

function setConfig(config: SshConfigSnapshot) {
  act(() => useStore.getState().setSshConfig(config));
}

const chip = (name: RegExp) => screen.queryByRole("button", { name });

afterEach(async () => {
  await setLanguage("en");
  await settleApp(20);
});

describe("ssh config warnings on the Welcome screen", () => {
  it("shows no chip without warnings", () => {
    setupApp();
    setConfig(withWarnings([]));
    expect(chip(/warning/i)).toBeNull();
  });

  it("counts reported and dropped warnings in the chip (en plurals)", () => {
    setupApp();
    setConfig(withWarnings([badLine(3)]));
    expect(chip(/^1 warning$/)).not.toBeNull();
    setConfig(withWarnings([badLine(3), badLine(4)], 10));
    expect(chip(/^12 warnings$/)).not.toBeNull();
    // Only dropped ones (never happens in practice, but the count is the sum).
    setConfig(withWarnings([], 1));
    expect(chip(/^1 warning$/)).not.toBeNull();
  });

  it("uses the Ukrainian plural forms", async () => {
    setupApp();
    await act(() => setLanguage("uk"));
    for (const [n, text] of [
      [1, "1 попередження"],
      [3, "3 попередження"],
      [5, "5 попереджень"],
      [21, "21 попередження"],
    ] as const) {
      setConfig(withWarnings(Array.from({ length: n }, (_, i) => badLine(i + 1))));
      expect(chip(new RegExp(`^${text}$`))).not.toBeNull();
    }
    setConfig(withWarnings([badLine(1)], 4));
    await act(async () => {
      chip(/^5 попереджень$/)?.click();
    });
    const dialog = screen.getByRole("dialog", { name: "Попередження конфігурації SSH" });
    expect(within(dialog).getByText("і ще 4")).toBeTruthy();
    expect(within(dialog).getByText("Не вдалося розібрати рядок: port")).toBeTruthy();
  });

  it("lists each warning as file:line and its description, keyboard first", async () => {
    const { user } = setupApp();
    setConfig(
      withWarnings([
        badLine(12),
        { location: { file: "~/.ssh/config", line: 30 }, code: "unsupportedMatchCriteria", params: { criteria: "exec" } },
        { location: { file: "~/.ssh/conf.d/x.conf", line: 0 }, code: "untrustedFile", params: { file: "~/.ssh/conf.d/x.conf" } },
        badLine(0, "?", "~/.ssh/id_test"),
        { location: { file: "~/.ssh/config", line: 40 }, code: "includeTooManyFiles", params: { pattern: "many/*.conf" } },
      ]),
    );
    const button = chip(/^5 warnings$/);
    expect(button).not.toBeNull();
    button?.focus();
    await user.keyboard("{Enter}");
    const dialog = screen.getByRole("dialog", { name: "SSH config warnings" });
    const items = within(within(dialog).getByRole("list", { name: "Warnings" })).getAllByRole("listitem");
    expect(items.map((li) => li.textContent)).toEqual([
      "~/.ssh/config:12Line could not be parsed: port",
      "~/.ssh/config:30Unsupported Match criteria: exec.",
      // A file-level warning (line 0) that names its file shows it once.
      "~/.ssh/conf.d/x.conf has unsafe ownership or permissions and was ignored.",
      // A whole file skipped (line 0 badLine): the file, then what happened to it.
      "~/.ssh/id_testThe file was skipped: it is not a config file (key material or binary data).",
      "~/.ssh/config:40The Include pattern many/*.conf matches too many files.",
    ]);
    expect(within(dialog).queryByText(/more$/)).toBeNull();
    // The Close button has the focus; Escape closes and the chip gets it back.
    expect(document.activeElement?.textContent).toBe("Close");
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(button);
  });

  it("ends with 'and N more' for dropped warnings and rows beyond the display limit", async () => {
    const { user } = setupApp();
    setConfig(withWarnings([badLine(1)], 1));
    await user.click(chip(/^2 warnings$/) as HTMLElement);
    expect(screen.getByText("and 1 more")).toBeTruthy();
    await user.keyboard("{Escape}");

    const many = Array.from({ length: MAX_SHOWN_CONFIG_WARNINGS + 7 }, (_, i) => badLine(i + 1));
    setConfig(withWarnings(many, 1234));
    await user.click(chip(new RegExp(`^${many.length + 1234} warnings$`)) as HTMLElement);
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getAllByRole("listitem")).toHaveLength(MAX_SHOWN_CONFIG_WARNINGS);
    expect(within(dialog).getByText("and 1241 more")).toBeTruthy();
  });

  it("renders hostile config text escaped (bidi, invisible, controls, markup)", async () => {
    const { user, container } = setupApp();
    const file = `~/.ssh/<b>evil${RLO}${ZWSP}.conf`;
    setConfig(
      withWarnings([
        { location: { file, line: 7 }, code: "includeCycle", params: { file } },
        badLine(8, `po${ESC}[31mrt${RLO}`),
        { location: { file: "~/.ssh/config", line: 9 }, code: "proxyCommandUnsupported", params: { alias: `<img src=x>${ZWSP}` } },
      ]),
    );
    await user.click(chip(/^3 warnings$/) as HTMLElement);
    const dialog = screen.getByRole("dialog");
    const text = dialog.textContent ?? "";
    for (const raw of [RLO, ZWSP, ESC]) expect(text).not.toContain(raw);
    expect(text).toContain("~/.ssh/<b>evil⟨U+202E⟩⟨U+200B⟩.conf:7");
    expect(text).toContain("~/.ssh/<b>evil⟨U+202E⟩⟨U+200B⟩.conf includes itself");
    expect(text).toContain(`po${BACKSLASH}x1b[31mrt⟨U+202E⟩`);
    expect(text).toContain("<img src=x>⟨U+200B⟩ uses ProxyCommand");
    expect(container.querySelector("b, img")).toBeNull();
    expect(dialog.querySelector("b, img")).toBeNull();
  });

  it("closes cleanly when the warnings go away while the dialog is open: focus goes to quick connect", async () => {
    const { user } = setupApp();
    setConfig(withWarnings([badLine(1)]));
    await user.click(chip(/^1 warning$/) as HTMLElement);
    setConfig(withWarnings([]));
    expect(screen.getByText("No warnings.")).toBeTruthy();
    expect(chip(/warning/i)).toBeNull();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(screen.getByRole("textbox", { name: "Quick connect" }));
  });
});

describe("MockBackend configWarnings scenario", () => {
  it("delivers warnings and dropped warnings in the snapshot; readSshConfig off empties both", async () => {
    const backend = new MockBackend({ scenario: "configWarnings" });
    const boot = await backend.appFrontendReady();
    expect(boot.sshConfig.warnings.length).toBeGreaterThan(50);
    expect(boot.sshConfig.droppedWarnings).toBe(1234);
    expect((await backend.sshConfigReload()).droppedWarnings).toBe(1234);
    const happy = await new MockBackend().sshConfigReload();
    expect(happy.warnings).toEqual([]);
    expect(happy.droppedWarnings).toBe(0);
    const settings = await backend.settingsGet();
    await backend.settingsUpdate({ ...settings, connection: { ...settings.connection, readSshConfig: false } });
    const off = await backend.sshConfigReload();
    expect(off.warnings).toEqual([]);
    expect(off.droppedWarnings).toBe(0);
  });
});
