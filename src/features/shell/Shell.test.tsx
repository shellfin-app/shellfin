import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { setBackend } from "../../ipc";
import { makeAgent, makeBootstrap, makeEnv } from "../../ipc/mock/fixtures";
import { MockBackend } from "../../ipc/mock/mockBackend";
import { resetStore, useStore } from "../../state/store";
import { Shell } from "./Shell";

function seed() {
  const now = Date.now();
  const s = useStore.getState();
  s.setBootstrap(makeBootstrap(now));
  s.setEnv(makeEnv(now));
  s.setAgent(makeAgent());
}

describe("Shell — Welcome (Welcome.dc)", () => {
  beforeEach(() => {
    resetStore();
    setBackend(new MockBackend());
    seed();
  });
  afterEach(() => setBackend(null));

  it("renders the English welcome screen with chrome", () => {
    render(<Shell />);
    expect(screen.getByRole("heading", { name: "Where do you want to connect?" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Search hosts, files and commands" }).textContent).toContain(
      "Hosts, files, commands…",
    );
    expect(screen.getByPlaceholderText("Bookmark name or user@host:port")).toBeTruthy();
    expect(screen.getByRole("tab", { name: "Home" }).getAttribute("aria-selected")).toBe("true");
    expect(screen.getByText("Not connected", { selector: ".status-bar__item" })).toBeTruthy();
    expect(screen.getByText("v0.1.0")).toBeTruthy();
  });

  it("lists both bookmark groups and the ssh-config hint", () => {
    render(<Shell />);
    expect(screen.getByRole("button", { name: /^~\/\.ssh\/config\s*5$/ })).toBeTruthy();
    expect(screen.getByRole("button", { name: /^My bookmarks\s*3$/ })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Connect to prod-web-01" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Connect to vps-hetzner" })).toBeTruthy();
    expect(screen.getByText(/Hosts from ~\/\.ssh\/config are loaded automatically/)).toBeTruthy();
  });

  it("shows recents and environment chips", () => {
    render(<Shell />);
    const recent = screen.getByRole("heading", { name: "Recent" }).closest("section");
    expect(recent).not.toBeNull();
    expect(within(recent as HTMLElement).getByText("deploy@10.0.4.12")).toBeTruthy();
    expect(within(recent as HTMLElement).getByText("3 terminals")).toBeTruthy();
    expect(screen.getByText("5 hosts")).toBeTruthy();
    expect(screen.getByText("1 include")).toBeTruthy();
    expect(screen.getByText("2 keys: id_ed25519 and work_rsa")).toBeTruthy();
    expect(screen.getByText("14 entries")).toBeTruthy();
  });

  it("disables Files without a session and hides Port forwarding", () => {
    render(<Shell />);
    expect((screen.getByRole("button", { name: "Files" }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.queryByRole("button", { name: "Port forwarding" })).toBeNull();
  });

  it("opens a session tab from a bookmark row", async () => {
    const user = userEvent.setup();
    render(<Shell />);
    await user.dblClick(screen.getByRole("button", { name: "Connect to pi-home" }));
    await act(async () => {
      await new Promise((r) => setTimeout(r, 150));
    });
    expect(useStore.getState().sessionOrder).toHaveLength(1);
    expect(screen.getByRole("tab", { name: /pi-home/ })).toBeTruthy();
  });

  it("every icon-only button has an accessible name", () => {
    const { container } = render(<Shell />);
    for (const b of container.querySelectorAll("button:not([aria-hidden=true])")) {
      const name = b.getAttribute("aria-label") ?? b.textContent?.trim();
      expect(name, b.className).toBeTruthy();
    }
  });
});
