import { describe, expect, it, vi } from "vitest";
import { isRawBytes, type Backend } from "../backend";
import { IpcError } from "../call";
import type { SessionEvent } from "../types";
import { MockBackend } from "./mockBackend";
import { SCENARIO_NAMES } from "./scenarios";

async function connect(backend: Backend, alias = "prod-web-01") {
  const events: SessionEvent[] = [];
  const id = await backend.openSession({ target: { kind: "sshConfig", alias }, probeOnly: false }, (e) => events.push(e));
  // Wait for the script to reach a resting point (connected, failed, or waiting on a prompt), not a fixed time.
  await vi.waitFor(
    () => {
      const resting = events.some(
        (e) => e.type === "prompt" || (e.type === "state" && (e.state.phase === "connected" || e.state.phase === "disconnected")),
      );
      if (!resting) throw new Error("session script has not settled");
    },
    { timeout: 5000, interval: 5 },
  );
  return { id, events };
}

describe("MockBackend", () => {
  it("implements the whole Backend interface (contract)", () => {
    const backend: Backend = new MockBackend();
    expect(typeof backend.appFrontendReady).toBe("function");
  });

  it("bootstraps with ssh-config hosts, bookmarks and recents from the mockups", async () => {
    const b = await new MockBackend().appFrontendReady();
    expect(b.sshConfig.hosts.map((h) => h.alias)).toEqual([
      "prod-web-01", "prod-web-02", "staging-db", "bastion", "gitlab-runner",
    ]);
    expect(b.bookmarks.bookmarks.map((x) => x.name).sort()).toEqual(["nas", "pi-home", "vps-hetzner"]);
    expect(b.recents).toHaveLength(3);
  });

  it("happy scenario: connecting -> authenticating -> connected with SFTP info", async () => {
    const backend = new MockBackend();
    const { events } = await connect(backend);
    const phases = events.flatMap((e) => (e.type === "state" ? [e.state.phase] : []));
    expect(phases).toEqual(["connecting", "authenticating", "connected"]);
    const connected = events.find((e) => e.type === "state" && e.state.phase === "connected");
    expect(connected && connected.type === "state" && connected.state.phase === "connected" && connected.state.info.homeDir).toBe(
      "/home/deploy",
    );
  });

  it("serves the fake file tree and file contents", async () => {
    const backend = new MockBackend();
    const { id } = await connect(backend);
    const listing = await backend.sftpList(id, "/srv/app");
    expect(listing.entries.map((e) => e.name).sort()).toEqual(
      [".env.production", "README.md", "certs", "deploy.sh", "docker-compose.yml", "logs", "nginx"].sort(),
    );
    const doc = await backend.sftpReadText(id, "/srv/app/docker-compose.yml", null);
    expect(doc.text).toContain("services:");
    await expect(backend.sftpList(id, "/nope")).rejects.toBeInstanceOf(IpcError);
  });

  it("fake terminal prints a prompt and answers a command", async () => {
    const backend = new MockBackend();
    const { id } = await connect(backend);
    const chunks: string[] = [];
    const termId = await backend.openTerminal(id, { cols: 80, rows: 24, pixelWidth: 0, pixelHeight: 0 }, null, (m) => {
      if (isRawBytes(m)) chunks.push(new TextDecoder().decode(m));
    });
    await new Promise((r) => setTimeout(r, 5));
    await backend.termWrite(termId, Array.from(new TextEncoder().encode("whoami\r")));
    const out = chunks.join("");
    expect(out).toContain("deploy@prod-web-01");
    expect(out).toContain("whoami");
    expect(out).toMatch(/\r\ndeploy\r\n/);
  });

  it("term_write takes a Uint8Array too and refuses payloads over 1 MiB", async () => {
    const backend = new MockBackend();
    const { id } = await connect(backend);
    const chunks: string[] = [];
    const termId = await backend.openTerminal(id, { cols: 80, rows: 24, pixelWidth: 0, pixelHeight: 0 }, null, (m) => {
      if (isRawBytes(m)) chunks.push(new TextDecoder().decode(m));
    });
    await new Promise((r) => setTimeout(r, 5));
    await backend.termWrite(termId, new TextEncoder().encode("whoami\r"));
    expect(chunks.join("")).toMatch(/\r\ndeploy\r\n/);
    await expect(backend.termWrite(termId, new Uint8Array(1024 * 1024 + 1))).rejects.toMatchObject({
      error: { code: "invalidArgument", params: { field: "data" } },
    });
    await expect(backend.termWrite("nope", [1])).rejects.toMatchObject({ error: { code: "terminalNotFound" } });
  });

  it("sftpUnavailable scenario fails sftp_* with sftpUnavailable", async () => {
    const backend = new MockBackend({ scenario: "sftpUnavailable" });
    const { id } = await connect(backend);
    await expect(backend.sftpList(id, "/")).rejects.toMatchObject({ error: { code: "sftpUnavailable" } });
  });

  it("every v1 scenario name resolves (unimplemented ones fall back to happy)", async () => {
    for (const name of SCENARIO_NAMES) {
      const backend = new MockBackend({ scenario: name });
      const { events } = await connect(backend);
      expect(events.length, name).toBeGreaterThan(0);
    }
  });

  describe("bookmark import (open / apply / cancel)", () => {
    it("open previews without writing; apply consumes the token; a second apply is a token error", async () => {
      const backend = new MockBackend({ scenario: "importStartupCommands" });
      const before = (await backend.bookmarksGet()).bookmarks.length;
      const preview = await backend.bookmarksImportOpen("t");
      expect(preview).toMatchObject({ picked: true, bookmarks: 4, conflicts: 1 });
      expect(preview.invalid).toHaveLength(1);
      expect(preview.startupCommands.map((c) => [c.name === "pi-home", c.exists])).toEqual([
        [false, false],
        [true, true],
        [false, false],
      ]);
      expect((await backend.bookmarksGet()).bookmarks.length).toBe(before);
      const token = preview.token ?? "";
      await backend.bookmarksImportApply(token, "skip", false);
      await expect(backend.bookmarksImportApply(token, "skip", false)).rejects.toMatchObject({
        error: { code: "invalidArgument", params: { field: "token" } },
      });
    });

    it("keep=false strips the commands and names them; keep=true imports the valid ones", async () => {
      const backend = new MockBackend();
      backend.importFile = {
        bookmarks: [
          { name: "a", host: "a.example.com", startupCommand: "uptime" },
          { name: "b", host: "b.example.com", startupCommand: "bad\ncommand" },
        ],
      };
      let preview = await backend.bookmarksImportOpen("t");
      expect(preview.startupCommands.map((c) => c.name)).toEqual(["a"]); // an invalid command is not offered
      let result = await backend.bookmarksImportApply(preview.token ?? "", "skip", false);
      expect(result.startupCommandsStripped).toEqual(["a", "b"]);
      expect((await backend.bookmarksGet()).bookmarks.find((x) => x.name === "a")?.startupCommand).toBeNull();

      backend.importFile = { bookmarks: [{ name: "c", host: "c.example.com", startupCommand: "uptime" }] };
      preview = await backend.bookmarksImportOpen("t");
      result = await backend.bookmarksImportApply(preview.token ?? "", "skip", true);
      expect(result.startupCommandsStripped).toEqual([]);
      expect((await backend.bookmarksGet()).bookmarks.find((x) => x.name === "c")?.startupCommand).toBe("uptime");
    });

    it("replace never overwrites a bookmark this import added (a later same-name entry is skipped, like the backend)", async () => {
      const backend = new MockBackend();
      backend.importFile = {
        bookmarks: [
          { name: "dupe", host: "a.example.com", startupCommand: "first" },
          { name: "DUPE", host: "b.example.com", startupCommand: "second" },
        ],
      };
      const preview = await backend.bookmarksImportOpen("t");
      const result = await backend.bookmarksImportApply(preview.token ?? "", "replace", true);
      expect(result).toMatchObject({ added: 1, replaced: 0, skipped: 1 });
      const dupe = (await backend.bookmarksGet()).bookmarks.filter((x) => x.name.toLowerCase() === "dupe");
      expect(dupe.map((x) => [x.host, x.startupCommand])).toEqual([["a.example.com", "first"]]);
    });

    it("a wrong token still consumes the pending import; cancel is idempotent and token-bound", async () => {
      const backend = new MockBackend();
      const first = await backend.bookmarksImportOpen("t");
      await expect(backend.bookmarksImportApply("nope", "skip", false)).rejects.toMatchObject({ error: { code: "invalidArgument" } });
      await expect(backend.bookmarksImportApply(first.token ?? "", "skip", false)).rejects.toMatchObject({ error: { code: "invalidArgument" } });

      const second = await backend.bookmarksImportOpen("t");
      await backend.bookmarksImportCancel("other");
      await backend.bookmarksImportCancel(second.token ?? "");
      await backend.bookmarksImportCancel(second.token ?? "");
      await expect(backend.bookmarksImportApply(second.token ?? "", "skip", false)).rejects.toMatchObject({ error: { code: "invalidArgument" } });
    });

    it("open refuses a read-only store before the dialog, leaving nothing pending", async () => {
      const backend = new MockBackend();
      const first = await backend.bookmarksImportOpen("t");
      (await backend.bookmarksGet()).readOnly = true;
      await expect(backend.bookmarksImportOpen("t")).rejects.toMatchObject({ error: { code: "storageReadOnly" } });
      await expect(backend.bookmarksImportApply(first.token ?? "", "skip", false)).rejects.toMatchObject({ error: { code: "invalidArgument" } });
    });

    it("a new open replaces the pending import; a cancelled dialog has no token", async () => {
      const backend = new MockBackend();
      const first = await backend.bookmarksImportOpen("t");
      const second = await backend.bookmarksImportOpen("t");
      expect(second.token).not.toBe(first.token);
      await expect(backend.bookmarksImportApply(first.token ?? "", "skip", false)).rejects.toMatchObject({ error: { code: "invalidArgument" } });
      backend.importFile = null;
      expect(await backend.bookmarksImportOpen("t")).toMatchObject({ picked: false, token: null });
    });
  });

  it("session_close emits recents:changed", async () => {
    const backend = new MockBackend();
    const { id } = await connect(backend);
    let recents = 0;
    await backend.listen("recents:changed", (r) => {
      recents = r.length;
    });
    await backend.sessionClose(id, "/srv/app");
    expect(recents).toBeGreaterThan(0);
  });
});
