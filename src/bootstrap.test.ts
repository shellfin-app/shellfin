import { afterEach, describe, expect, it, vi } from "vitest";
import { bootstrap } from "./bootstrap";
import { setBackend } from "./ipc";
import { MockBackend } from "./ipc/mock/mockBackend";
import { resetStore } from "./state/store";

describe("bootstrap", () => {
  afterEach(() => setBackend(null));

  it("a Retry after a partly failed listener setup does not register the other listeners twice", async () => {
    resetStore();
    const backend = new MockBackend();
    const original = backend.listen.bind(backend);
    let failedOnce = false;
    const registered: string[] = [];
    vi.spyOn(backend, "listen").mockImplementation(async (event, handler) => {
      if (event === "ssh-config:changed" && !failedOnce) {
        failedOnce = true;
        throw new Error("listen failed");
      }
      registered.push(event);
      return original(event, handler);
    });
    setBackend(backend);

    await expect(bootstrap()).rejects.toThrow("listen failed");
    await bootstrap(); // Retry

    expect(registered.length).toBeGreaterThan(0);
    expect(new Set(registered).size).toBe(registered.length); // every event exactly once
    expect(registered).toContain("ssh-config:changed");
    expect(registered).toContain("recents:changed");
  });
});
