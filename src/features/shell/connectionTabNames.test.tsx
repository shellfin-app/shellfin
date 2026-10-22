import { act, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { setBackend } from "../../ipc";
import type { SessionEvent } from "../../ipc/types";
import { setupConnected, settleApp } from "../../test/harness";
import { useStore } from "../../state/store";
import { duplicateSession } from "../session/connectionActions";
import { ConnectingCard } from "../session/ConnectingCard";
import { PromptOrigin } from "../session/PromptOrigin";
import { ReconnectOverlay } from "../session/ReconnectOverlay";
import { SessionErrorPanel } from "../session/SessionErrorPanel";
import { closeSession, closeSessionThenReopen } from "../session/sessionActions";
import { useMenuStore } from "../menu/menuStore";

afterEach(async () => {
  await settleApp();
  useMenuStore.getState().close();
  setBackend(null);
});

const tabNames = (): (string | null)[] => Array.from(document.querySelectorAll(".conn-tab__name")).map((e) => e.textContent);

async function duplicate(first: string, count: number): Promise<void> {
  act(() => duplicateSession(first));
  await waitFor(() => expect(useStore.getState().sessionOrder).toHaveLength(count));
}

describe("duplicated connection tabs", () => {
  it("number the copies: name, name (2), name (3)", async () => {
    const { sessionId } = await setupConnected({ tree: false });
    expect(tabNames()).toEqual(["prod-web-01"]);
    await duplicate(sessionId, 2);
    await duplicate(sessionId, 3);
    expect(tabNames()).toEqual(["prod-web-01", "prod-web-01 (2)", "prod-web-01 (3)"]);
    // tooltip and close button follow the tab text
    expect(screen.getByRole("button", { name: "Close connection prod-web-01 (2)" })).toBeTruthy();
    expect(screen.getByRole("tab", { name: /prod-web-01 \(3\)/ }).getAttribute("title")).toBe("prod-web-01 (3)");
  });

  it("the all-connections list uses the same names", async () => {
    const { sessionId, user } = await setupConnected({ tree: false });
    await duplicate(sessionId, 2);
    await user.click(screen.getByRole("button", { name: "All connections" }));
    const menu = await screen.findByRole("menu");
    expect(
      within(menu)
        .getAllByRole("menuitemcheckbox")
        .map((i) => i.textContent),
    ).toEqual(["Home", "prod-web-01", "prod-web-01 (2)"]);
  });

  it("closing (2) frees the number for the next copy", async () => {
    const { sessionId } = await setupConnected({ tree: false });
    await duplicate(sessionId, 2);
    await duplicate(sessionId, 3);
    const second = useStore.getState().sessionOrder[1] ?? "";
    await act(() => closeSession(second));
    expect(tabNames()).toEqual(["prod-web-01", "prod-web-01 (3)"]);
    await duplicate(sessionId, 3);
    expect(tabNames()).toEqual(["prod-web-01", "prod-web-01 (3)", "prod-web-01 (2)"]);
  });

  it("a renamed tab shows only its name", async () => {
    const { sessionId } = await setupConnected({ tree: false });
    await duplicate(sessionId, 2);
    const second = useStore.getState().sessionOrder[1] ?? "";
    act(() => useStore.getState().renameSession(second, "db"));
    expect(tabNames()).toEqual(["prod-web-01", "db"]);
    expect(screen.getByRole("button", { name: "Close connection db" })).toBeTruthy();
  });

  it("reconnect-to-apply keeps the tab's number and place", async () => {
    const { sessionId } = await setupConnected({ tree: false });
    await duplicate(sessionId, 2);
    await duplicate(sessionId, 3);
    const [, second, third] = useStore.getState().sessionOrder;
    await act(() => closeSession(second ?? ""));
    expect(tabNames()).toEqual(["prod-web-01", "prod-web-01 (3)"]);
    await act(() => closeSessionThenReopen(third ?? ""));
    await waitFor(() => expect(useStore.getState().sessionOrder).toHaveLength(2));
    await waitFor(() => expect(tabNames()).toEqual(["prod-web-01", "prod-web-01 (3)"]));
  });

  it("a prompt for the second duplicate names the numbered tab", async () => {
    const { sessionId } = await setupConnected({ tree: false });
    await duplicate(sessionId, 2);
    const second = useStore.getState().sessionOrder[1] ?? "";
    const hop = { index: 0, label: "prod-web-01", user: "deploy", host: "prod-web-01", port: 22 };
    render(<PromptOrigin sessionId={second} hop={hop} />);
    expect(screen.getByTestId("prompt-origin").textContent).toBe("Requested by tabprod-web-01 (2)");
  });

  it("the connecting card, the failed panel and the reconnect overlay of a duplicate show the tab's name", async () => {
    const { sessionId } = await setupConnected({ tree: false });
    await duplicate(sessionId, 2);
    const second = useStore.getState().sessionOrder[1] ?? "";
    const hop = { index: 0, label: "prod-web-01", user: "deploy", host: "prod-web-01", port: 22 };
    const setState = (event: SessionEvent) => act(() => useStore.getState().applySessionEvent(second, event));

    setState({ type: "state", state: { phase: "connecting", hop } });
    const card = render(<ConnectingCard sessionId={second} />);
    expect(within(card.container).getByRole("heading", { name: "prod-web-01 (2)" })).toBeTruthy();
    card.unmount();

    setState({ type: "state", state: { phase: "disconnected", error: { code: "connectTimeout", params: { host: "prod-web-01", port: 22, seconds: 10 } }, canRetry: true } });
    const panel = render(<SessionErrorPanel sessionId={second} />);
    expect(within(panel.container).getByRole("heading").textContent).toBe("Could not connect to prod-web-01 (2)");
    panel.unmount();

    setState({
      type: "state",
      state: {
        phase: "reconnecting",
        stage: "waiting",
        hop,
        attempt: 1,
        maxAttempts: 5,
        nextAttemptAtMs: Date.now() + 5000,
        cause: { code: "disconnected", params: { reason: "keepaliveTimeout", message: null } },
      },
    });
    const overlay = render(<ReconnectOverlay sessionId={second} />);
    expect(within(overlay.container).getByRole("group", { name: "Connection to prod-web-01 (2) lost" })).toBeTruthy();
    overlay.unmount();
  });

  it("the status bar of a duplicate that has no hop yet shows the numbered name, a rename wins", async () => {
    const { sessionId } = await setupConnected({ tree: false });
    await duplicate(sessionId, 2);
    const second = useStore.getState().sessionOrder[1] ?? "";
    // Let the copy finish connecting first, so a late "connected" cannot overwrite the state set below, then drop
    // its info: a connection that has not connected yet has no hop to show.
    await waitFor(() => expect(useStore.getState().sessionsById[second]?.state.phase).toBe("connected"));
    act(() =>
      useStore.setState((s) => {
        const entry = s.sessionsById[second];
        if (!entry) return {};
        const { info: _info, ...rest } = entry;
        return { sessionsById: { ...s.sessionsById, [second]: rest } };
      }),
    );
    act(() => useStore.getState().setActiveSession(second));
    const status = () => document.querySelector(".status-bar__item--strong bdi")?.textContent;
    await waitFor(() => expect(status()).toBe("prod-web-01 (2)"));
    act(() => useStore.getState().renameSession(second, "db"));
    await waitFor(() => expect(status()).toBe("db"));
  });

  it("reordering keeps the numbers", async () => {
    const { sessionId } = await setupConnected({ tree: false });
    await duplicate(sessionId, 2);
    await duplicate(sessionId, 3);
    act(() => useStore.getState().placeSession(sessionId, 2));
    expect(tabNames()).toEqual(["prod-web-01 (2)", "prod-web-01 (3)", "prod-web-01"]);
  });
});
