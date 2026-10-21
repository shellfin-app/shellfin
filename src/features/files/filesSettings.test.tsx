// Files settings (ARCHITECTURE §8.11): double-click action, size units, date format, permissions column, auto-refresh.
import { act, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import type { Settings } from "../../ipc/types";
import { useStore } from "../../state/store";
import { settleApp, setupConnected } from "../../test/harness";
import { autoRefreshSession, autoRefreshTick, installAutoRefresh, isAutoRefreshing, visibleFolders } from "./autoRefresh";

afterEach(async () => {
  vi.useRealTimers();
  await settleApp();
  setBackend(null);
  vi.restoreAllMocks();
});

const row = (name: RegExp | string) => screen.getByRole("treeitem", { name });

function patchFiles(p: Partial<Settings["files"]>): void {
  const settings = useStore.getState().settings;
  if (!settings) throw new Error("no settings");
  act(() => useStore.getState().setSettings({ ...settings, files: { ...settings.files, ...p } }));
}

describe("double-click", () => {
  it("open (default): a click opens the file in the editor", async () => {
    const { user, sessionId } = await setupConnected();
    await user.click(row("README.md"));
    await waitFor(() => expect(useStore.getState().buffers[`${sessionId}:/srv/app/README.md`]?.load).toBe("ready"));
  });

  it("download: a click only selects, a double-click starts the download, folders still expand", async () => {
    const { user, backend, sessionId } = await setupConnected();
    patchFiles({ doubleClick: "download" });
    const download = vi.spyOn(backend, "transferDownload");
    await user.click(row("README.md"));
    expect(useStore.getState().workspaces[sessionId]?.tree.selection).toBe("/srv/app/README.md");
    expect(Object.keys(useStore.getState().buffers)).toHaveLength(0);
    expect(download).not.toHaveBeenCalled();

    await user.dblClick(row("README.md"));
    await waitFor(() => expect(download).toHaveBeenCalledWith(sessionId, "/srv/app/README.md", expect.any(String), expect.any(Function)));
    expect(Object.keys(useStore.getState().buffers)).toHaveLength(0);

    await user.click(row("nginx"));
    await screen.findByRole("treeitem", { name: /nginx\.conf/ });
  });
});

describe("permissions column", () => {
  const META = ".file-row__meta";

  it("is off by default", async () => {
    await setupConnected();
    expect(document.querySelector(META)).toBeNull();
  });

  it("on: shows rwx, owner and group per row, as text", async () => {
    await setupConnected();
    patchFiles({ showPermissions: true });
    await waitFor(() => expect(document.querySelector(META)).toBeTruthy());
    expect(row(/deploy\.sh/).querySelector(META)?.textContent).toBe("-rwxr-xr-x alice alice");
    expect(row(/\.env\.production/).querySelector(META)?.textContent).toBe("-rw------- alice alice");
    expect(row(/^nginx/).querySelector(META)?.textContent).toMatch(/^drwx/);
  });

  it("falls back to the numeric ids when the server gave no names", async () => {
    const { sessionId } = await setupConnected();
    patchFiles({ showPermissions: true });
    act(() => {
      const listing = useStore.getState().workspaces[sessionId]?.tree.listings["/srv/app"];
      if (!listing) throw new Error("no listing");
      useStore.getState().setListing(
        sessionId,
        "/srv/app",
        { ...listing.listing, entries: listing.listing.entries.map((e) => (e.name === "deploy.sh" ? { ...e, owner: null, group: null } : e)) },
        Date.now(),
      );
    });
    await waitFor(() => expect(row(/deploy\.sh/).querySelector(META)?.textContent).toBe("-rwxr-xr-x 1000 1000"));
  });

  it("server names are shown as text and bounded, never as markup", async () => {
    const { sessionId } = await setupConnected();
    patchFiles({ showPermissions: true });
    act(() => {
      const listing = useStore.getState().workspaces[sessionId]?.tree.listings["/srv/app"];
      if (!listing) throw new Error("no listing");
      useStore.getState().setListing(
        sessionId,
        "/srv/app",
        { ...listing.listing, entries: listing.listing.entries.map((e) => (e.name === "deploy.sh" ? { ...e, owner: "<img src=x>‮" } : e)) },
        Date.now(),
      );
    });
    await waitFor(() => expect(row(/deploy\.sh/).querySelector(META)?.textContent).toContain("<img src=x>"));
    expect(row(/deploy\.sh/).querySelector("img")).toBeNull();
    expect(row(/deploy\.sh/).querySelector(META)?.textContent).not.toContain("‮");
  });
});

describe("tooltip details: sizes and dates follow the settings", () => {
  it("binary units and relative dates by default, decimal and absolute on request", async () => {
    const { sessionId } = await setupConnected();
    const entry = useStore.getState().workspaces[sessionId]?.tree.listings["/srv/app"]?.listing.entries.find((e) => e.name === "README.md");
    expect(entry?.size).toBeTruthy();
    const title = () => row("README.md").getAttribute("title") ?? "";
    expect(title()).toMatch(/Size: [\d.]+ B/);
    expect(title()).toMatch(/Modified: .*(ago|now|yesterday)/);

    // a 1500 byte file shows "1.5 KiB" in binary and "1.5 KB" in decimal
    act(() => {
      const listing = useStore.getState().workspaces[sessionId]?.tree.listings["/srv/app"];
      if (!listing) throw new Error("no listing");
      useStore.getState().setListing(
        sessionId,
        "/srv/app",
        { ...listing.listing, entries: listing.listing.entries.map((e) => (e.name === "README.md" ? { ...e, size: 1500 } : e)) },
        Date.now(),
      );
    });
    await waitFor(() => expect(title()).toContain("Size: 1.5 KiB"));
    patchFiles({ sizeUnits: "decimal", dateFormat: "absolute" });
    await waitFor(() => expect(title()).toContain("Size: 1.5 KB"));
    expect(title()).not.toMatch(/ago|yesterday/);
    expect(title()).toMatch(/Modified: [A-Z][a-z]{2} \d{1,2}, \d{4}/);
  });
});

describe("auto-refresh", () => {
  it("reloads the root and the expanded folders, nothing else", async () => {
    const { user, backend, sessionId } = await setupConnected();
    await user.click(row("nginx"));
    await screen.findByRole("treeitem", { name: /nginx\.conf/ });
    const list = vi.spyOn(backend, "sftpList");
    await autoRefreshSession(sessionId);
    const paths = list.mock.calls.map((c) => c[1]).sort();
    expect(paths).toEqual(["/srv/app", "/srv/app/nginx"]);
  });

  it("skips expanded folders that are not on screen (collapsed parent, other root)", () => {
    const expanded = { "/srv/app/a": true, "/srv/app/a/b": true, "/srv/app/c/d": true, "/etc": true, "/srv/application": true } as const;
    expect(visibleFolders("/srv/app", expanded)).toEqual(["/srv/app", "/srv/app/a", "/srv/app/a/b"]);
    expect(visibleFolders("/", { "/etc": true, "/etc/ssh": true, "/var/log": true })).toEqual(["/", "/etc", "/etc/ssh"]);
  });

  it("never overlaps: a slow refresh makes the next tick skip the session", async () => {
    const { backend, sessionId } = await setupConnected();
    let release: () => void = () => undefined;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const real = backend.sftpList.bind(backend);
    const list = vi.spyOn(backend, "sftpList").mockImplementation(async (...args) => {
      await gate;
      return real(...args);
    });
    autoRefreshTick();
    await waitFor(() => expect(isAutoRefreshing(sessionId)).toBe(true));
    autoRefreshTick();
    autoRefreshTick();
    expect(list).toHaveBeenCalledTimes(1);
    release();
    await waitFor(() => expect(isAutoRefreshing(sessionId)).toBe(false));
    autoRefreshTick();
    await waitFor(() => expect(list.mock.calls.length).toBeGreaterThan(1));
  });

  it("is paused while the window is hidden", async () => {
    const { backend } = await setupConnected();
    const list = vi.spyOn(backend, "sftpList");
    const visibility = vi.spyOn(document, "visibilityState", "get").mockReturnValue("hidden");
    autoRefreshTick();
    await settleApp(40);
    expect(list).not.toHaveBeenCalled();
    visibility.mockReturnValue("visible");
    autoRefreshTick();
    await waitFor(() => expect(list).toHaveBeenCalled());
  });

  it("skips a connection that is not ready", async () => {
    const { backend, sessionId } = await setupConnected();
    const list = vi.spyOn(backend, "sftpList");
    act(() => useStore.getState().applySessionEvent(sessionId, { type: "state", state: { phase: "disconnected", error: null, canRetry: true } }));
    autoRefreshTick();
    await autoRefreshSession(sessionId);
    expect(list).not.toHaveBeenCalled();
  });

  it("does not move rows under an open name input", async () => {
    const { backend, sessionId } = await setupConnected();
    act(() => useStore.getState().setTreeEdit(sessionId, { kind: "new", parent: "/srv/app", entry: "file" }));
    const list = vi.spyOn(backend, "sftpList");
    await autoRefreshSession(sessionId);
    expect(list).not.toHaveBeenCalled();
  });

  it("a failing refresh is silent and keeps the old listing", async () => {
    const { backend, sessionId } = await setupConnected();
    vi.spyOn(backend, "sftpList").mockRejectedValue(new Error("boom"));
    const noticesBefore = useStore.getState().notices.length;
    await autoRefreshSession(sessionId);
    expect(useStore.getState().notices).toHaveLength(noticesBefore);
    expect(useStore.getState().workspaces[sessionId]?.tree.listings["/srv/app"]).toBeTruthy();
  });

  it("the timer follows the setting: off does nothing, 5 s ticks every 5 s, changes restart it", async () => {
    const { backend } = await setupConnected();
    vi.useFakeTimers({ shouldAdvanceTime: false, toFake: ["setInterval", "clearInterval"] });
    const list = vi.spyOn(backend, "sftpList");
    const uninstall = installAutoRefresh();
    try {
      vi.advanceTimersByTime(60_000);
      expect(list).not.toHaveBeenCalled();

      patchFiles({ autoRefresh: 5 });
      vi.advanceTimersByTime(4_900);
      expect(list).not.toHaveBeenCalled();
      vi.advanceTimersByTime(200);
      await vi.waitFor(() => expect(list).toHaveBeenCalled());

      list.mockClear();
      patchFiles({ autoRefresh: 0 });
      vi.advanceTimersByTime(60_000);
      expect(list).not.toHaveBeenCalled();
    } finally {
      uninstall();
    }
  });
});
