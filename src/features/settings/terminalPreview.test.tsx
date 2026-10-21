import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import "../../i18n";
import { DEFAULT_APPEARANCE } from "../../lib/theme";
import { LIGHT_PALETTE } from "../terminal/terminalTheme";
import { TerminalPreview, type TerminalPreviewProps } from "./TerminalPreview";

const base: TerminalPreviewProps = {
  look: { appearance: { ...DEFAULT_APPEARANCE, theme: "dark" }, terminalBackground: null, terminalBackgroundText: "" },
  fontFamily: null,
  fontSize: "13",
  lineHeight: 1.2,
  letterSpacing: "0",
  cursorStyle: "block",
  cursorBlink: true,
};

const preview = () => screen.getByRole("img", { name: "Terminal preview" });
const cursor = () => preview().querySelector(".term-preview__cursor") as HTMLElement;

describe("TerminalPreview", () => {
  it("shows a prompt, coloured output and a prompt with the cursor, from static text", () => {
    render(<TerminalPreview {...base} />);
    expect(preview().textContent).toContain("deploy@web-01:~$ ls -la");
    expect(preview().textContent).toContain("deploy.sh");
    const lines = preview().querySelectorAll(".term-preview__line");
    expect(lines[lines.length - 1]?.textContent).toContain("deploy@web-01:~$");
    const coloured = [...preview().querySelectorAll<HTMLElement>("span[style]")].filter((s) => s.style.color !== "");
    expect(coloured.length).toBeGreaterThan(3);
  });

  it("uses the terminal palette of the shown theme and the custom background", () => {
    const { rerender } = render(<TerminalPreview {...base} />);
    expect(preview().style.background).toBe("rgb(15, 17, 21)");
    rerender(
      <TerminalPreview
        {...base}
        look={{ appearance: { ...DEFAULT_APPEARANCE, theme: "light" }, terminalBackground: null, terminalBackgroundText: "" }}
      />,
    );
    expect(preview().style.background).toBe("rgb(255, 255, 255)");
    expect(preview().style.color).toBe("rgb(31, 35, 41)");
    expect(LIGHT_PALETTE.foreground).toBe("#1F2329");
    rerender(<TerminalPreview {...base} look={{ ...base.look, terminalBackground: "#000000" }} />);
    expect(preview().style.background).toBe("rgb(0, 0, 0)");
  });

  it("follows the cursor style and blink setting", () => {
    const { rerender } = render(<TerminalPreview {...base} />);
    expect(cursor().className).toContain("term-preview__cursor--block");
    expect(cursor().className).toContain("term-preview__cursor--blink");
    rerender(<TerminalPreview {...base} cursorStyle="bar" cursorBlink={false} />);
    expect(cursor().className).toContain("term-preview__cursor--bar");
    expect(cursor().className).not.toContain("--blink");
    rerender(<TerminalPreview {...base} cursorStyle="underline" />);
    expect(cursor().className).toContain("term-preview__cursor--underline");
  });

  it("applies font, size, line height and letter spacing live and never uses ligatures", () => {
    const { rerender } = render(<TerminalPreview {...base} />);
    expect(preview().style.fontVariantLigatures).toBe("none");
    rerender(<TerminalPreview {...base} fontFamily="Hack" fontSize="18" lineHeight={1.5} letterSpacing="2" />);
    expect(preview().style.fontFamily).toContain("Hack");
    expect(preview().style.fontSize).toBe("18px");
    expect(preview().style.lineHeight).toBe("1.5");
    expect(preview().style.letterSpacing).toBe("2px");
    // Half-typed values fall back instead of breaking the preview.
    rerender(<TerminalPreview {...base} fontSize="" letterSpacing="x" />);
    expect(preview().style.fontSize).toBe("13px");
    expect(preview().style.letterSpacing).toBe("0px");
  });
});
