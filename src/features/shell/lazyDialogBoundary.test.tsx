import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { resetStore, useStore } from "../../state/store";
import { DialogHost } from "./DialogHost";

// The lazily loaded Settings dialog fails (as a chunk that cannot be loaded or a render error would).
vi.mock("../settings/SettingsDialog", () => ({
  SettingsDialog: () => {
    throw new Error("chunk failed");
  },
}));

describe("lazy dialogs in DialogHost", () => {
  beforeEach(() => {
    resetStore();
    vi.spyOn(console, "error").mockImplementation(() => {});
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("a failing lazy dialog closes with an error notice instead of taking the window down", async () => {
    render(
      <>
        <DialogHost />
        <p>still here</p>
      </>,
    );
    useStore.getState().openDialog({ kind: "settings" });
    await waitFor(() => expect(useStore.getState().dialog).toBeNull());
    expect(useStore.getState().notices.some((n) => n.error?.detail === "chunk failed")).toBe(true);
    expect(screen.getByText("still here")).toBeTruthy();
    // The next dialog gets a fresh boundary.
    useStore.getState().openDialog({ kind: "about" });
    expect(await screen.findByRole("dialog")).toBeTruthy();
  });
});
