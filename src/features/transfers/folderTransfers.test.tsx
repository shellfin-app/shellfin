// Folder downloads in the UI (ARCHITECTURE §6.6 "Folders"): the store model of a job, the popover row, the decision
// and conflict dialogs and the tree menu item, against the `folderTransfers` mock scenario.
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import { MockBackend } from "../../ipc/mock/mockBackend";
import type { DecisionRequest, SkipCounts, TransferDecision, TransferEvent } from "../../ipc/types";
import { MAX_FOLDER_ISSUES, emptySkipCounts } from "../../state/slices/transfers";
import { resetStore, useStore } from "../../state/store";
import { settleApp, setupConnected } from "../../test/harness";
import { applyTransferEvent, decideTransfer, needsFolderDecision } from "./transferActions";

afterEach(async () => {
  await settleApp();
  setBackend(null);
  vi.restoreAllMocks();
});

const rows = () => useStore.getState().transfers;
const folderOf = (id: string) => rows().find((r) => r.transferId === id)?.folder;
const counts = (patch: Partial<SkipCounts>): SkipCounts => ({ ...emptySkipCounts(), ...patch });
const RLO = String.fromCodePoint(0x202e);

describe("applyTransferEvent: folder jobs", () => {
  beforeEach(() => resetStore());
  const apply = (event: TransferEvent) => applyTransferEvent("s1", null, event);
  const started = (id = "f1") => apply({ type: "started", transferId: id, name: "release", direction: "download", total: null, kind: "folder" });

  it("started -> scanning row with a folder state; a file transfer gets none", () => {
    started();
    apply({ type: "started", transferId: "x", name: "a.txt", direction: "download", total: 5, kind: "file" });
    expect(rows()[0]).toMatchObject({ kind: "folder", remoteDir: null, status: "running", total: null, folder: { phase: "scanning", issues: [] } });
    expect(rows()[1]).toMatchObject({ kind: "file" });
    expect(rows()[1]?.folder).toBeUndefined();
  });

  it("a retry (`started.total` set) is copying from the start, not scanning", () => {
    apply({ type: "started", transferId: "r1", name: "release", direction: "download", total: 4096, kind: "folder" });
    expect(rows()[0]).toMatchObject({ total: 4096, folder: { phase: "copying" } });
  });

  it("scanProgress / scanned / awaitingDecision / progress move through the phases", () => {
    started();
    apply({ type: "scanProgress", transferId: "f1", files: 12, dirs: 3, bytes: 400 });
    expect(folderOf("f1")).toMatchObject({ phase: "scanning", scan: { files: 12, dirs: 3, bytes: 400 } });
    apply({ type: "scanned", transferId: "f1", files: 20, dirs: 4, bytes: 900, skipped: counts({ symlink: 2 }), renamed: 1 });
    expect(rows()[0]?.total).toBe(900);
    expect(folderOf("f1")).toMatchObject({ phase: "copying", filesTotal: 20, renamed: 1, skipped: { symlink: 2 } });
    apply({ type: "awaitingDecision", transferId: "f1", request: { kind: "largeTree", files: 20, bytes: 900 } });
    expect(folderOf("f1")).toMatchObject({ phase: "awaiting", pending: { kind: "largeTree" } });
    apply({ type: "progress", transferId: "f1", bytes: 100, total: 900, filesDone: 2, filesTotal: 20, current: "a.txt", failed: 1, skipped: counts({ symlink: 2, exists: 1 }) });
    expect(rows()[0]).toMatchObject({ bytes: 100, total: 900 });
    expect(folderOf("f1")).toMatchObject({ phase: "copying", pending: null, filesDone: 2, current: "a.txt", failed: 1, skipped: { exists: 1 } });
    // A plain progress (file fields absent) keeps the folder counters.
    apply({ type: "progress", transferId: "f1", bytes: 200, total: 900 });
    expect(folderOf("f1")).toMatchObject({ filesDone: 2, failed: 1 });
  });

  it("itemIssue keeps at most 1 000 issues", () => {
    started();
    for (let i = 0; i < MAX_FOLDER_ISSUES + 100; i += 1) {
      apply({ type: "itemIssue", transferId: "f1", itemId: null, relPath: `l${i}`, issue: { kind: "skipped", reason: "symlink" } });
    }
    expect(folderOf("f1")?.issues).toHaveLength(MAX_FOLDER_ISSUES);
    expect(folderOf("f1")?.issues[0]).toEqual({ itemId: null, relPath: "l0", issue: { kind: "skipped", reason: "symlink" } });
  });

  it("conflicts: the first event brings the sample, later ones update the count", () => {
    started();
    const sample = [{ itemId: 3, relPath: "a/b.txt", source: { size: 1, mtime: 2 }, target: { size: 3, mtime: 4 } }];
    apply({ type: "conflicts", transferId: "f1", count: 1, sample, round: 0 });
    apply({ type: "conflicts", transferId: "f1", count: 7, sample: [], round: 0 });
    expect(folderOf("f1")?.conflicts).toEqual({ count: 7, sample, round: 0 });
  });

  it("done with a summary: retryable when items failed; failed / cancelled / skipped clear what was pending", () => {
    started();
    apply({ type: "conflicts", transferId: "f1", count: 1, sample: [], round: 0 });
    const summary = { localName: "release (2)", filesDone: 8, skipped: counts({ exists: 1 }), failed: 1, renamed: 2, retryable: true };
    apply({ type: "done", transferId: "f1", bytes: 50, entry: null, summary });
    expect(rows()[0]).toMatchObject({ status: "done", retryable: true, bytes: 50 });
    expect(folderOf("f1")).toMatchObject({ phase: "finished", summary, filesDone: 8, failed: 1, conflicts: null, pending: null });

    started("f2");
    apply({ type: "awaitingDecision", transferId: "f2", request: { kind: "lowSpace", needed: 10, available: 1 } });
    apply({ type: "failed", transferId: "f2", error: { code: "timeout", params: { op: "treeScan" } }, retryable: true });
    expect(rows()[1]).toMatchObject({ status: "failed", retryable: true, folder: { pending: null, phase: "finished" } });
    started("f3");
    apply({ type: "cancelled", transferId: "f3" });
    started("f4");
    apply({ type: "skipped", transferId: "f4" });
    expect(rows().slice(2).map((r) => r.status)).toEqual(["cancelled", "skipped"]);
  });
});

describe("decideTransfer: the next request arrives before the answer resolves", () => {
  beforeEach(() => resetStore());
  const apply = (event: TransferEvent) => applyTransferEvent("s1", null, event);
  const large: DecisionRequest = { kind: "largeTree", files: 20_000, bytes: 1 };
  const root: DecisionRequest = { kind: "rootExists", name: "release", isDir: true };
  const space: DecisionRequest = { kind: "lowSpace", needed: 10, available: 1 };
  const chains: [string, DecisionRequest, TransferDecision, DecisionRequest][] = [
    ["largeTree -> rootExists", large, "proceed", root],
    ["largeTree -> lowSpace", large, "proceed", space],
    ["rootExists -> lowSpace", root, "merge", space],
  ];
  it.each(chains)("%s: the follow-up request stays pending", async (_name, first, answer, next) => {
    const backend = new MockBackend();
    setBackend(backend);
    // The backend's next question is delivered on the channel before the command resolves.
    vi.spyOn(backend, "transferDecide").mockImplementation(async (id) => {
      apply({ type: "awaitingDecision", transferId: id, request: next });
    });
    apply({ type: "started", transferId: "f1", name: "release", direction: "download", total: null, kind: "folder" });
    apply({ type: "awaitingDecision", transferId: "f1", request: first });
    await decideTransfer("f1", answer);
    expect(folderOf("f1")).toMatchObject({ phase: "awaiting", pending: next });
  });

  it("a failed answer restores the request only if nothing newer arrived", async () => {
    const backend = new MockBackend();
    setBackend(backend);
    vi.spyOn(backend, "transferDecide").mockRejectedValue({ code: "invalidArgument", params: { field: "decision" } });
    apply({ type: "started", transferId: "f1", name: "release", direction: "download", total: null, kind: "folder" });
    apply({ type: "awaitingDecision", transferId: "f1", request: large });
    await expect(decideTransfer("f1", "proceed")).rejects.toBeTruthy();
    expect(folderOf("f1")).toMatchObject({ pending: large });
  });
});

describe("decideTransfer: the answer resolves first, then the next request arrives", () => {
  beforeEach(() => resetStore());
  const apply = (event: TransferEvent) => applyTransferEvent("s1", null, event);
  const large: DecisionRequest = { kind: "largeTree", files: 20_000, bytes: 1 };
  const root: DecisionRequest = { kind: "rootExists", name: "release", isDir: true };
  const space: DecisionRequest = { kind: "lowSpace", needed: 10, available: 1 };
  const sample = [{ itemId: 3, relPath: "a/b.txt", source: { size: 1, mtime: 2 }, target: { size: 3, mtime: 4 } }];
  const started = () => apply({ type: "started", transferId: "f1", name: "release", direction: "download", total: null, kind: "folder" });
  const withBackend = () => {
    const backend = new MockBackend();
    setBackend(backend);
    return backend;
  };

  const chains: [string, DecisionRequest, TransferDecision, DecisionRequest][] = [
    ["largeTree -> rootExists", large, "proceed", root],
    ["largeTree -> lowSpace", large, "proceed", space],
    ["rootExists -> lowSpace", root, "merge", space],
  ];
  it.each(chains)("%s: the follow-up request after the resolved answer is pending", async (_name, first, answer, next) => {
    vi.spyOn(withBackend(), "transferDecide").mockResolvedValue(undefined);
    started();
    apply({ type: "awaitingDecision", transferId: "f1", request: first });
    await decideTransfer("f1", answer);
    expect(folderOf("f1")).toMatchObject({ phase: "copying", pending: null });
    apply({ type: "awaitingDecision", transferId: "f1", request: next });
    expect(folderOf("f1")).toMatchObject({ phase: "awaiting", pending: next, decisionSeq: 2 });
  });

  it("a failed answer keeps a newer request that arrived meanwhile (decision)", async () => {
    vi.spyOn(withBackend(), "transferDecide").mockImplementation(async (id) => {
      apply({ type: "awaitingDecision", transferId: id, request: space });
      throw { code: "invalidArgument", params: { field: "decision" } };
    });
    started();
    apply({ type: "awaitingDecision", transferId: "f1", request: large });
    await expect(decideTransfer("f1", "proceed")).rejects.toBeTruthy();
    expect(folderOf("f1")).toMatchObject({ phase: "awaiting", pending: space });
  });

  it("a failed answer keeps newer conflicts that arrived meanwhile (conflicts)", async () => {
    vi.spyOn(withBackend(), "transferDecide").mockImplementation(async (id) => {
      apply({ type: "conflicts", transferId: id, count: 4, sample: [], round: 1 });
      throw { code: "invalidArgument", params: { field: "decision" } };
    });
    started();
    apply({ type: "conflicts", transferId: "f1", count: 1, sample, round: 0 });
    await expect(decideTransfer("f1", "skipAll")).rejects.toBeTruthy();
    expect(folderOf("f1")).toMatchObject({ conflicts: { count: 4, sample: [], round: 1 }, conflictsAnswered: 0 });
  });

  it("a failed answer brings the conflicts back and accepts their round again", async () => {
    vi.spyOn(withBackend(), "transferDecide").mockRejectedValue({ code: "invalidArgument", params: { field: "decision" } });
    started();
    apply({ type: "conflicts", transferId: "f1", count: 1, sample, round: 0 });
    await expect(decideTransfer("f1", "replaceAll")).rejects.toBeTruthy();
    expect(folderOf("f1")).toMatchObject({ conflicts: { count: 1, sample, round: 0 }, conflictsAnswered: null });
    apply({ type: "conflicts", transferId: "f1", count: 2, sample: [], round: 0 });
    expect(folderOf("f1")?.conflicts).toEqual({ count: 2, sample, round: 0 });
  });

  it("a late conflicts event of the answered round never brings the dialog back", async () => {
    const backend = withBackend();
    // One stale event is delivered while the call runs, another after it resolved.
    vi.spyOn(backend, "transferDecide").mockImplementation(async (id) => {
      apply({ type: "conflicts", transferId: id, count: 2, sample, round: 0 });
    });
    started();
    apply({ type: "conflicts", transferId: "f1", count: 1, sample, round: 0 });
    await decideTransfer("f1", "replaceOlder");
    apply({ type: "conflicts", transferId: "f1", count: 3, sample, round: 0 });
    const row = rows()[0];
    expect(row?.folder).toMatchObject({ conflicts: null, conflictsAnswered: 0, conflictsSeq: 1 });
    expect(row && needsFolderDecision(row)).toBe(false);
    // A new round (the job asks again) is a new question.
    apply({ type: "conflicts", transferId: "f1", count: 1, sample, round: 1 });
    expect(folderOf("f1")?.conflicts).toEqual({ count: 1, sample, round: 1 });
  });

  it("the stale event does not reopen the conflicts dialog in the app", async () => {
    const { sessionId, backend } = await setupConnected();
    vi.spyOn(backend, "transferDecide").mockResolvedValue(undefined);
    const ev = (e: TransferEvent) => act(() => applyTransferEvent(sessionId, null, e));
    ev({ type: "started", transferId: "f1", name: "release", direction: "download", total: null, kind: "folder" });
    ev({ type: "conflicts", transferId: "f1", count: 1, sample, round: 0 });
    await waitFor(() => expect(useStore.getState().dialog?.kind).toBe("transferConflicts"));
    act(() => useStore.getState().closeDialog());
    await act(() => decideTransfer("f1", "replaceAll"));
    ev({ type: "conflicts", transferId: "f1", count: 2, sample, round: 0 });
    await settleApp(300);
    expect(useStore.getState().dialog).toBeNull();
  });
});

async function openPopover(user: Awaited<ReturnType<typeof setupConnected>>["user"]) {
  await user.click(await screen.findByRole("button", { name: /transfer/i }));
  return screen.findByRole("dialog", { name: "Transfers" });
}

const treeRow = (name: string | RegExp) => screen.getByRole("treeitem", { name });

async function menuItem(row: HTMLElement) {
  fireEvent.contextMenu(row, { clientX: 10, clientY: 10 });
  return screen.findByRole("menu");
}

describe("tree menu: Download folder…", () => {
  it("is offered for folders and symlinks to folders, not for files; off for lossy names and offline", async () => {
    const { user, backend, sessionId } = await setupConnected({ scenario: "folderTransfers" });
    let menu = await menuItem(treeRow("release"));
    expect(within(menu).getByRole("menuitem", { name: "Download folder…" }).getAttribute("aria-disabled")).toBeNull();
    await user.keyboard("{Escape}");

    menu = await menuItem(treeRow(/release-link/));
    expect(within(menu).getByRole("menuitem", { name: "Download folder…" }).getAttribute("aria-disabled")).toBeNull();
    await user.keyboard("{Escape}");

    menu = await menuItem(treeRow("deploy.sh"));
    expect(within(menu).queryByRole("menuitem", { name: "Download folder…" })).toBeNull();
    await user.keyboard("{Escape}");

    menu = await menuItem(treeRow(/-files/));
    expect(within(menu).getByRole("menuitem", { name: "Download folder…" }).getAttribute("aria-disabled")).toBe("true");
    await user.keyboard("{Escape}");

    act(() => backend.loseConnection(sessionId));
    menu = await menuItem(treeRow("release"));
    expect(within(menu).getByRole("menuitem", { name: "Download folder…" }).getAttribute("aria-disabled")).toBe("true");
  });
});

describe("folder download flow", () => {
  it("root exists -> Merge; Files already exist -> Replace older; done with a failure -> issues and Retry", async () => {
    const { user, backend } = await setupConnected({ scenario: "folderTransfers" });
    const start = vi.spyOn(backend, "transferDownloadFolder");
    const decide = vi.spyOn(backend, "transferDecide");
    const retry = vi.spyOn(backend, "transferRetry");
    await user.click(within(await menuItem(treeRow("release"))).getByRole("menuitem", { name: "Download folder…" }));
    expect(start).toHaveBeenCalledWith(expect.any(String), "/srv/app/release", "Download folder into", expect.any(Function));

    let dialog = await screen.findByRole("alertdialog", { name: "The folder already exists" });
    expect(within(dialog).getByRole("button", { name: "Keep both" })).toBeTruthy();
    await user.click(within(dialog).getByRole("button", { name: "Merge" }));
    expect(decide).toHaveBeenCalledWith(expect.any(String), "merge");

    dialog = await screen.findByRole("alertdialog", { name: "2 files already exist" });
    expect(dialog.textContent).toContain("bin/tools.sh");
    expect(dialog.textContent).toContain("docs/README.md");
    expect(dialog.textContent).toContain("On this computer:");
    expect(dialog.textContent).toContain("On the server:");
    await user.click(within(dialog).getByRole("button", { name: "Replace older" }));
    expect(decide).toHaveBeenLastCalledWith(expect.any(String), "replaceOlder");

    await waitFor(() => expect(rows()[0]).toMatchObject({ status: "done", retryable: true }));
    const popover = await openPopover(user);
    expect(within(popover).getByText(/8 files copied, 1 failed, 9 skipped/)).toBeTruthy();
    await user.click(within(popover).getByRole("button", { name: "10 problems" }));
    expect(within(popover).getByText("link-to-etc")).toBeTruthy();
    expect(within(popover).getAllByText("Skipped: symbolic links are not followed")).toHaveLength(2);
    expect(within(popover).getByText("data/locked.db")).toBeTruthy();
    expect(within(popover).getByText("Permission denied for /srv/app/release/data/locked.db.")).toBeTruthy();
    // Hostile names are escaped (the NUL as \x00, bidi as U+XXXX), never rendered raw.
    expect(popover.textContent).not.toContain("\u0000");

    await user.click(within(popover).getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(retry).toHaveBeenCalledWith(expect.any(String), false, expect.any(Function)));
    await waitFor(() => expect(rows().map((r) => r.status)).toEqual(["done"]));
    expect(rows()[0]?.folder?.summary).toMatchObject({ failed: 0, filesDone: 9 });
  });

  it("Replace all applies to every conflicting file", async () => {
    const { user, backend } = await setupConnected({ scenario: "folderTransfers" });
    const decide = vi.spyOn(backend, "transferDecide");
    await user.click(within(await menuItem(treeRow("release"))).getByRole("menuitem", { name: "Download folder…" }));
    await user.click(within(await screen.findByRole("alertdialog", { name: "The folder already exists" })).getByRole("button", { name: "Merge" }));
    const dialog = await screen.findByRole("alertdialog", { name: "2 files already exist" });
    await user.click(within(dialog).getByRole("button", { name: "Replace all" }));
    expect(decide).toHaveBeenLastCalledWith(expect.any(String), "replaceAll");
    await waitFor(() => expect(rows()[0]?.status).toBe("done"));
    expect(rows()[0]?.folder?.summary).toMatchObject({ filesDone: 9, skipped: { exists: 0 } });
  });

  it("a file in the way offers only Keep both / Cancel; Cancel cancels the job", async () => {
    const { user, backend } = await setupConnected({ scenario: "folderTransfers" });
    const decide = vi.spyOn(backend, "transferDecide");
    await user.click(within(await menuItem(treeRow("backup"))).getByRole("menuitem", { name: "Download folder…" }));
    const dialog = await screen.findByRole("alertdialog", { name: "The name is already taken" });
    expect(within(dialog).queryByRole("button", { name: "Merge" })).toBeNull();
    expect(within(dialog).getByRole("button", { name: "Keep both" })).toBeTruthy();
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(decide).toHaveBeenCalledWith(expect.any(String), "cancel");
    await waitFor(() => expect(rows()[0]?.status).toBe("cancelled"));
  });

  it("Keep both: the finished row says where the copy went", async () => {
    const { user } = await setupConnected({ scenario: "folderTransfers" });
    await user.click(within(await menuItem(treeRow("backup"))).getByRole("menuitem", { name: "Download folder…" }));
    const dialog = await screen.findByRole("alertdialog", { name: "The name is already taken" });
    await user.click(within(dialog).getByRole("button", { name: "Keep both" }));
    await waitFor(() => expect(rows()[0]?.status).toBe("done"));
    expect(rows()[0]?.folder?.summary?.localName).toBe("backup (2)");
    const popover = await openPopover(user);
    expect(within(popover).getByText(/Saved as the folder backup \(2\)/)).toBeTruthy();
  });

  it("large tree: closing the dialog leaves the job waiting; the popover reopens it; Copy proceeds", async () => {
    const { user, backend } = await setupConnected({ scenario: "folderTransfers" });
    const decide = vi.spyOn(backend, "transferDecide");
    await user.click(within(await menuItem(treeRow("huge-logs"))).getByRole("menuitem", { name: "Download folder…" }));
    let dialog = await screen.findByRole("alertdialog", { name: "Copy 20,000 files?" });
    expect(dialog.textContent).toContain("This folder has 20,000 files");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(decide).not.toHaveBeenCalled();

    const status = screen.getByRole("button", { name: /need/ });
    expect(status.textContent).toContain("1 transfer needs your decision");
    const popover = await openPopover(user);
    expect(within(popover).getByText("Waiting for your decision")).toBeTruthy();
    await user.click(within(popover).getByRole("button", { name: "Decide…" }));
    dialog = await screen.findByRole("alertdialog", { name: "Copy 20,000 files?" });
    await user.click(within(dialog).getByRole("button", { name: "Copy" }));
    expect(decide).toHaveBeenCalledWith(expect.any(String), "proceed");
    await waitFor(() => expect(rows()[0]?.status).toBe("done"), { timeout: 5000 });
  });

  it("low space: needed vs available, Copy anyway proceeds", async () => {
    const { user, backend } = await setupConnected({ scenario: "folderTransfers" });
    backend.localDisk.folders = {};
    backend.localDisk.freeBytes = 1024;
    const decide = vi.spyOn(backend, "transferDecide");
    await user.click(within(await menuItem(treeRow("backup"))).getByRole("menuitem", { name: "Download folder…" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Not enough free space" });
    expect(dialog.textContent).toMatch(/needs 2(\.0)? MiB, but only 1(\.0)? KiB is free/);
    await user.click(within(dialog).getByRole("button", { name: "Copy anyway" }));
    expect(decide).toHaveBeenCalledWith(expect.any(String), "proceed");
    await waitFor(() => expect(rows()[0]?.status).toBe("done"));
  });

  it("the decision dialog does not push over another dialog: the popover offers it", async () => {
    const { user, sessionId } = await setupConnected({ scenario: "folderTransfers" });
    act(() => useStore.getState().openDialog({ kind: "settings" }));
    act(() => {
      applyTransferEvent(sessionId, null, { type: "started", transferId: "f9", name: "x", direction: "download", total: null, kind: "folder" });
      applyTransferEvent(sessionId, null, { type: "awaitingDecision", transferId: "f9", request: { kind: "lowSpace", needed: 10, available: 1 } });
    });
    await settleApp(300);
    expect(useStore.getState().dialog?.kind).toBe("settings");
    act(() => useStore.getState().closeDialog());
    const popover = await openPopover(user);
    expect(within(popover).getByRole("button", { name: "Decide…" })).toBeTruthy();
  });
});

describe("folder row rendering", () => {
  it("scanning, then copying with the current file (escaped) and N files already exist", async () => {
    const { user, sessionId } = await setupConnected();
    const apply = (e: TransferEvent) => act(() => applyTransferEvent(sessionId, null, e));
    apply({ type: "started", transferId: "f1", name: `rel${RLO}ease`, direction: "download", total: null, kind: "folder" });
    apply({ type: "scanProgress", transferId: "f1", files: 42, dirs: 3, bytes: 100 });
    const popover = await openPopover(user);
    expect(within(popover).getByText("Scanning… 42 files found")).toBeTruthy();
    expect(popover.textContent).toContain("rel⟨U+202E⟩ease");

    apply({ type: "scanned", transferId: "f1", files: 10, dirs: 3, bytes: 2048, skipped: emptySkipCounts(), renamed: 0 });
    apply({ type: "progress", transferId: "f1", bytes: 1024, total: 2048, filesDone: 3, filesTotal: 10, current: `x${RLO}.txt` });
    expect(within(popover).getByText(/^3 of 10 files, 1(\.0)? KiB of 2(\.0)? KiB$/)).toBeTruthy();
    expect(popover.textContent).toContain("x⟨U+202E⟩.txt");
    expect(popover.textContent).not.toContain(RLO);

    act(() => useStore.getState().openDialog({ kind: "settings" })); // keep the conflicts dialog from opening
    apply({ type: "conflicts", transferId: "f1", count: 3, sample: [], round: 0 });
    expect(within(popover).getByText("3 files already exist")).toBeTruthy();
    expect(within(popover).getByRole("button", { name: "Choose…" })).toBeTruthy();
    expect(within(popover).getByRole("button", { name: /Cancel transfer of/ })).toBeTruthy();
    act(() => useStore.getState().closeDialog());

    // More issues than listed: a note with the rest.
    for (let i = 0; i < 2; i += 1) {
      apply({ type: "itemIssue", transferId: "f1", itemId: null, relPath: `l${i}`, issue: { kind: "skipped", reason: "special" } });
    }
    apply({ type: "progress", transferId: "f1", bytes: 1024, total: 2048, failed: 0, skipped: counts({ special: 1500 }) });
    await user.click(within(popover).getByRole("button", { name: "1,500 problems" }));
    expect(within(popover).getByText("1,498 more are not listed")).toBeTruthy();
  });

  it("a fatal failure shows the error and Retry when retryable", async () => {
    const { user, sessionId } = await setupConnected();
    act(() => {
      applyTransferEvent(sessionId, null, { type: "started", transferId: "f1", name: "big", direction: "download", total: null, kind: "folder" });
      applyTransferEvent(sessionId, null, { type: "failed", transferId: "f1", error: { code: "treeTooLarge", params: { limit: "entries", value: 200001 } }, retryable: false });
      applyTransferEvent(sessionId, null, { type: "started", transferId: "f2", name: "logs", direction: "download", total: null, kind: "folder" });
      applyTransferEvent(sessionId, null, { type: "failed", transferId: "f2", error: { code: "timeout", params: { op: "treeScan" } }, retryable: true });
    });
    const popover = await openPopover(user);
    expect(within(popover).getByText(/^The folder was not copied: it has too many entries \(200\D?001\)\. Nothing was written\.$/)).toBeTruthy();
    expect(within(popover).getByText("The operation timed out (reading the folder tree).")).toBeTruthy();
    expect(within(popover).getAllByRole("button", { name: "Retry" })).toHaveLength(1);
  });

  it("treeTooLarge names the limit with its value for every limit (memory in bytes)", async () => {
    const { errorMessage } = await import("../../i18n/errorMessage");
    const msg = (limit: "depth" | "entries" | "memory" | "directory", value: number) => errorMessage({ code: "treeTooLarge", params: { limit, value } });
    expect(msg("depth", 65)).toContain("(65 levels)");
    expect(msg("entries", 200001)).toMatch(/\(200\D?001\)/);
    expect(msg("memory", 67108864)).toMatch(/more than 64(\.0)? MiB of memory/);
    expect(msg("directory", 100000)).toMatch(/more than 100\D?000 entries/);
  });
});
