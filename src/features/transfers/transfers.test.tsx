import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import { useStore } from "../../state/store";
import { settleApp, setupConnected } from "../../test/harness";
import { retryTransfer } from "./transferActions";

afterEach(async () => {
  await settleApp();
  setBackend(null);
  vi.restoreAllMocks();
  // @ts-expect-error jsdom has no elementFromPoint; the tests that need it define it
  delete document.elementFromPoint;
});

const row = (name: RegExp | string) => screen.getByRole("treeitem", { name });
const rowsOf = () => useStore.getState().transfers;

async function openPopover(user: Awaited<ReturnType<typeof setupConnected>>["user"]) {
  await user.click(await screen.findByRole("button", { name: /transfer/i }));
  return screen.findByRole("dialog", { name: "Transfers" });
}

describe("uploads", () => {
  it("Upload files… starts the picked uploads, shows them in the popover and refreshes the folder when done", async () => {
    const { user, backend } = await setupConnected();
    const pick = vi.spyOn(backend, "transferUploadPick");
    await user.click(screen.getByRole("button", { name: "Upload files…" }));
    expect(pick).toHaveBeenCalledWith(expect.any(String), "/srv/app", "Upload files", false, expect.any(Function));

    await waitFor(() => expect(rowsOf()[0]?.status).toBe("done"));
    await screen.findByRole("treeitem", { name: /report\.txt/ });

    const popover = await openPopover(user);
    expect(within(popover).getByText("report.txt")).toBeTruthy();
    expect(within(popover).getByText(/Done/)).toBeTruthy();
    await user.click(within(popover).getByRole("button", { name: "Clear finished" }));
    await waitFor(() => expect(screen.queryByRole("button", { name: /transfer/i })).toBeNull());
  });

  it("shows live progress and Cancel stops a running transfer", async () => {
    // Steps are long enough that a CPU stall cannot let the 4-step transfer finish before Cancel is clicked.
    const slow = { timeout: 5000 };
    const { user } = await setupConnected({ transferStepMs: 600 });
    await user.click(screen.getByRole("button", { name: "Upload files…" }));
    const popover = await openPopover(user);
    await within(popover).findByText(/of .*KiB/, undefined, slow);
    await user.click(within(popover).getByRole("button", { name: "Cancel transfer of report.txt" }));
    await within(popover).findByText("Cancelled", undefined, slow);
    expect(rowsOf()[0]?.status).toBe("cancelled");
    expect(screen.queryByRole("treeitem", { name: /report\.txt/ })).toBeNull();
  });

  it("uploadConflict: existing remote files raise one 'Replace?' dialog; Replace all retries with overwrite", async () => {
    const { user, backend } = await setupConnected({ scenario: "uploadConflict" });
    const retry = vi.spyOn(backend, "transferRetry");
    await user.click(screen.getByRole("button", { name: "Upload files…" }));

    const dialog = await screen.findByRole("alertdialog", { name: "Replace these files?" });
    expect(within(dialog).getAllByRole("checkbox")).toHaveLength(2);
    expect(dialog.textContent).toContain("deploy.sh");
    expect(dialog.textContent).toContain("README.md");
    expect(dialog.textContent).toContain("Yours:");
    expect(dialog.textContent).toContain("On the server:");
    // Waiting for the Replace decision is not a failure.
    const statusButton = screen.getByRole("button", { name: /need your decision/ });
    expect(statusButton.textContent).toContain("2 transfers need your decision");
    expect(statusButton.textContent).not.toContain("failed");
    await waitFor(() => expect(rowsOf().filter((r) => r.status === "done").map((r) => r.name)).toEqual(["notes.txt"]));
    await user.click(within(dialog).getByRole("button", { name: "Replace selected (2)" }));
    await waitFor(() => expect(retry).toHaveBeenCalledTimes(2));
    expect(retry.mock.calls.map((c) => c[1])).toEqual([true, true]);
    await waitFor(() => expect(rowsOf().every((r) => r.status === "done")).toBe(true));
    expect(rowsOf().map((r) => r.name).sort()).toEqual(["README.md", "deploy.sh", "notes.txt"]);
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("uploadConflict: Skip dismisses the conflicting transfers (cancelled, forgotten by the backend, no Replace left)", async () => {
    const { user, backend } = await setupConnected({ scenario: "uploadConflict" });
    const retry = vi.spyOn(backend, "transferRetry");
    const cancel = vi.spyOn(backend, "transferCancel");
    await user.click(screen.getByRole("button", { name: "Upload files…" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Replace these files?" });
    await user.click(within(dialog).getByRole("button", { name: "Skip all" }));
    expect(retry).not.toHaveBeenCalled();
    expect(cancel).toHaveBeenCalledTimes(2);
    expect(rowsOf().filter((r) => r.status === "cancelled").map((r) => r.name).sort()).toEqual(["README.md", "deploy.sh"]);
    expect(rowsOf().some((r) => r.status === "failed")).toBe(false);
    const popover = await openPopover(user);
    expect(within(popover).queryByRole("button", { name: "Replace" })).toBeNull();
  });

  it("cancelling Replace and dropping the same file again lists 1 file, not 2", async () => {
    const { user, backend } = await setupConnected();
    document.elementFromPoint = () => row("deploy.sh");
    act(() => void backend.simulateDrop([{ name: "deploy.sh", isDir: false }]));
    let dialog = await screen.findByRole("alertdialog", { name: "Replace this file?" });
    await user.click(within(dialog).getByRole("button", { name: "Skip all" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());

    act(() => void backend.simulateDrop([{ name: "deploy.sh", isDir: false }]));
    dialog = await screen.findByRole("alertdialog", { name: "Replace this file?" });
    expect(within(dialog).getAllByRole("checkbox")).toHaveLength(1);
    expect(dialog.textContent).toContain("deploy.sh");
    expect(rowsOf().filter((r) => r.status === "failed")).toHaveLength(1);
  });

  it("a transfer cut off by a lost connection can be retried (without overwrite) after the reconnect", async () => {
    const { user, backend, sessionId } = await setupConnected({ transferStepMs: 100 });
    const retry = vi.spyOn(backend, "transferRetry");
    await user.click(screen.getByRole("button", { name: "Upload files…" }));
    await waitFor(() => expect(rowsOf()[0]?.status).toBe("running"));
    backend.loseConnection(sessionId);
    await waitFor(() => expect(rowsOf()[0]?.status).toBe("failed"));
    backend.restoreConnection(sessionId);
    const popover = await openPopover(user);
    await within(popover).findByText("The connection was lost.");
    await user.click(within(popover).getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(retry).toHaveBeenCalledOnce());
    expect(retry.mock.calls[0]?.[1]).toBe(false);
    await waitFor(() => expect(rowsOf().map((r) => r.status)).toEqual(["done"]), { timeout: 5000 });
  });

  it("retrying a transfer the backend has forgotten reports it and removes the Retry button", async () => {
    const { backend, sessionId } = await setupConnected();
    useStore.getState().upsertTransfer({
      transferId: "gone",
      sessionId,
      name: "old.txt",
      direction: "upload",
      kind: "file",
      bytes: 0,
      total: 1,
      status: "failed",
      retryable: true,
      error: { code: "remoteExists", params: { path: "/srv/app/old.txt" } },
      remoteDir: "/srv/app",
    });
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    await expect(retryTransfer("gone", true)).rejects.toMatchObject({ error: { code: "notFound" } });
    expect(rowsOf().find((r) => r.transferId === "gone")?.retryable).toBe(false);
    expect(backend).toBeTruthy();
  });
});

describe("downloads", () => {
  it("Download… from the context menu runs a download transfer", async () => {
    const { user, backend } = await setupConnected();
    const download = vi.spyOn(backend, "transferDownload");
    fireEvent.contextMenu(row("deploy.sh"), { clientX: 10, clientY: 10 });
    await user.click(await screen.findByRole("menuitem", { name: "Download…" }));
    expect(download).toHaveBeenCalledWith(expect.any(String), "/srv/app/deploy.sh", "Download file", expect.any(Function));
    await waitFor(() => expect(rowsOf()[0]).toMatchObject({ name: "deploy.sh", direction: "download", status: "done" }));
    expect(screen.queryByRole("treeitem", { name: /deploy\.sh \(1\)/ })).toBeNull();
  });
});

describe("OS file drops on the tree", () => {
  const hit = (el: Element) => {
    document.elementFromPoint = () => el;
  };

  it("drop:hover highlights the folder under the pointer, drop:leave clears it", async () => {
    const { backend } = await setupConnected();
    hit(row("nginx"));
    act(() => backend.simulateDropHover(40, 80));
    await waitFor(() => expect(row("nginx").className).toContain("file-row--drop"));
    act(() => backend.simulateDropLeave());
    await waitFor(() => expect(row("nginx").className).not.toContain("file-row--drop"));
  });

  it("dropping files on a folder uploads them there with the drop token; folders get a notice and are skipped", async () => {
    const { backend } = await setupConnected();
    const dropped = vi.spyOn(backend, "transferUploadDropped");
    hit(row("nginx"));
    act(() => {
      backend.simulateDrop(
        [
          { name: "a.txt", isDir: false },
          { name: "photos", isDir: true },
        ],
        { x: 40, y: 80 },
      );
    });
    expect(await screen.findByText("Folders are not supported yet. Drop files instead.")).toBeTruthy();
    expect(dropped).toHaveBeenCalledOnce();
    const args = dropped.mock.calls[0];
    expect(args?.[1]).toMatch(/^drop-/); // the token, never a path
    expect(args?.[2]).toBe("/srv/app/nginx");
    await waitFor(() => expect(rowsOf().map((r) => r.name)).toContain("a.txt"));
    await waitFor(() => expect(rowsOf().find((r) => r.name === "a.txt")?.status).toBe("done"));
    expect(rowsOf().find((r) => r.name === "photos")).toBeUndefined(); // the folder failure is not a transfer row
  });

  it("files dropped on a file row go to that file's folder; the empty tree area means the tree root", async () => {
    const { backend } = await setupConnected();
    const dropped = vi.spyOn(backend, "transferUploadDropped");
    hit(row("deploy.sh"));
    act(() => void backend.simulateDrop([{ name: "b.txt", isDir: false }]));
    await waitFor(() => expect(dropped).toHaveBeenCalledOnce());
    expect(dropped.mock.calls[0]?.[2]).toBe("/srv/app");
    hit(screen.getByRole("tree", { name: "Remote files" }));
    act(() => void backend.simulateDrop([{ name: "c.txt", isDir: false }]));
    await waitFor(() => expect(dropped).toHaveBeenCalledTimes(2));
    expect(dropped.mock.calls[1]?.[2]).toBe("/srv/app");
  });

  it("dropping outside the tree uploads nothing and says where to drop", async () => {
    const { backend } = await setupConnected();
    const dropped = vi.spyOn(backend, "transferUploadDropped");
    hit(document.body);
    act(() => void backend.simulateDrop([{ name: "a.txt", isDir: false }]));
    expect(await screen.findByText("Drop files onto the file tree to upload them.")).toBeTruthy();
    expect(dropped).not.toHaveBeenCalled();
  });

  it("only folders dropped: the notice, no upload", async () => {
    const { backend } = await setupConnected();
    const dropped = vi.spyOn(backend, "transferUploadDropped");
    hit(row("nginx"));
    act(() => void backend.simulateDrop([{ name: "photos", isDir: true }]));
    expect(await screen.findByText("Folders are not supported yet. Drop files instead.")).toBeTruthy();
    expect(dropped).not.toHaveBeenCalled();
  });

  it("an expired drop token is reported (60 s TTL)", async () => {
    let now = 1_000_000;
    const { backend } = await setupConnected({ now: () => now });
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    hit(row("nginx"));
    // The webview sees the drop 61 s late (e.g. a stuck main thread): the token is gone.
    const original = backend.transferUploadDropped.bind(backend);
    vi.spyOn(backend, "transferUploadDropped").mockImplementation(async (...args) => {
      now += 61_000;
      return original(...args);
    });
    act(() => void backend.simulateDrop([{ name: "a.txt", isDir: false }]));
    expect(await screen.findByText("The dropped files are no longer available. Drop them again.")).toBeTruthy();
  });

  it("no drop is accepted while the connection is down", async () => {
    const { backend, sessionId } = await setupConnected();
    const dropped = vi.spyOn(backend, "transferUploadDropped");
    backend.loseConnection(sessionId);
    hit(row("nginx"));
    act(() => void backend.simulateDrop([{ name: "a.txt", isDir: false }]));
    await screen.findByText("Drop files onto the file tree to upload them.");
    expect(dropped).not.toHaveBeenCalled();
  });
});
