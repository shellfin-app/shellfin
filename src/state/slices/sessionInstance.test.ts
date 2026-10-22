import { beforeEach, describe, expect, it } from "vitest";
import type { RecentTarget, SessionInfo } from "../../ipc/types";
import { resetStore, useStore } from "../store";

const quick = (spec: string): RecentTarget => ({ kind: "quick", spec });

function info(sessionId: string, label: string): SessionInfo {
  return {
    sessionId,
    label,
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

const connect = (id: string, label: string) =>
  useStore.getState().applySessionEvent(id, { type: "state", state: { phase: "connected", info: info(id, label) } });

const rename = (id: string, label: string) =>
  useStore.getState().applySessionEvent(id, {
    type: "bookmarkUpdated",
    label,
    color: null,
    limitOverrides: { uploadKibps: null, downloadKibps: null },
    needsReconnect: false,
  });

const instance = (id: string) => useStore.getState().sessionsById[id]?.instance;

describe("session instance numbers", () => {
  beforeEach(() => resetStore());

  it("are chosen again when connecting replaces the label", () => {
    const s = useStore.getState();
    s.addSession("a", quick("alice@web"), "alice@web");
    s.addSession("b", quick("bob@web"), "bob@web");
    expect([instance("a"), instance("b")]).toEqual([1, 1]);
    connect("a", "web");
    connect("b", "web");
    expect([instance("a"), instance("b")]).toEqual([1, 2]);
  });

  it("a bookmark renamed to another open bookmark's name gets the next number", () => {
    const s = useStore.getState();
    s.addSession("a", quick("a"), "alpha");
    s.addSession("b", quick("b"), "beta");
    rename("b", "alpha");
    expect([instance("a"), instance("b")]).toEqual([1, 2]);
    rename("b", "gamma");
    expect(instance("b")).toBe(1);
  });

  it("keep their number while the label stays the same", () => {
    const s = useStore.getState();
    s.addSession("a", quick("a"), "web");
    s.addSession("b", quick("a"), "web");
    s.addSession("c", quick("a"), "web");
    s.removeSession("b");
    connect("c", "web");
    rename("c", "web");
    expect([instance("a"), instance("c")]).toEqual([1, 3]);
  });

  it("a reopened session takes its preferred number only while it is free", () => {
    const s = useStore.getState();
    s.addSession("a", quick("a"), "web");
    s.addSession("c", quick("a"), "web", false, 3);
    expect(instance("c")).toBe(3);
    s.addSession("d", quick("a"), "web", false, 3);
    expect(instance("d")).toBe(2);
  });
});
