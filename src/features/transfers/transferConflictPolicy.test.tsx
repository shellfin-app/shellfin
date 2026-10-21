// transfers.onConflict (ARCHITECTURE §8.10): ask -> Replace dialog; overwrite -> silent; skip -> "skipped" rows.
import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import type { ConflictPolicy } from "../../ipc/types";
import { useStore } from "../../state/store";
import type { MockBackend } from "../../ipc/mock/mockBackend";
import { settleApp, setupConnected } from "../../test/harness";
import { applyTransferEvent } from "./transferActions";

afterEach(async () => {
  await settleApp();
  setBackend(null);
  vi.restoreAllMocks();
});

const rows = () => useStore.getState().transfers;

/** Saves the policy the way the Settings dialog does: the backend decides, the store mirrors it. */
async function setPolicy(backend: MockBackend, onConflict: ConflictPolicy): Promise<void> {
  const settings = useStore.getState().settings;
  if (!settings) throw new Error("no settings");
  const saved = await backend.settingsUpdate({ ...settings, transfers: { ...settings.transfers, onConflict } });
  act(() => useStore.getState().setSettings(saved));
}

describe("transfer event `skipped`", () => {
  it("is a finished row (not failed, not retryable) shown as skipped", async () => {
    const { sessionId } = await setupConnected();
    act(() => {
      applyTransferEvent(sessionId, "/srv/app", { type: "started", transferId: "t1", name: "a.txt", direction: "upload", total: 10 });
      applyTransferEvent(sessionId, "/srv/app", { type: "skipped", transferId: "t1" });
    });
    expect(rows()[0]).toMatchObject({ transferId: "t1", status: "skipped", retryable: false });
    await userOpenPopover();
    const popover = await screen.findByRole("dialog", { name: "Transfers" });
    expect(within(popover).getByText("Skipped, the file already exists")).toBeTruthy();
    expect(within(popover).queryByRole("button", { name: "Retry" })).toBeNull();
    expect(within(popover).queryByRole("button", { name: "Replace" })).toBeNull();
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });
});

async function userOpenPopover() {
  const button = await screen.findByRole("button", { name: /transfer/i });
  act(() => button.click());
}

describe("onConflict policy", () => {
  it("skip: the upload ends as skipped and no Replace dialog appears", async () => {
    const { user, backend } = await setupConnected({ scenario: "uploadConflict" });
    await setPolicy(backend, "skip");
    const retry = vi.spyOn(backend, "transferRetry");
    await user.click(screen.getByRole("button", { name: "Upload files…" }));
    await waitFor(() => expect(rows().filter((r) => r.status === "skipped").map((r) => r.name).sort()).toEqual(["README.md", "deploy.sh"]));
    await waitFor(() => expect(rows().find((r) => r.name === "notes.txt")?.status).toBe("done"));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(retry).not.toHaveBeenCalled();
  });

  it("overwrite: existing files are replaced without a question", async () => {
    const { user, backend } = await setupConnected({ scenario: "uploadConflict" });
    await setPolicy(backend, "overwrite");
    await user.click(screen.getByRole("button", { name: "Upload files…" }));
    await waitFor(() => expect(rows()).toHaveLength(3));
    await waitFor(() => expect(rows().every((r) => r.status === "done")).toBe(true));
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("ask: the Replace dialog is shown (and only then)", async () => {
    const { user, backend } = await setupConnected({ scenario: "uploadConflict" });
    await setPolicy(backend, "ask");
    await user.click(screen.getByRole("button", { name: "Upload files…" }));
    expect(await screen.findByRole("alertdialog", { name: "Replace these files?" })).toBeTruthy();
  });

  it("a remoteExists failure arriving while the policy is not ask never opens the Replace dialog", async () => {
    const { sessionId, backend } = await setupConnected();
    await setPolicy(backend, "skip");
    act(() => {
      applyTransferEvent(sessionId, "/srv/app", { type: "started", transferId: "t2", name: "x.txt", direction: "upload", total: 1 });
      applyTransferEvent(sessionId, "/srv/app", {
        type: "failed",
        transferId: "t2",
        error: { code: "remoteExists", params: { path: "/srv/app/x.txt" } },
        retryable: true,
      });
    });
    await new Promise((r) => setTimeout(r, 400));
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });
});
