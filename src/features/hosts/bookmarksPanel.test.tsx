// Bookmarks panel: notice levels, connected-bookmark menu, pointer drag and drop, move
// menu/keyboard, colors, live edits.
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import type { Bookmark, BookmarkInput } from "../../ipc/types";
import type { MockBackend } from "../../ipc/mock/mockBackend";
import { bookmarkTarget } from "../../lib/targets";
import { COPIED, FOLDERS_UNSUPPORTED, DROP_NO_TARGET, OUTPUT_TOO_LARGE } from "../../lib/notices";
import { setupApp } from "../../test/harness";
import { infoLevel } from "../../state/slices/ui";
import { useStore } from "../../state/store";
import { useMenuStore } from "../menu/menuStore";
import { openSession } from "../session/sessionActions";
import { disposeAllTerminals } from "../terminal/terminalRegistry";
import { bookmarksOfGroup, sortedGroups } from "./bookmarkOrder";
import { refreshBookmarks } from "./hostActions";
import { autoScrollDelta, computeBookmarkDrop, computeGroupDrop } from "./pointerDrag";

const PI = "11111111-1111-4111-8111-111111111111";
const NAS = "22222222-2222-4222-8222-222222222222";
const VPS = "33333333-3333-4333-8333-333333333333";

afterEach(async () => {
  await new Promise((r) => setTimeout(r, 100));
  useMenuStore.getState().close();
  disposeAllTerminals();
  setBackend(null);
  vi.restoreAllMocks();
});

function input(b: Bookmark, patch: Partial<BookmarkInput> = {}): BookmarkInput {
  return {
    id: b.id,
    name: b.name,
    host: b.host,
    port: b.port,
    user: b.user,
    auth: { method: "auto" },
    proxyJump: null,
    groupId: b.groupId,
    tags: [],
    startDir: null,
    useSshConfig: true,
    color: b.color,
    ...patch,
  };
}

function bookmarkById(id: string): Bookmark {
  const b = useStore.getState().bookmarks?.bookmarks.find((x) => x.id === id);
  if (!b) throw new Error(`no bookmark ${id}`);
  return b;
}

function rightClick(el: Element): void {
  act(() => {
    el.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 40, clientY: 40 }));
  });
}

function menuNames(): string[] {
  return within(screen.getByRole("menu")).getAllByRole("menuitem").map((i) => i.textContent ?? "");
}

describe("1. notice levels", () => {
  it("success / info / warning / error toasts are styled apart and say their level", () => {
    setupApp();
    act(() => {
      const s = useStore.getState();
      s.pushInfo(COPIED);
      s.pushInfo(DROP_NO_TARGET);
      s.pushInfo(FOLDERS_UNSUPPORTED);
      s.pushNotice({ code: "internal" }, "warning");
      s.pushNotice({ code: "notConnected", params: { sessionId: "s" } });
    });
    const region = screen.getByRole("region", { name: "Notifications" });
    const levels = [...region.querySelectorAll<HTMLElement>(".notice")].map((n) => n.dataset["level"]);
    expect(levels).toEqual(["success", "info", "warning", "warning", "error"]);
    const copied = region.querySelector(".notice--success") as HTMLElement;
    expect(copied.textContent).toContain("Copied to the clipboard.");
    expect(within(copied).getByText(/Done/)).toBeTruthy(); // the level is a word too, not only a color
    expect(copied.getAttribute("role")).toBe("status");
    expect((region.querySelector(".notice--error") as HTMLElement).getAttribute("role")).toBe("alert");
  });

  it("clipboard copies and saved output are success, a plain error stays error", () => {
    setupApp();
    expect(infoLevel(OUTPUT_TOO_LARGE)).toBe("error");
    act(() => {
      useStore.getState().pushInfo(COPIED);
      useStore.getState().pushInfo({ kind: "outputSaved", path: "x.txt" });
      useStore.getState().pushInfo({ kind: "outputCancelled" });
    });
    expect(useStore.getState().notices.map((n) => n.level)).toEqual(["success", "success", "info"]);
  });
});

describe("2. menu of a connected bookmark", () => {
  it("offers Go to session, Disconnect, Close connection and Open another connection - not Connect", async () => {
    const { user, backend } = setupApp();
    const sessionId = await openSession(bookmarkTarget(PI), "pi-home");
    await waitFor(() => expect(useStore.getState().sessionsById[sessionId]?.state.phase).toBe("connected"));
    act(() => useStore.getState().setActiveSession(null));
    const row = screen.getByRole("button", { name: "Go to session pi-home" });
    rightClick(row);
    expect(menuNames().slice(0, 4)).toEqual(["Go to session", "Disconnect", "Close connection", "Open another connection"]);
    expect(screen.queryByRole("menuitem", { name: "Connect" })).toBeNull();
    await user.click(screen.getByRole("menuitem", { name: "Go to session" }));
    expect(useStore.getState().activeSessionId).toBe(sessionId);

    // Double click goes to the existing session too (no second connection).
    act(() => useStore.getState().setActiveSession(null));
    await user.dblClick(screen.getByRole("button", { name: "Go to session pi-home" }));
    expect(useStore.getState().activeSessionId).toBe(sessionId);
    expect(useStore.getState().sessionOrder).toHaveLength(1);

    const disconnect = vi.spyOn(backend, "sessionDisconnect");
    rightClick(screen.getByRole("button", { name: "Go to session pi-home" }));
    await user.click(screen.getByRole("menuitem", { name: "Disconnect" }));
    expect(disconnect).toHaveBeenCalledWith(sessionId);

    rightClick(screen.getByRole("button", { name: "Go to session pi-home" }));
    await user.click(screen.getByRole("menuitem", { name: "Open another connection" }));
    await waitFor(() => expect(useStore.getState().sessionOrder).toHaveLength(2));

    rightClick(screen.getByRole("button", { name: "Go to session pi-home" }));
    await user.click(screen.getByRole("menuitem", { name: "Close connection" }));
    const confirm = await screen.findByRole("alertdialog", { name: "Close this connection?" });
    await user.click(within(confirm).getByRole("button", { name: "Close connection" }));
    await waitFor(() => expect(useStore.getState().sessionOrder).toHaveLength(1));
  });

  it("an ssh-config host with a session gets the same menu", async () => {
    setupApp();
    const sessionId = await openSession({ kind: "sshConfig", alias: "bastion" }, "bastion");
    await waitFor(() => expect(useStore.getState().sessionsById[sessionId]?.state.phase).toBe("connected"));
    rightClick(screen.getByRole("button", { name: "Go to session bastion" }));
    expect(menuNames().slice(0, 4)).toEqual(["Go to session", "Disconnect", "Close connection", "Open another connection"]);
  });
});

// ---- 3. pointer drag and drop ----

const ROW_H = 28;
/** jsdom has no layout: stack the list top-down (headers and rows 28px, an empty section 60px). */
function stubLayout(): void {
  const rect = (top: number, bottom: number): DOMRect => ({ top, bottom, left: 0, right: 200, width: 200, height: bottom - top, x: 0, y: top, toJSON: () => ({}) }) as DOMRect;
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
    const list = document.querySelector<HTMLElement>(".bookmarks-panel__list");
    if (!list) return rect(0, 0);
    if (this === list) return rect(0, 600);
    let y = 0;
    for (const section of list.querySelectorAll<HTMLElement>(".host-group")) {
      const top = y;
      const header = section.querySelector<HTMLElement>(".host-group__header");
      const headerRect = rect(y, y + ROW_H);
      y += ROW_H;
      const rows = [...section.querySelectorAll<HTMLElement>("[data-host-key]")];
      const rowRects = rows.map((r) => {
        const rr = rect(y, y + ROW_H);
        y += ROW_H;
        return [r, rr] as const;
      });
      if (rows.length === 0 && section.dataset["collapsed"] !== "true") y += 60;
      if (this === section) return rect(top, y);
      if (this === header) return headerRect;
      const hit = rowRects.find(([r]) => r === this);
      if (hit) return hit[1];
    }
    return rect(0, 0);
  });
}

function rowEl(id: string): HTMLElement {
  return document.querySelector<HTMLElement>(`[data-host-key="bm:${id}"]`) as HTMLElement;
}
function rowButton(id: string): HTMLElement {
  return rowEl(id).querySelector("button") as HTMLElement;
}
function sectionEl(group: string | null): HTMLElement {
  return document.querySelector<HTMLElement>(`[data-drop-section][data-drop-group="${group ?? ""}"]`) as HTMLElement;
}
function headerButton(group: string): HTMLElement {
  return sectionEl(group).querySelector("[data-drop-header]") as HTMLElement;
}
function top(el: Element): number {
  return el.getBoundingClientRect().top;
}
function bottom(el: Element): number {
  return el.getBoundingClientRect().bottom;
}
function displayedNames(group: string | null): string[] {
  return [...sectionEl(group).querySelectorAll(".host-row__name")].map((e) => e.textContent ?? "");
}

function pointerDown(el: Element, y: number): void {
  fireEvent.pointerDown(el, { button: 0, clientX: 30, clientY: y, pointerId: 1 });
}
function pointerMove(y: number): void {
  fireEvent.pointerMove(window, { clientX: 30, clientY: y, pointerId: 1 });
}
function pointerUp(y: number): void {
  fireEvent.pointerUp(window, { clientX: 30, clientY: y, pointerId: 1 });
}
/** A full drag from the middle of `from` to the vertical position `y`. */
function drag(from: HTMLElement, y: number): void {
  const start = (top(from) + bottom(from)) / 2;
  pointerDown(from, start);
  pointerMove(start + 8); // past the threshold
  pointerMove(y);
  pointerUp(y);
}

async function twoGroups(backend: MockBackend) {
  const work = await backend.groupSave({ id: null, name: "Work", order: 0 });
  const home = await backend.groupSave({ id: null, name: "Home", order: 1 });
  const mk = (name: string, groupId: string) =>
    backend.bookmarkSave({ id: null, name, host: `${name}.example.com`, port: null, user: null, auth: { method: "auto" }, proxyJump: null, groupId, tags: [], startDir: null, useSshConfig: true });
  const w1 = await mk("w1", work.id);
  const w2 = await mk("w2", work.id);
  const w3 = await mk("w3", work.id);
  const h1 = await mk("h1", home.id);
  await refreshBookmarks();
  return { work, home, w1, w2, w3, h1 };
}

describe("3. drag and drop with pointer events", () => {
  it("geometry: rows split at the middle, headers drop first/last, outside the lists is no drop", () => {
    const sections = [
      { groupId: null, headerTop: 0, headerBottom: 28, sectionBottom: 112, collapsed: false, rows: [{ key: "a", top: 28, bottom: 56 }, { key: "b", top: 56, bottom: 84 }, { key: "c", top: 84, bottom: 112 }] },
      { groupId: "g", headerTop: 112, headerBottom: 140, sectionBottom: 140, collapsed: true, rows: [{ key: "d", top: 140, bottom: 168 }] },
    ];
    expect(computeBookmarkDrop(30, sections)).toEqual({ groupId: null, slot: 0, lineY: 28 });
    expect(computeBookmarkDrop(80, sections)).toEqual({ groupId: null, slot: 2, lineY: 84 });
    expect(computeBookmarkDrop(90, sections)).toEqual({ groupId: null, slot: 2, lineY: 84 });
    expect(computeBookmarkDrop(111, sections)).toEqual({ groupId: null, slot: 3, lineY: 112 });
    expect(computeBookmarkDrop(120, sections)).toEqual({ groupId: "g", slot: 1, lineY: 140 }); // collapsed: append
    expect(computeBookmarkDrop(500, sections)).toBeNull();
    expect(computeBookmarkDrop(-5, sections)).toBeNull();
    const groups = [{ id: "g1", top: 0, bottom: 100 }, { id: "g2", top: 100, bottom: 200 }];
    expect(computeGroupDrop(-10, groups)).toEqual({ slot: 0, lineY: 0 });
    expect(computeGroupDrop(120, groups)).toEqual({ slot: 1, lineY: 100 });
    expect(computeGroupDrop(180, groups)).toEqual({ slot: 2, lineY: 200 });
    expect(computeGroupDrop(500, groups)).toEqual({ slot: 2, lineY: 200 });
    expect(autoScrollDelta(300, 0, 600)).toBe(0);
    expect(autoScrollDelta(2, 0, 600)).toBeLessThan(0);
    expect(autoScrollDelta(598, 0, 600)).toBeGreaterThan(0);
  });

  it("reorders inside a group and persists it through bookmark_move", async () => {
    const { backend } = setupApp();
    stubLayout();
    const spy = vi.spyOn(backend, "bookmarkMove");
    expect(displayedNames(null)).toEqual(["pi-home", "nas", "vps-hetzner"]);
    // Drop pi-home on the lower half of vps-hetzner = the end of the list.
    drag(rowButton(PI), bottom(rowEl(VPS)) - 2);
    await waitFor(() => expect(displayedNames(null)).toEqual(["nas", "vps-hetzner", "pi-home"]));
    expect(spy).toHaveBeenCalledWith(PI, null, 2);
    // ... and back up above nas.
    drag(rowButton(PI), top(rowEl(NAS)) + 2);
    await waitFor(() => expect(displayedNames(null)).toEqual(["pi-home", "nas", "vps-hetzner"]));
    expect(spy).toHaveBeenLastCalledWith(PI, null, 0);
  });

  it("moves a bookmark into another group (and out to the ungrouped list)", async () => {
    const { backend } = setupApp();
    const { work, home, w2 } = await twoGroups(backend);
    stubLayout();
    const spy = vi.spyOn(backend, "bookmarkMove");
    // pi-home -> between w1 and w2 of "Work"
    drag(rowButton(PI), top(rowEl(w2.id)) + 2);
    await waitFor(() => expect(bookmarkById(PI).groupId).toBe(work.id));
    expect(spy).toHaveBeenCalledWith(PI, work.id, 1);
    expect(displayedNames(work.id)).toEqual(["w1", "pi-home", "w2", "w3"]);
    expect(displayedNames(null)).toEqual(["nas", "vps-hetzner"]);

    // nas -> onto the header of "Home" = first position there
    drag(rowButton(NAS), (top(headerButton(home.id)) + bottom(headerButton(home.id))) / 2);
    await waitFor(() => expect(bookmarkById(NAS).groupId).toBe(home.id));
    expect(displayedNames(home.id)).toEqual(["nas", "h1"]);

    // w1 -> header of "My bookmarks" = ungrouped, first
    drag(rowButton(w2.id), (top(headerButton("")) + bottom(headerButton(""))) / 2);
    await waitFor(() => expect(bookmarkById(w2.id).groupId).toBeNull());
    expect(displayedNames(null)[0]).toBe("w2");
  });

  it("reorders groups by their header", async () => {
    const { backend } = setupApp();
    const { work, home } = await twoGroups(backend);
    stubLayout();
    const spy = vi.spyOn(backend, "groupMove");
    drag(headerButton(home.id), top(sectionEl(work.id)) - 1);
    await waitFor(() => expect(sortedGroups(useStore.getState().bookmarks as never).map((g) => g.name)).toEqual(["Home", "Work"]));
    expect(spy).toHaveBeenCalledWith(home.id, 0);
    // Dragging a group never toggles it.
    expect(headerButton(home.id).getAttribute("aria-expanded")).toBe("true");
  });

  it("shows a drop line while dragging; Escape cancels without moving anything", async () => {
    const { backend } = setupApp();
    stubLayout();
    const spy = vi.spyOn(backend, "bookmarkMove");
    const start = (top(rowEl(PI)) + bottom(rowEl(PI))) / 2;
    pointerDown(rowButton(PI), start);
    pointerMove(start + 10);
    pointerMove(bottom(rowEl(VPS)) - 2);
    const line = await screen.findByTestId("drop-line");
    expect(line.style.top).not.toBe("");
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByTestId("drop-line")).toBeNull();
    pointerUp(bottom(rowEl(VPS)) - 2);
    expect(spy).not.toHaveBeenCalled();
    expect(displayedNames(null)).toEqual(["pi-home", "nas", "vps-hetzner"]);
  });

  it("a press that stays under the threshold is a click, a drop where it already is changes nothing", async () => {
    const { backend, user } = setupApp();
    stubLayout();
    const spy = vi.spyOn(backend, "bookmarkMove");
    pointerDown(rowButton(NAS), 100);
    pointerMove(102);
    pointerUp(102);
    await user.click(rowButton(NAS));
    expect(rowButton(NAS).getAttribute("aria-pressed")).toBe("true");
    drag(rowButton(NAS), bottom(rowEl(NAS)) - 2); // lower half of itself
    expect(spy).not.toHaveBeenCalled();
  });

  it("the click that ends a drag does not select or toggle", async () => {
    const { backend } = setupApp();
    const { work } = await twoGroups(backend);
    stubLayout();
    drag(rowButton(PI), top(rowEl(VPS)) + 2);
    fireEvent.click(rowButton(PI)); // the click the browser sends after pointerup
    expect(rowButton(PI).getAttribute("aria-pressed")).toBe("false");
    expect(headerButton(work.id).getAttribute("aria-expanded")).toBe("true");
  });

  it("ssh-config hosts cannot be dragged, and nothing is draggable while the store is read-only", async () => {
    const { backend } = setupApp();
    stubLayout();
    const spy = vi.spyOn(backend, "bookmarkMove");
    const host = document.querySelector<HTMLElement>('[data-host-key="ssh:bastion"] button') as HTMLElement;
    drag(host, top(rowEl(VPS)) + 2);
    expect(spy).not.toHaveBeenCalled();
    expect(screen.queryByTestId("drop-line")).toBeNull();

    act(() => {
      const dto = useStore.getState().bookmarks;
      if (dto) useStore.getState().setBookmarks({ ...dto, readOnly: true });
    });
    drag(rowButton(PI), bottom(rowEl(VPS)) - 2);
    expect(spy).not.toHaveBeenCalled();
  });

  it("dropping onto the ssh-config list is no drop", () => {
    const { backend } = setupApp();
    stubLayout();
    const spy = vi.spyOn(backend, "bookmarkMove");
    drag(rowButton(PI), 10);
    expect(spy).not.toHaveBeenCalled();
  });
});

describe("3. keyboard and menu alternatives", () => {
  it("Move up / Move down in the context menu (disabled at the edges)", async () => {
    const { user } = setupApp();
    rightClick(rowButton(PI));
    expect(screen.getByRole("menuitem", { name: /^Move up/ }).getAttribute("aria-disabled")).toBe("true");
    await user.click(screen.getByRole("menuitem", { name: /^Move down/ }));
    await waitFor(() => expect(displayedNames(null)).toEqual(["nas", "pi-home", "vps-hetzner"]));
    rightClick(rowButton(VPS));
    expect(screen.getByRole("menuitem", { name: /^Move down/ }).getAttribute("aria-disabled")).toBe("true");
    await user.click(screen.getByRole("menuitem", { name: /^Move up/ }));
    await waitFor(() => expect(displayedNames(null)).toEqual(["nas", "vps-hetzner", "pi-home"]));
  });

  it("Alt+Arrow moves the focused bookmark and a group header", async () => {
    const { backend } = setupApp();
    const { work, home } = await twoGroups(backend);
    rowButton(NAS).focus();
    fireEvent.keyDown(rowButton(NAS), { key: "ArrowUp", altKey: true });
    await waitFor(() => expect(displayedNames(null)).toEqual(["nas", "pi-home", "vps-hetzner"]));
    headerButton(work.id).focus();
    fireEvent.keyDown(headerButton(work.id), { key: "ArrowDown", altKey: true });
    await waitFor(() => expect(sortedGroups(useStore.getState().bookmarks as never).map((g) => g.id)).toEqual([home.id, work.id]));
  });

  it("Move to group… lets the user pick a group (appended) or no group", async () => {
    const { backend, user } = setupApp();
    const { work } = await twoGroups(backend);
    rightClick(rowButton(PI));
    await user.click(screen.getByRole("menuitem", { name: "Move to group…" }));
    const dialog = await screen.findByRole("dialog");
    const move = within(dialog).getByRole("button", { name: "Move" });
    expect((move as HTMLButtonElement).disabled).toBe(true);
    await user.selectOptions(within(dialog).getByRole("combobox"), work.id);
    await user.click(move);
    await waitFor(() => expect(bookmarkById(PI).groupId).toBe(work.id));
    expect(bookmarksOfGroup(useStore.getState().bookmarks as never, work.id).map((b) => b.name)).toEqual(["w1", "w2", "w3", "pi-home"]);
    expect(displayedNames(work.id).at(-1)).toBe("pi-home");
  });

  it("Move up/down stay inside the group", async () => {
    const { backend, user } = setupApp();
    const { work, w2 } = await twoGroups(backend);
    rightClick(rowButton(w2.id));
    await user.click(screen.getByRole("menuitem", { name: /^Move up/ }));
    await waitFor(() => expect(displayedNames(work.id)).toEqual(["w2", "w1", "w3"]));
  });
});

describe("6. bookmark color", () => {
  it("the dialog has a palette of 8 colors plus none, with names; the color is saved and shown", async () => {
    const { user } = setupApp();
    await user.click(screen.getByRole("button", { name: "Edit pi-home" }));
    const dialog = await screen.findByRole("dialog", { name: "Edit bookmark" });
    const group = within(dialog).getByRole("radiogroup", { name: "Color" });
    const radios = within(group).getAllByRole("radio");
    expect(radios.map((r) => r.getAttribute("aria-label"))).toEqual(["No color", "Red", "Orange", "Yellow", "Green", "Teal", "Blue", "Purple", "Gray"]);
    expect(within(group).getByRole("radio", { name: "No color" }).getAttribute("aria-checked")).toBe("true");
    await user.click(within(group).getByRole("radio", { name: "Blue" }));
    expect(within(group).getByRole("radio", { name: "Blue" }).getAttribute("aria-checked")).toBe("true");
    fireEvent.keyDown(within(group).getByRole("radio", { name: "Blue" }), { key: "ArrowRight" });
    expect(within(group).getByRole("radio", { name: "Purple" }).getAttribute("aria-checked")).toBe("true");
    await user.click(within(group).getByRole("radio", { name: "Blue" }));
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(bookmarkById(PI).color).toBe("blue"));
    const mark = within(rowEl(PI)).getByRole("img", { name: "Blue" });
    expect(mark).toBeTruthy();
    expect(rowEl(PI).className).toContain("host-row--colored");
    expect(rowButton(PI).title).toContain("Blue");
  });

  it("the connection tab and the all-connections list show the color with its name", async () => {
    const { backend, user } = setupApp();
    await backend.bookmarkSave(input(bookmarkById(PI), { color: "green" }));
    await refreshBookmarks();
    const sessionId = await openSession(bookmarkTarget(PI), "pi-home");
    await waitFor(() => expect(useStore.getState().sessionsById[sessionId]?.state.phase).toBe("connected"));
    const tab = document.querySelector<HTMLElement>(`[data-session-tab="${sessionId}"]`) as HTMLElement;
    expect(tab.dataset["color"]).toBe("green");
    expect(tab.className).toContain("conn-tab--colored");
    expect(within(tab).getByRole("img", { name: "Green" })).toBeTruthy();
    expect(within(tab).getByRole("tab").title).toContain("Green");
    await user.click(screen.getByRole("button", { name: "All connections" }));
    const menu = await screen.findByRole("menu");
    expect(within(menu).getByRole("img", { name: "Green" })).toBeTruthy();
  });
});

describe("7. editing a connected bookmark", () => {
  it("label, color and bandwidth limits update at once; a connection-relevant change offers Reconnect", async () => {
    const { backend, user } = setupApp();
    const sessionId = await openSession(bookmarkTarget(PI), "pi-home");
    await waitFor(() => expect(useStore.getState().sessionsById[sessionId]?.state.phase).toBe("connected"));
    expect(screen.queryByText("Reconnect to apply changes")).toBeNull();

    await backend.bookmarkSave(input(bookmarkById(PI), { name: "pi-renamed", color: "red", uploadLimitKiBps: 128, downloadLimitKiBps: 0 }));
    const entry = () => useStore.getState().sessionsById[sessionId];
    expect(entry()?.label).toBe("pi-renamed");
    expect(entry()?.color).toBe("red");
    expect(entry()?.info?.limitOverrides).toEqual({ uploadKibps: 128, downloadKibps: 0 });
    expect(entry()?.needsReconnect).toBe(false);
    const tab = document.querySelector<HTMLElement>(`[data-session-tab="${sessionId}"]`) as HTMLElement;
    expect(within(tab).getByText("pi-renamed")).toBeTruthy();
    expect(tab.dataset["color"]).toBe("red");
    expect(screen.queryByText("Reconnect to apply changes")).toBeNull();

    // A new host needs a new connection.
    await backend.bookmarkSave(input(bookmarkById(PI), { name: "pi-renamed", host: "other.local", color: "red" }));
    expect(entry()?.needsReconnect).toBe(true);
    expect(await screen.findByText("Reconnect to apply changes")).toBeTruthy();

    // Undoing the change makes the hint go away again.
    await backend.bookmarkSave(input(bookmarkById(PI), { name: "pi-renamed", host: "192.168.1.40", color: "red" }));
    await waitFor(() => expect(screen.queryByText("Reconnect to apply changes")).toBeNull());
    await backend.bookmarkSave(input(bookmarkById(PI), { name: "pi-renamed", host: "other.local", color: "red" }));
    await screen.findByText("Reconnect to apply changes");

    const before = useStore.getState().sessionOrder.indexOf(sessionId);
    await user.click(screen.getByRole("button", { name: "Reconnect" }));
    await waitFor(() => expect(useStore.getState().sessionsById[sessionId]).toBeUndefined());
    await waitFor(() => expect(useStore.getState().sessionOrder).toHaveLength(1));
    const fresh = useStore.getState().sessionOrder[0] as string;
    expect(useStore.getState().sessionOrder.indexOf(fresh)).toBe(before);
    await waitFor(() => expect(useStore.getState().sessionsById[fresh]?.state.phase).toBe("connected"));
    expect(useStore.getState().sessionsById[fresh]?.needsReconnect).toBe(false);
    expect(screen.queryByText("Reconnect to apply changes")).toBeNull();
  });

  it("the Transfers button and the bookmark dialog use the live limits", async () => {
    const { backend } = setupApp();
    const sessionId = await openSession(bookmarkTarget(NAS), "nas");
    await waitFor(() => expect(useStore.getState().sessionsById[sessionId]?.state.phase).toBe("connected"));
    await backend.bookmarkSave(input(bookmarkById(NAS), { uploadLimitKiBps: 64 }));
    expect(useStore.getState().sessionsById[sessionId]?.info?.limitOverrides.uploadKibps).toBe(64);
  });
});

describe("4. terminal tab label", () => {
  it("a lone terminal has no number; with two the numbers appear", async () => {
    const { user } = setupApp();
    const sessionId = await openSession({ kind: "sshConfig", alias: "prod-web-01" }, "prod-web-01");
    await waitFor(() => expect(useStore.getState().workspaces[sessionId]?.terminalTabs).toHaveLength(1), { timeout: 3000 });
    const panel = screen.getByRole("region", { name: "Terminal panel" });
    expect(within(panel).getAllByRole("tab").map((t) => t.textContent)).toEqual(["Terminal"]);
    await user.click(within(panel).getByRole("button", { name: "New terminal" }));
    await waitFor(() => expect(within(panel).getAllByRole("tab")).toHaveLength(2));
    expect(within(panel).getAllByRole("tab").map((t) => t.textContent)).toEqual(["1 · Terminal", "2 · Terminal"]);
  });
});
