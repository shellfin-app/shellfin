import { act, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import "../i18n";
import { ContextMenu } from "../features/menu/ContextMenu";
import { useMenuStore } from "../features/menu/menuStore";
import { Dialog } from "./Dialog";
import { InfoTip } from "./InfoTip";

function setNativeValue(el: HTMLInputElement, value: string) {
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set?.call(el, value);
}

/** What an IME does when it commits: beforeinput, the value change, input (isComposing), compositionend. */
function commitComposition(el: HTMLInputElement, text: string) {
  el.dispatchEvent(new InputEvent("beforeinput", { bubbles: true, cancelable: false, inputType: "insertCompositionText", data: text }));
  setNativeValue(el, text);
  el.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "insertCompositionText", data: text, isComposing: true }));
  el.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: text }));
}

function Harness({ guardMs, onChange, onEnd }: { guardMs: number; onChange: (v: string) => void; onEnd: () => void }) {
  const [value, setValue] = useState("");
  return (
    <Dialog title="Password" inputGuardMs={guardMs}>
      <input
        aria-label="Secret"
        value={value}
        onChange={(e) => {
          setValue(e.target.value);
          onChange(e.target.value);
        }}
        onCompositionEnd={onEnd}
      />
    </Dialog>
  );
}

describe("Dialog Escape: inner widgets get it first", () => {
  it("an open InfoTip inside the dialog closes alone; the next Escape closes the dialog", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(
      <Dialog title="Settings" onClose={onClose}>
        <InfoTip setting="X" text="Longer text" />
      </Dialog>,
    );
    const tip = screen.getByRole("tooltip", { hidden: true });
    screen.getByRole("button", { name: "More about X" }).focus();
    expect(tip.className).toContain("--open");
    await user.keyboard("{Escape}");
    expect(tip.className).not.toContain("--open");
    expect(onClose).not.toHaveBeenCalled();
    await user.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("an open context menu closes alone; the dialog stays open until an unhandled Escape", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(
      <>
        <Dialog title="Bookmark" onClose={onClose}>
          <input aria-label="Host" />
        </Dialog>
        <ContextMenu />
      </>,
    );
    const input = screen.getByLabelText("Host");
    act(() => {
      useMenuStore.getState().open({
        x: 10,
        y: 10,
        items: [{ id: "copy", label: "Copy", onSelect: () => undefined }],
        restoreFocus: () => input.focus(),
      });
    });
    expect(screen.getByRole("menu")).toBeTruthy();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
    await user.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("ignores an Escape that originates outside the dialog", () => {
    const onClose = vi.fn();
    render(
      <>
        <button type="button">outside</button>
        <Dialog title="D" onClose={onClose}>
          <input aria-label="Host" />
        </Dialog>
      </>,
    );
    fireEvent.keyDown(screen.getByText("outside"), { key: "Escape" });
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.keyDown(screen.getByLabelText("Host"), { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});

describe("Dialog input guard — IME composition", () => {
  it("ignores the commit of a composition that started before the dialog opened", () => {
    document.body.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true })); // e.g. in the terminal
    const onChange = vi.fn();
    const onEnd = vi.fn();
    render(<Harness guardMs={500} onChange={onChange} onEnd={onEnd} />);
    const input = screen.getByLabelText("Secret") as HTMLInputElement;
    expect(document.activeElement).toBe(input);

    commitComposition(input, "ぱすわーど");
    expect(onChange).not.toHaveBeenCalled();
    expect(onEnd).not.toHaveBeenCalled();
    expect(input.value).toBe(""); // the browser's change was undone, too
  });

  it("is not time-bound: a slow IME that commits long after the 500 ms window is still ignored", () => {
    vi.useFakeTimers();
    try {
      document.body.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
      const onChange = vi.fn();
      render(<Harness guardMs={20} onChange={onChange} onEnd={() => undefined} />);
      act(() => {
        vi.advanceTimersByTime(60);
      });
      const input = screen.getByLabelText("Secret") as HTMLInputElement;
      commitComposition(input, "late");
      expect(onChange).not.toHaveBeenCalled();
      expect(input.value).toBe("");
    } finally {
      vi.useRealTimers();
    }
  });

  it("accepts a composition the user starts inside the dialog", () => {
    document.body.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
    const onChange = vi.fn();
    const onEnd = vi.fn();
    render(<Harness guardMs={500} onChange={onChange} onEnd={onEnd} />);
    const input = screen.getByLabelText("Secret") as HTMLInputElement;

    fireEvent.compositionStart(input); // a new composition, after the dialog opened
    commitComposition(input, "ok");
    expect(onChange).toHaveBeenCalledWith("ok");
    expect(onEnd).toHaveBeenCalledTimes(1);
    expect(input.value).toBe("ok");
  });

  it("dialogs without the guard are unaffected", () => {
    document.body.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
    const onChange = vi.fn();
    render(<Harness guardMs={0} onChange={onChange} onEnd={() => undefined} />);
    const input = screen.getByLabelText("Secret") as HTMLInputElement;
    commitComposition(input, "x");
    expect(onChange).toHaveBeenCalledWith("x");
  });
});
