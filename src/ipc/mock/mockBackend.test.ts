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
