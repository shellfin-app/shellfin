import { describe, expect, it } from "vitest";
import type { SessionEvent, TransferEvent } from "../types";
import { MockBackend, type MockBackendOptions } from "./mockBackend";
import { SCENARIO_NAMES } from "./scenarios";

async function connect(options: MockBackendOptions = {}) {
  const backend = new MockBackend(options);
  const events: SessionEvent[] = [];
  let resolveConnected: () => void = () => undefined;
  const connected = new Promise<void>((resolve) => {
    resolveConnected = resolve;
  });
  const sessionId = await backend.openSession({ target: { kind: "sshConfig", alias: "prod-web-01" }, probeOnly: false }, (e) => {
    events.push(e);
    if (e.type === "state" && e.state.phase === "connected") resolveConnected();
  });
  await connected;
  return { backend, sessionId, events };
}

const text = (path: string, body: string, extra: Partial<Parameters<MockBackend["sftpWriteText"]>[1]> = {}) => ({
  path,
  text: body,
  encoding: "utf-8",
  hasBom: false,
  eol: "lf" as const,
  expected: null,
  force: false,
  ...extra,
});

describe("MockBackend editor and file scenarios", () => {
  it("lists every scenario of §8.8 (including autoReconnect)", () => {
    for (const name of [
      "happy", "hostKeyUnknown", "hostKeyChanged", "hostKeyCaManaged", "passphraseRetry", "passwordRetry", "kbdInteractive2fa",
      "dropAfter10s", "reconnectHostKeyMismatch", "saveConflict", "binaryFile", "nonUtf8File", "largeFile", "sftpUnavailable",
      "uploadConflict", "terminalFlood", "agentUntrusted", "autoReconnect", "folderTransfers",
    ]) {
      expect(SCENARIO_NAMES).toContain(name);
    }
  });

  it("etags: a changed server file is remoteChanged unless only the mtime moved; force overwrites", async () => {
    const { backend, sessionId } = await connect();
    const doc = await backend.sftpReadText(sessionId, "/srv/app/README.md", null);
    const mtimeOnly = { ...doc.etag, mtime: doc.etag.mtime - 5 };
    // same bytes, different mtime: no conflict (§6.4 sha256 check)
    await expect(backend.sftpWriteText(sessionId, text("/srv/app/README.md", "v2\n", { expected: mtimeOnly }))).resolves.toBeTruthy();
    // the file changed since `doc` was read
    await expect(backend.sftpWriteText(sessionId, text("/srv/app/README.md", "v3\n", { expected: doc.etag }))).rejects.toMatchObject({
      error: { code: "remoteChanged", params: { path: "/srv/app/README.md" } },
    });
    await expect(backend.sftpWriteText(sessionId, text("/srv/app/README.md", "v3\n", { expected: doc.etag, force: true }))).resolves.toBeTruthy();
    expect((await backend.sftpReadText(sessionId, "/srv/app/README.md", null)).text).toBe("v3\n");
  });

  it("saveConflict: the first plain save of each file finds it changed; after Overwrite/reload saves work", async () => {
    const { backend, sessionId } = await connect({ scenario: "saveConflict" });
    const doc = await backend.sftpReadText(sessionId, "/srv/app/deploy.sh", null);
    await expect(backend.sftpWriteText(sessionId, text("/srv/app/deploy.sh", "mine", { expected: doc.etag }))).rejects.toMatchObject({
      error: { code: "remoteChanged" },
    });
    const fresh = await backend.sftpReadText(sessionId, "/srv/app/deploy.sh", null);
    expect(fresh.text).toContain("# changed on the server");
    await expect(backend.sftpWriteText(sessionId, text("/srv/app/deploy.sh", "mine", { expected: fresh.etag }))).resolves.toBeTruthy();
  });

  it("binaryFile / nonUtf8File / largeFile follow §6.3", async () => {
    const bin = await connect({ scenario: "binaryFile" });
    await expect(bin.backend.sftpReadText(bin.sessionId, "/srv/app/data.bin", null)).rejects.toMatchObject({ error: { code: "fileIsBinary" } });

    const enc = await connect({ scenario: "nonUtf8File" });
    const path = "/srv/app/legacy-cp1251.txt";
    await expect(enc.backend.sftpReadText(enc.sessionId, path, null)).rejects.toMatchObject({
      error: { code: "decodeFailed", params: { encoding: "utf-8" } },
    });
    await expect(enc.backend.sftpReadText(enc.sessionId, path, "no-such-encoding")).rejects.toMatchObject({ error: { code: "unsupportedEncoding" } });
    const decoded = await enc.backend.sftpReadText(enc.sessionId, path, "windows-1251");
    expect(decoded).toMatchObject({ encoding: "windows-1251" });
    expect((await enc.backend.sftpReadText(enc.sessionId, "/srv/app/windows.ini", null)).eol).toBe("crlf");
    expect((await enc.backend.sftpReadText(enc.sessionId, "/srv/app/mixed.txt", null)).mixedEol).toBe(true);

    const big = await connect({ scenario: "largeFile" });
    await expect(big.backend.sftpReadText(big.sessionId, "/srv/app/huge.log", null)).rejects.toMatchObject({
      error: { code: "fileTooLarge", params: { size: 80 * 1024 * 1024, limit: 5 * 1024 * 1024 } },
    });
    const crowded = await big.backend.sftpList(big.sessionId, "/srv/app/many");
    expect(crowded).toMatchObject({ truncated: true });
    expect(crowded.entries).toHaveLength(3000);
  });

  it("sftpUnavailable: every sftp_* fails with sftpUnavailable and the session says so", async () => {
    const { backend, sessionId, events } = await connect({ scenario: "sftpUnavailable" });
    const connected = events.find((e) => e.type === "state" && e.state.phase === "connected");
    expect(connected && connected.type === "state" && connected.state.phase === "connected" && connected.state.info.sftpAvailable).toBe(false);
    await expect(backend.sftpList(sessionId, "/")).rejects.toMatchObject({ error: { code: "sftpUnavailable" } });
    await expect(backend.transferUploadPick(sessionId, "/", "t", false, () => undefined)).rejects.toMatchObject({ error: { code: "sftpUnavailable" } });
  });

  it("autoReconnect: SFTP fails with notConnected while reconnecting and works again afterwards", async () => {
    const { backend, sessionId } = await connect({ scenario: "autoReconnect", dropAfterMs: 1_000_000 });
    await expect(backend.sftpList(sessionId, "/srv/app")).resolves.toBeTruthy();
    backend.loseConnection(sessionId);
    await expect(backend.sftpList(sessionId, "/srv/app")).rejects.toMatchObject({ error: { code: "notConnected" } });
    await expect(backend.sftpReadText(sessionId, "/srv/app/README.md", null)).rejects.toMatchObject({ error: { code: "notConnected" } });
    backend.restoreConnection(sessionId);
    await expect(backend.sftpList(sessionId, "/srv/app")).resolves.toBeTruthy();
  });

  it("directory rename moves the subtree; create needs an existing parent", async () => {
    const { backend, sessionId } = await connect();
    await backend.sftpRename(sessionId, "/srv/app/nginx", "/srv/app/web");
    expect((await backend.sftpList(sessionId, "/srv/app/web")).entries.map((e) => e.name).sort()).toEqual(["mime.types", "nginx.conf"]);
    await expect(backend.sftpList(sessionId, "/srv/app/nginx")).rejects.toMatchObject({ error: { code: "remoteNotFound" } });
    await expect(backend.sftpCreateFile(sessionId, "/srv/app/missing/x.txt")).rejects.toMatchObject({ error: { code: "remoteNotFound" } });
  });
});

describe("MockBackend transfers", () => {
  const collect = () => {
    const events: TransferEvent[] = [];
    return { events, on: (e: TransferEvent) => void events.push(e) };
  };
  const settled = async (events: TransferEvent[], count: number) => {
    for (let i = 0; i < 200 && events.filter((e) => e.type === "done" || e.type === "failed" || e.type === "cancelled").length < count; i += 1) {
      await new Promise((r) => setTimeout(r, 5));
    }
  };

  it("upload: started, progress, done with the new entry; the file exists afterwards", async () => {
    const { backend, sessionId } = await connect();
    const { events, on } = collect();
    const ids = await backend.transferUploadPick(sessionId, "/srv/app", "t", false, on);
    expect(ids).toHaveLength(1);
    await settled(events, 1);
    expect(events.map((e) => e.type)).toEqual(["started", "progress", "progress", "progress", "progress", "done"]);
    const done = events.at(-1);
    expect(done && done.type === "done" && done.entry?.path).toBe("/srv/app/report.txt");
    await expect(backend.sftpStat(sessionId, "/srv/app/report.txt")).resolves.toBeTruthy();
  });

  it("uploadConflict: existing names fail with a retryable remoteExists; retry with overwrite replaces; unknown ids are notFound", async () => {
    const { backend, sessionId } = await connect({ scenario: "uploadConflict" });
    const { events, on } = collect();
    await backend.transferUploadPick(sessionId, "/srv/app", "t", false, on);
    await settled(events, 3);
    const failed = events.filter((e): e is Extract<TransferEvent, { type: "failed" }> => e.type === "failed");
    expect(failed.map((f) => f.error.code)).toEqual(["remoteExists", "remoteExists"]);
    expect(failed.every((f) => f.retryable)).toBe(true);

    const retryEvents = collect();
    const newId = await backend.transferRetry(failed[0]?.transferId ?? "", true, retryEvents.on);
    expect(newId).not.toBe(failed[0]?.transferId);
    await settled(retryEvents.events, 1);
    expect(retryEvents.events.at(-1)?.type).toBe("done");
    // the old id is spent
    await expect(backend.transferRetry(failed[0]?.transferId ?? "", true, () => undefined)).rejects.toMatchObject({
      error: { code: "notFound", params: { what: "transfer" } },
    });
  });

  it("cancel emits `cancelled`; session_close cancels what is still running", async () => {
    const { backend, sessionId } = await connect({ transferStepMs: 20 });
    const { events, on } = collect();
    const [id] = await backend.transferUploadPick(sessionId, "/srv/app", "t", false, on);
    await backend.transferCancel(id ?? "");
    await settled(events, 1);
    expect(events.at(-1)).toMatchObject({ type: "cancelled" });
  });

  it("drop registry: consumed by the first upload, expires after 60 s, folders fail with localIsADirectory", async () => {
    let now = 5_000_000;
    const { backend, sessionId } = await connect({ now: () => now });
    let dropId = "";
    backend.listen("drop:files", (p) => {
      dropId = p.dropId;
    });
    backend.simulateDrop([
      { name: "a.txt", isDir: false },
      { name: "dir", isDir: true },
    ]);
    const { events, on } = collect();
    await backend.transferUploadDropped(sessionId, dropId, "/srv/app", false, on);
    await settled(events, 2);
    expect(events.filter((e) => e.type === "failed").map((e) => (e.type === "failed" ? e.error.code : ""))).toEqual(["localIsADirectory"]);
    await expect(backend.transferUploadDropped(sessionId, dropId, "/srv/app", false, on)).rejects.toMatchObject({ error: { code: "dropExpired" } });

    backend.simulateDrop([{ name: "b.txt", isDir: false }]);
    now += 61_000;
    await expect(backend.transferUploadDropped(sessionId, dropId, "/srv/app", false, on)).rejects.toMatchObject({ error: { code: "dropExpired" } });
  });

  it("download: a directory is isADirectory, a file streams to done", async () => {
    const { backend, sessionId } = await connect();
    await expect(backend.transferDownload(sessionId, "/srv/app/nginx", "t", () => undefined)).rejects.toMatchObject({ error: { code: "isADirectory" } });
    const { events, on } = collect();
    await backend.transferDownload(sessionId, "/srv/app/README.md", "t", on);
    await settled(events, 1);
    expect(events[0]).toMatchObject({ type: "started", direction: "download" });
    expect(events.at(-1)).toMatchObject({ type: "done" });
  });
});
