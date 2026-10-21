import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import "../i18n";
import { Checkbox, Setting, Input } from "./Field";
import { InfoTip } from "./InfoTip";

const TEXT = "A longer explanation.";

describe("InfoTip", () => {
  it("is a real button named after the setting and linked to its text by aria-describedby", () => {
    render(<InfoTip setting="Font size" text={TEXT} />);
    const button = screen.getByRole("button", { name: "More about Font size" });
    expect(button.tagName).toBe("BUTTON");
    const tip = screen.getByRole("tooltip", { hidden: true });
    expect(button.getAttribute("aria-describedby")).toBe(tip.id);
    expect(tip.textContent).toBe(TEXT);
    expect(tip.className).not.toContain("--open");
  });

  it("opens on hover and on keyboard focus, closes on leave and blur, and never takes focus itself", async () => {
    const user = userEvent.setup();
    render(
      <>
        <InfoTip setting="X" text={TEXT} />
        <button type="button">next</button>
      </>,
    );
    const button = screen.getByRole("button", { name: "More about X" });
    const tip = screen.getByRole("tooltip", { hidden: true });
    await user.hover(button);
    expect(tip.className).toContain("--open");
    await user.unhover(button);
    expect(tip.className).not.toContain("--open");
    await user.tab();
    expect(document.activeElement).toBe(button);
    expect(tip.className).toContain("--open");
    await user.tab(); // focus moves on: no trap
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "next" }));
    expect(tip.className).not.toContain("--open");
  });

  it("Escape closes the tip and does not reach a surrounding handler", async () => {
    const user = userEvent.setup();
    const outer = vi.fn();
    render(
      <div onKeyDown={outer}>
        <InfoTip setting="X" text={TEXT} />
      </div>,
    );
    await user.tab();
    const tip = screen.getByRole("tooltip", { hidden: true });
    expect(tip.className).toContain("--open");
    await user.keyboard("{Escape}");
    expect(tip.className).not.toContain("--open");
    expect(outer).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "More about X" }));
  });
});

describe("Setting and Checkbox with info", () => {
  it("keep the control's accessible name free of the tip and label it by htmlFor", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <>
        <Setting label="Name" info={TEXT}>
          <Input defaultValue="a" />
        </Setting>
        <Checkbox label="Flag" info={TEXT} checked={false} onChange={onChange} />
      </>,
    );
    expect(screen.getByRole("textbox", { name: "Name" })).toBeTruthy();
    await user.click(screen.getByText("Flag"));
    expect(onChange).toHaveBeenCalledWith(true);
    expect(screen.getByRole("checkbox", { name: "Flag" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "More about Flag" })).toBeTruthy();
  });
});
