import { beforeEach, describe, expect, it } from "vitest";
import type { RecentTarget, SessionInfo } from "../ipc/types";
import { makeBootstrap } from "../ipc/mock/fixtures";
import { resetStore, useStore } from "./store";

const target: RecentTarget = { kind: "sshConfig", alias: "prod-web-01" };

function info(sessionId: string): SessionInfo {
  return {
    sessionId,
    label: "prod-web-01",
    hops: [],
    auth: { method: "publickey", keyType: "ed25519", keySource: "~/.ssh/id_ed25519" },
    sftpAvailable: true,
    homeDir: "/home/deploy",
    startDir: "/srv/app",
    remotePathStyle: "posix",
    remoteIdentity: null,
    limitOverrides: { uploadKibps: null, downloadKibps: null },
      color: null,
  };
}

describe("root store", () => {
  beforeEach(() => resetStore());

  it("starts empty and not ready", () => {
    const s = useStore.getState();
    expect(s.ready).toBe(false);
    expect(s.sessionOrder).toEqual([]);
    expect(s.activeSessionId).toBeNull();
  });

  it("applies the bootstrap across slices", () => {
    useStore.getState().setBootstrap(makeBootstrap(Date.now()));
    const s = useStore.getState();
    expect(s.ready).toBe(true);
    expect(s.settings?.version).toBe(1);
    expect(s.sshConfig?.hosts.length).toBe(5);
    expect(s.bookmarks?.bookmarks.length).toBe(3);
    expect(s.recents.length).toBe(3);
  });

  it("walks a session through its states and keeps info while reconnecting", () => {
    const { addSession, applySessionEvent } = useStore.getState();
    addSession("s1", target, "prod-web-01");
    expect(useStore.getState().sessionsById["s1"]?.state.phase).toBe("resolving");
    expect(useStore.getState().activeSessionId).toBe("s1");

    applySessionEvent("s1", { type: "state", state: { phase: "connected", info: info("s1") } });
    expect(useStore.getState().sessionsById["s1"]?.state.phase).toBe("connected");
    expect(useStore.getState().sessionsById["s1"]?.info?.startDir).toBe("/srv/app");

    applySessionEvent("s1", {
      type: "state",
      state: {
        phase: "reconnecting",
        stage: "waiting",
        hop: null,
        attempt: 1,
        maxAttempts: 5,
        nextAttemptAtMs: Date.now() + 1000,
        cause: { code: "disconnected", params: { reason: "keepaliveTimeout", message: null } },
      },
    });
    const entry = useStore.getState().sessionsById["s1"];
    expect(entry?.state.phase).toBe("reconnecting");
    expect(entry?.info).toBeDefined();

    applySessionEvent("s1", { type: "stats", txBytesPerSec: 1, rxBytesPerSec: 2 });
    expect(useStore.getState().sessionStats["s1"]).toEqual({ txBytesPerSec: 1, rxBytesPerSec: 2 });
  });

  it("keeps one prompt per session and clears it on promptClosed", () => {
    const { addSession, applySessionEvent } = useStore.getState();
    addSession("s1", target, "x");
    applySessionEvent("s1", {
      type: "prompt",
      prompt: {
        kind: "password", id: "p1", hop: { index: 0, label: "x", user: "u", host: "h", port: 22 }, user: "u", host: "h",
        attemptsLeft: 3, lastError: null, canRemember: true, savedPasswordAvailable: false, serverLabel: null,
      },
    });
    expect(useStore.getState().prompts["s1"]?.id).toBe("p1");
    applySessionEvent("s1", { type: "promptClosed", promptId: "p1", reason: "answered" });
    expect(useStore.getState().prompts["s1"]).toBeUndefined();
  });

  it("removing a session drops its workspace/buffers/terminals and activates a neighbour", () => {
    const s = useStore.getState();
    s.addSession("a", target, "a");
    s.addSession("b", { kind: "sshConfig", alias: "b" }, "b");
    s.ensureWorkspace("b");
    s.addBuffer({
      id: "b:/x", sessionId: "b", path: "/x", load: "ready", dirty: true, encoding: "utf-8", eol: "lf", etag: null,
      savedAtMs: null,
    });
    s.setCursor("b:/x", { line: 3, col: 4 });
    s.openEditorTab("b", "b:/x");
    s.addTerminal({ termId: "t1", sessionId: "b", title: "bash", name: null, cwd: null, status: "running", exitCode: null });
    s.addTerminalTab("b", "t1");

    useStore.getState().removeSession("b");
    const after = useStore.getState();
    expect(after.sessionOrder).toEqual(["a"]);
    expect(after.activeSessionId).toBe("a");
    expect(after.workspaces["b"]).toBeUndefined();
    expect(after.buffers["b:/x"]).toBeUndefined();
    expect(after.cursors["b:/x"]).toBeUndefined();
    expect(after.terminals["t1"]).toBeUndefined();
  });

  it("Removing a session closes the prompt, rename, drop target and dialogs that pointed at it", () => {
    const s = useStore.getState();
    s.addSession("a", target, "a");
    s.addSession("b", { kind: "sshConfig", alias: "b" }, "b");
    s.ensureWorkspace("a");
    s.ensureWorkspace("b");
    s.addTerminal({ termId: "t1", sessionId: "b", title: "bash", name: null, cwd: null, status: "running", exitCode: null });
    s.addTerminalTab("b", "t1");
    const dialogs = [
      { kind: "confirmCloseTerminals", sessionId: "b" },
      { kind: "deleteEntry", sessionId: "b", path: "/x", entry: "file" },
      { kind: "moveEntry", sessionId: "b", path: "/x" },
      { kind: "goToFolder", sessionId: "b" },
      { kind: "replaceTransfers", sessionId: "b" },
    ] as const;
    for (const dialog of dialogs) {
      useStore.setState({ dialog, promptSession: "b", renamingSession: "b", renamingTerminal: "t1" });
      useStore.getState().setActiveSession("b");
      useStore.getState().setDropTarget("/x");
      useStore.getState().removeSession("b");
      const after = useStore.getState();
      expect(after.dialog, dialog.kind).toBeNull();
      expect(after.promptSession).toBeNull();
      expect(after.renamingSession).toBeNull();
      expect(after.renamingTerminal).toBeNull();
      expect(after.dropTarget).toBeNull();
      // set the session up again for the next round
      useStore.getState().addSession("b", { kind: "sshConfig", alias: "b" }, "b");
      useStore.getState().ensureWorkspace("b");
      useStore.getState().addTerminal({ termId: "t1", sessionId: "b", title: "bash", name: null, cwd: null, status: "running", exitCode: null });
      useStore.getState().addTerminalTab("b", "t1");
    }
  });

  it("A dialog of another session stays open when a different session is removed", () => {
    const s = useStore.getState();
    s.addSession("a", target, "a");
    s.addSession("b", { kind: "sshConfig", alias: "b" }, "b");
    useStore.setState({ dialog: { kind: "goToFolder", sessionId: "a" }, promptSession: "a" });
    useStore.getState().removeSession("b");
    expect(useStore.getState().dialog).toEqual({ kind: "goToFolder", sessionId: "a" });
    expect(useStore.getState().promptSession).toBe("a");
  });

  it("The cursor lives outside `buffers`, so moving it does not replace the buffers map", () => {
    const s = useStore.getState();
    s.addSession("a", target, "a");
    s.ensureWorkspace("a");
    s.addBuffer({ id: "a:/f", sessionId: "a", path: "/f", load: "ready" });
    const before = useStore.getState().buffers;
    s.setCursor("a:/f", { line: 2, col: 5 });
    expect(useStore.getState().buffers).toBe(before);
    expect(useStore.getState().cursors["a:/f"]).toEqual({ line: 2, col: 5 });
    const cursors = useStore.getState().cursors;
    s.setCursor("a:/f", { line: 2, col: 5 }); // unchanged: no update at all
    expect(useStore.getState().cursors).toBe(cursors);
    s.setCursor("nope", { line: 9, col: 9 }); // unknown buffer: ignored
    expect(useStore.getState().cursors["nope"]).toBeUndefined();
    s.renameBuffer("a:/f", "a:/g", "/g");
    expect(useStore.getState().cursors["a:/g"]).toEqual({ line: 2, col: 5 });
    expect(useStore.getState().cursors["a:/f"]).toBeUndefined();
  });

  it("A stats event does not replace sessionsById", () => {
    const s = useStore.getState();
    s.addSession("a", target, "a");
    const byId = useStore.getState().sessionsById;
    s.applySessionEvent("a", { type: "stats", txBytesPerSec: 5, rxBytesPerSec: 6 });
    expect(useStore.getState().sessionsById).toBe(byId);
    expect(useStore.getState().sessionStats["a"]).toEqual({ txBytesPerSec: 5, rxBytesPerSec: 6 });
    const stats = useStore.getState().sessionStats;
    s.applySessionEvent("a", { type: "stats", txBytesPerSec: 5, rxBytesPerSec: 6 });
    expect(useStore.getState().sessionStats).toBe(stats);
  });

  it("An unchanged listing keeps its object; a changed or stale one is replaced", () => {
    const s = useStore.getState();
    s.addSession("a", target, "a");
    s.ensureWorkspace("a");
    const entry = (name: string, size: number) => ({
      name, path: `/${name}`, kind: "file" as const, linkTargetKind: null, size, mtime: 1, permissions: 0o644,
      uid: 1, gid: 1, owner: null, group: null, lossyName: false,
    });
    const listing = (size: number) => ({ path: "/", entries: [entry("a", size)], truncated: false });
    s.setListing("a", "/", listing(1), 1000);
    const first = useStore.getState().workspaces["a"]?.tree.listings["/"];
    s.setListing("a", "/", listing(1), 2000); // same content, 1 s later: nothing changes
    expect(useStore.getState().workspaces["a"]?.tree.listings["/"]).toBe(first);
    s.setListing("a", "/", listing(1), 20_000); // same content, but old: only the age is renewed
    const renewed = useStore.getState().workspaces["a"]?.tree.listings["/"];
    expect(renewed?.listing).toBe(first?.listing);
    expect(renewed?.fetchedAtMs).toBe(20_000);
    s.setListing("a", "/", listing(2), 21_000); // content changed
    expect(useStore.getState().workspaces["a"]?.tree.listings["/"]?.listing.entries[0]?.size).toBe(2);
    s.markListingsStale("a");
    s.setListing("a", "/", listing(2), 22_000); // stale: replaced so the flag clears
    expect(useStore.getState().workspaces["a"]?.tree.listings["/"]?.stale).toBe(false);
  });

  it("workspace: tree expansion, editor and terminal tabs", () => {
    const s = useStore.getState();
    s.addSession("a", target, "a");
    s.ensureWorkspace("a");
    s.setTreeRoot("a", "/srv/app");
    expect(useStore.getState().workspaces["a"]?.tree.expanded["/srv/app"]).toBe(true);
    s.toggleExpanded("a", "/srv/app/nginx");
    expect(useStore.getState().workspaces["a"]?.tree.expanded["/srv/app/nginx"]).toBe(true);
    s.toggleExpanded("a", "/srv/app/nginx");
    expect(useStore.getState().workspaces["a"]?.tree.expanded["/srv/app/nginx"]).toBeUndefined();

    s.openEditorTab("a", "b1");
    s.openEditorTab("a", "b2");
    s.openEditorTab("a", "b1");
    expect(useStore.getState().workspaces["a"]?.editorTabs).toEqual(["b1", "b2"]);
    expect(useStore.getState().workspaces["a"]?.activeBuffer).toBe("b1");
    s.closeEditorTab("a", "b1");
    expect(useStore.getState().workspaces["a"]?.activeBuffer).toBe("b2");
  });

  it("state holds only plain objects and arrays (no Map/Set)", () => {
    useStore.getState().setBootstrap(makeBootstrap(Date.now()));
    const bad: string[] = [];
    const walk = (v: unknown, path: string) => {
      if (v instanceof Map || v instanceof Set) bad.push(path);
      else if (v && typeof v === "object") for (const [k, c] of Object.entries(v)) walk(c, `${path}.${k}`);
    };
    walk(useStore.getState(), "state");
    expect(bad).toEqual([]);
  });
});
