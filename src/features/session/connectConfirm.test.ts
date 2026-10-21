import { beforeEach, describe, expect, it } from "vitest";
import { resetStore, useStore } from "../../state/store";
import { answerConnectConfirm, requestConnectConfirm } from "./connectConfirm";

describe("connect confirmation from a background flow", () => {
  beforeEach(() => resetStore());

  it("a background question waits for the open dialog to close instead of replacing it", async () => {
    useStore.getState().openDialog({ kind: "settings" });
    const answer = requestConnectConfirm("prod", { background: true });
    await Promise.resolve();
    expect(useStore.getState().dialog).toEqual({ kind: "settings" });

    useStore.getState().closeDialog();
    await Promise.resolve();
    await Promise.resolve();
    expect(useStore.getState().dialog).toEqual({ kind: "confirmConnect", name: "prod" });
    answerConnectConfirm(true);
    expect(await answer).toBe(true);
  });

  it("a user-initiated question opens at once", () => {
    const answer = requestConnectConfirm("prod");
    expect(useStore.getState().dialog).toEqual({ kind: "confirmConnect", name: "prod" });
    answerConnectConfirm(false);
    return expect(answer).resolves.toBe(false);
  });
});
