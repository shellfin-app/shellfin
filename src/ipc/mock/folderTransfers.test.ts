// MockBackend folder downloads model the backend contract (ARCHITECTURE §6.6 "Folders"): event order, skip reasons,
// limits, decisions, deferred conflicts with apply-to-all, item failures, retry of failed items only, cancel.
import { describe, expect, it } from "vitest";
import type { SessionEvent, TransferEvent } from "../types";
import type { FakeFs } from "./fakeFs";
import { treeLink } from "./fakeFs";
import { MockBackend, type MockBackendOptions } from "./mockBackend";

async function connect(options: MockBackendOptions = {}) {
  const backend = new MockBackend({ scenario: "folderTransfers", ...options });
  let resolveConnected: () => void = () => undefined;
  const connected = new Promise<void>((resolve) => {
    resolveConnected = resolve;
  });
  const sessionId = await backend.openSession({ target: { kind: "sshConfig", alias: "prod-web-01" }, probeOnly: false }, (e: SessionEvent) => {
    if (e.type === "state" && e.state.phase === "connected") resolveConnected();
  });
  await connected;
  return { backend, sessionId };
}

const fsOf = (backend: MockBackend) => (backend as unknown as { fs: FakeFs }).fs;

function recorder() {
  const events: TransferEvent[] = [];
  const until = async (pred: (e: TransferEvent) => boolean) => {
    for (let i = 0; i < 400 && !events.some(pred); i += 1) await new Promise((r) => setTimeout(r, 5));
    const hit = events.find(pred);
    if (!hit) throw new Error(`event not seen; got ${events.map((e) => e.type).join(",")}`);
    return hit;
  };
  return { events, on: (e: TransferEvent) => void events.push(e), until };
}

const of = <T extends TransferEvent["type"]>(events: TransferEvent[], type: T) =>
  events.filter((e): e is Extract<TransferEvent, { type: T }> => e.type === type);
const isFinal = (e: TransferEvent) => e.type === "done" || e.type === "failed" || e.type === "cancelled" || e.type === "skipped";

describe("MockBackend folder downloads", () => {
  it("release: scan skips, root exists -> merge, deferred conflicts -> Replace older, an item failure -> retry copies only it", async () => {
    const { backend, sessionId } = await connect();
    const r = recorder();
    const id = await backend.transferDownloadFolder(sessionId, "/srv/app/release", "t", r.on);
    expect(id).toMatch(/^transfer-/);
    await r.until((e) => e.type === "awaitingDecision");

    expect(r.events[0]).toMatchObject({ type: "started", kind: "folder", total: null, name: "release", direction: "download" });
    const reasons = of(r.events, "itemIssue").map((e) => (e.issue.kind === "skipped" ? `${e.relPath}:${e.issue.reason}` : ""));
    expect(reasons).toEqual(
      expect.arrayContaining([
        "link-to-etc:symlink",
        "loop:symlink",
        "queue.fifo:special",
        "docs/a_b.txt:nameCollision",
        "docs/readme.md:nameCollision",
      ]),
    );
    expect(reasons.filter((x) => x.endsWith(":lossyName"))).toHaveLength(2);
    expect(reasons.filter((x) => x.endsWith(":invalidName"))).toHaveLength(1);
    // Nothing below the lossy folder is reported or copied.
    expect(reasons.some((x) => x.includes("inner.txt"))).toBe(false);
    const scanned = of(r.events, "scanned")[0];
    expect(scanned).toMatchObject({ files: 10, skipped: { symlink: 2, special: 1, lossyName: 2, invalidName: 1, nameCollision: 2 } });
    expect(scanned?.renamed).toBeGreaterThanOrEqual(3);
    expect(of(r.events, "awaitingDecision")[0]?.request).toEqual({ kind: "rootExists", name: "release", isDir: true });

    await backend.transferDecide(id ?? "", "merge");
    const conflicts = await r.until((e) => e.type === "conflicts" && e.count === 2);
    expect(conflicts.type === "conflicts" && conflicts.sample.map((s) => s.relPath).sort()).toEqual(["bin/tools.sh", "docs/README.md"]);
    // The rest is copied; the job then waits for the conflicts (no final event yet).
    await new Promise((res) => setTimeout(res, 30));
    expect(r.events.some(isFinal)).toBe(false);
    const failedItem = of(r.events, "itemIssue").find((e) => e.issue.kind === "failed");
    expect(failedItem).toMatchObject({ relPath: "data/locked.db", issue: { kind: "failed", retryable: true, error: { code: "permissionDenied" } } });

    // A decision that does not fit is ignored; Replace older answers all conflicts.
    await backend.transferDecide(id ?? "", "proceed");
    await backend.transferDecide(id ?? "", "replaceOlder");
    const done = await r.until((e) => e.type === "done");
    // tools.sh (older locally) replaced, README.md (newer locally) skipped as `exists`.
    expect(done).toMatchObject({ type: "done", entry: null, summary: { filesDone: 8, failed: 1, retryable: true, skipped: { exists: 1 } } });
    expect(of(r.events, "itemIssue").some((e) => e.relPath === "docs/README.md" && e.issue.kind === "skipped" && e.issue.reason === "exists")).toBe(true);
    await expect(backend.transferDecide(id ?? "", "replaceAll")).rejects.toMatchObject({ error: { code: "notFound", params: { what: "transfer" } } });

    const again = recorder();
    const retryId = await backend.transferRetry(id ?? "", false, again.on);
    expect(retryId).not.toBe(id);
    const retried = await again.until(isFinal);
    // Retry: `started.total` = the remaining bytes (locked.db), no scan, a `progress` right away.
    expect(again.events[0]).toMatchObject({ type: "started", kind: "folder", total: 1024 * 1024 });
    expect(again.events[1]).toMatchObject({ type: "progress", filesDone: 8, filesTotal: 10 });
    expect(of(again.events, "scanned")).toHaveLength(0);
    expect(retried).toMatchObject({ type: "done", summary: { filesDone: 9, failed: 0, retryable: false } });
    expect(of(again.events, "itemIssue")).toHaveLength(0); // only the failed item ran again
    await expect(backend.transferRetry(id ?? "", false, () => undefined)).rejects.toMatchObject({ error: { code: "notFound" } });
  });

  it("huge-logs: the soft threshold asks first; cancel ends the job, proceed copies all 20 000 files in batched progress", async () => {
    const { backend, sessionId } = await connect();
    const r = recorder();
    const id = (await backend.transferDownloadFolder(sessionId, "/srv/app/huge-logs", "t", r.on)) ?? "";
    expect(await r.until((e) => e.type === "awaitingDecision")).toMatchObject({ request: { kind: "largeTree", files: 20_000, bytes: 20_000 * 512 } });
    expect(of(r.events, "scanProgress").length).toBeGreaterThan(0);
    await backend.transferDecide(id, "cancel");
    expect(await r.until(isFinal)).toMatchObject({ type: "cancelled" });

    const p = recorder();
    const id2 = (await backend.transferDownloadFolder(sessionId, "/srv/app/huge-logs", "t", p.on)) ?? "";
    await p.until((e) => e.type === "awaitingDecision");
    await backend.transferDecide(id2, "proceed");
    expect(await p.until(isFinal)).toMatchObject({ type: "done", summary: { filesDone: 20_000 } });
    expect(of(p.events, "progress").length).toBeLessThan(40);
  });

  it("backup: a file is in the way locally -> only Keep both / Cancel; Merge is ignored", async () => {
    const { backend, sessionId } = await connect();
    const r = recorder();
    const id = (await backend.transferDownloadFolder(sessionId, "/srv/app/backup", "t", r.on)) ?? "";
    expect(await r.until((e) => e.type === "awaitingDecision")).toMatchObject({ request: { kind: "rootExists", isDir: false } });
    await backend.transferDecide(id, "merge");
    await new Promise((res) => setTimeout(res, 20));
    expect(r.events.some(isFinal)).toBe(false);
    await backend.transferDecide(id, "keepBoth");
    expect(await r.until(isFinal)).toMatchObject({ type: "done" });
    expect(backend.localDisk.folders["backup (2)"]).toMatchObject({ isDir: true });
  });

  it("onConflict skip: an existing destination folder ends the job as skipped", async () => {
    const { backend, sessionId } = await connect();
    const settings = await backend.settingsGet();
    await backend.settingsUpdate({ ...settings, transfers: { ...settings.transfers, onConflict: "skip" } });
    const r = recorder();
    await backend.transferDownloadFolder(sessionId, "/srv/app/release", "t", r.on);
    expect(await r.until(isFinal)).toMatchObject({ type: "skipped" });
  });

  it("rejects files and lossy paths before the picker, resolves a symlink root once, null when the picker is cancelled", async () => {
    const { backend, sessionId } = await connect();
    await expect(backend.transferDownloadFolder(sessionId, "/srv/app/README.md", "t", () => undefined)).rejects.toMatchObject({
      error: { code: "notADirectory" },
    });
    await expect(backend.transferDownloadFolder(sessionId, "/srv/app/caf�-files", "t", () => undefined)).rejects.toMatchObject({
      error: { code: "invalidArgument" },
    });
    backend.localDisk.folders = {};
    const r = recorder();
    await backend.transferDownloadFolder(sessionId, "/srv/app/release-link", "t", r.on);
    expect(await r.until(isFinal)).toMatchObject({ type: "done" });
    expect(r.events[0]).toMatchObject({ name: "release-link" });

    const cancelled = await connect({ scenario: "folderTransfers" });
    Object.assign(cancelled.backend, { hooks: { ...(cancelled.backend as unknown as { hooks: object }).hooks, folderPickerCancelled: true } });
    await expect(cancelled.backend.transferDownloadFolder(cancelled.sessionId, "/srv/app/release", "t", () => undefined)).resolves.toBeNull();
  });

  it("hard limits refuse the scan before anything is written: a truncated folder, too deep a tree", async () => {
    const { backend, sessionId } = await connect({ scenario: "largeFile" });
    const r = recorder();
    await backend.transferDownloadFolder(sessionId, "/srv/app/many", "t", r.on);
    expect(await r.until(isFinal)).toMatchObject({ type: "failed", retryable: false, error: { code: "treeTooLarge", params: { limit: "directory" } } });

    let path = "/srv/app/deep";
    fsOf(backend).addTree(path, {});
    for (let i = 0; i < 70; i += 1) {
      path = `${path}/d${i}`;
      fsOf(backend).addTree(path, {});
    }
    const d = recorder();
    await backend.transferDownloadFolder(sessionId, "/srv/app/deep", "t", d.on);
    expect(await d.until(isFinal)).toMatchObject({ type: "failed", error: { code: "treeTooLarge", params: { limit: "depth", value: 65 } } });
    expect(of(d.events, "scanned")).toHaveLength(0);
  });

  it("reports at most 1 000 issues one by one; the counters count all of them", async () => {
    const { backend, sessionId } = await connect();
    const spec: Record<string, ReturnType<typeof treeLink> | number> = { "keep.txt": 1 };
    for (let i = 0; i < 1500; i += 1) spec[`l${i}`] = treeLink("/etc");
    fsOf(backend).addTree("/srv/app/links", spec);
    const r = recorder();
    await backend.transferDownloadFolder(sessionId, "/srv/app/links", "t", r.on);
    const done = await r.until(isFinal);
    expect(of(r.events, "itemIssue")).toHaveLength(1000);
    expect(of(r.events, "scanned")[0]?.skipped.symlink).toBe(1500);
    expect(done).toMatchObject({ type: "done", summary: { filesDone: 1, skipped: { symlink: 1500 } } });
  });

  it("cancel stops a running copy; a lost connection fails it retryably", async () => {
    const { backend, sessionId } = await connect({ transferStepMs: 20 });
    backend.localDisk.folders = {};
    const r = recorder();
    const id = (await backend.transferDownloadFolder(sessionId, "/srv/app/release", "t", r.on)) ?? "";
    await r.until((e) => e.type === "progress");
    await backend.transferCancel(id);
    expect(await r.until(isFinal)).toMatchObject({ type: "cancelled" });
    await expect(backend.transferDecide(id, "proceed")).rejects.toMatchObject({ error: { code: "notFound" } });

    const lost = recorder();
    await backend.transferDownloadFolder(sessionId, "/srv/app/release", "t", lost.on);
    await lost.until((e) => e.type === "progress");
    backend.loseConnection(sessionId);
    expect(await lost.until(isFinal)).toMatchObject({ type: "failed", retryable: true, error: { code: "disconnected" } });
  });
});
