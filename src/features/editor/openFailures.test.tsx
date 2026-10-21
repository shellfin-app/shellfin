import { screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import { IpcError } from "../../ipc/call";
import { useStore } from "../../state/store";
import { settleApp, setupConnected } from "../../test/harness";

afterEach(async () => {
  await settleApp();
  setBackend(null);
  vi.restoreAllMocks();
});

const row = (name: RegExp | string) => screen.getByRole("treeitem", { name });

describe("open failures the editor area explains in place", () => {
  it("notARegularFile (FIFO, socket, device): failure page with the reason, no extra toast", async () => {
    const { user, backend, sessionId } = await setupConnected();
    vi.spyOn(backend, "sftpReadText").mockRejectedValue(
      new IpcError({ code: "notARegularFile", params: { path: "/srv/app/deploy.sh" } }),
    );
    await user.click(row("deploy.sh"));
    await waitFor(() => expect(useStore.getState().buffers[`${sessionId}:/srv/app/deploy.sh`]?.load).toBe("error"));
    expect(await screen.findByText(/is not a regular file/)).toBeTruthy();
    expect(document.querySelector(".cm-editor")).toBeNull();
    expect(useStore.getState().notices).toHaveLength(0);
  });

  it("an unexpected failure is still reported as a notice", async () => {
    const { user, backend, sessionId } = await setupConnected();
    vi.spyOn(backend, "sftpReadText").mockRejectedValue(
      new IpcError({ code: "sftpFailure", params: { status: "permissionDenied" } } as never),
    );
    await user.click(row("deploy.sh"));
    await waitFor(() => expect(useStore.getState().buffers[`${sessionId}:/srv/app/deploy.sh`]?.load).toBe("error"));
    await waitFor(() => expect(useStore.getState().notices.length).toBeGreaterThan(0));
  });
});
