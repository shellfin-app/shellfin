// File tree size and date columns, the sort header and the sort actions (ARCHITECTURE §6.2, §8.11). jsdom has no
// container queries, so these tests check the DOM and classes, not what is visible at a given width.
import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { formatBytes, formatFileDateShort } from "../../i18n/format";
import { setBackend } from "../../ipc";
import { resetLayoutPrefs } from "../../state/layoutPrefs";
import { useStore } from "../../state/store";
import { settleApp, setupConnected } from "../../test/harness";
import { ACTIONS, runAction } from "../actions/registry";

afterEach(async () => {
  await settleApp();
  setBackend(null);
  resetLayoutPrefs();
  vi.restoreAllMocks();
});

const row = (name: string) => screen.getByRole("treeitem", { name });
const cell = (el: HTMLElement, column: "size" | "date") => el.querySelector<HTMLElement>(`.file-row__col--${column}`);
/** The entry names of the data folder, in display order. */
const dataOrder = () =>
  screen
    .getAllByRole("treeitem")
    .filter((el) => el.getAttribute("aria-level") === "3" && el.dataset["treePath"]?.startsWith("/srv/app/data/"))
    .map((el) => el.dataset["treePath"]?.slice("/srv/app/data/".length));

function patchFiles(files: Partial<NonNullable<ReturnType<typeof useStore.getState>["settings"]>["files"]>) {
  const s = useStore.getState().settings;
  if (!s) throw new Error("no settings");
  act(() => useStore.getState().setSettings({ ...s, files: { ...s.files, ...files } }));
}

async function openData() {
  const ctx = await setupConnected({ scenario: "fileColumns" });
  await ctx.user.click(row("data"));
  await screen.findByRole("treeitem", { name: "report.csv" });
  return ctx;
}

describe("file tree columns", () => {
  it("cells: size for files only, the compact date for everything with an mtime, nothing for the root", async () => {
    await openData();
    const lookup = (path: string) => useStore.getState().workspaces[useStore.getState().sessionOrder[0]!]?.tree.listings["/srv/app/data"]?.listing.entries.find((e) => e.path === path);
    const report = lookup("/srv/app/data/report.csv")!;
    expect(cell(row("report.csv"), "size")?.textContent).toBe(formatBytes(1_536_000, "binary"));
    expect(cell(row("report.csv"), "date")?.textContent).toBe(formatFileDateShort(report.mtime!, Date.now(), "relative"));
    expect(cell(row("empty.txt"), "size")?.textContent).toBe("0 B");
    // folders and symlinks: no size (a link's own size would mislead), but a date
    expect(cell(row("archive"), "size")?.textContent).toBe("");
    expect(cell(row("archive"), "date")?.textContent).not.toBe("");
    expect(cell(row("latest.csv"), "size")?.textContent).toBe("");
    // the synthetic root row has neither
    const root = screen.getAllByRole("treeitem")[0]!;
    expect(cell(root, "size")?.textContent).toBe("");
    expect(cell(root, "date")?.textContent).toBe("");
    // the cells are hidden from assistive technology; the row name stays the file name
    expect(cell(row("report.csv"), "size")?.getAttribute("aria-hidden")).toBe("true");
    expect(screen.getByRole("tree").classList.contains("file-tree--columns")).toBe(true);
  });

  it("DOM order with columns is the order without them plus the cells: name, permissions, marks, size, date", async () => {
    await openData();
    patchFiles({ showPermissions: true });
    const sessionId = useStore.getState().sessionOrder[0]!;
    const path = "/srv/app/data/report.csv";
    // a dirty buffer for the row (the dirty dot); the tree reads only sessionId, path and dirty
    act(() =>
      useStore.setState((s) => {
        s.buffers["test-dirty"] = { id: "test-dirty", sessionId, path, dirty: true } as never;
      }),
    );
    const order = (el: HTMLElement) =>
      [...el.children]
        .map((c) => c.className.toString())
        .map((c) => (c.includes("file-row__name") ? "name" : c.includes("file-row__meta") ? "perms" : c.includes("file-row__marks") ? "marks" : c.includes("file-row__col--size") ? "size" : c.includes("file-row__col--date") ? "date" : null))
        .filter((c) => c !== null);
    // the dirty dot is an img with a label, so it is part of the row's name
    const report = () => screen.getByRole("treeitem", { name: /^report\.csv/ });
    await waitFor(() => expect(order(report())).toEqual(["name", "perms", "marks", "size", "date"]));
    // the marks slot holds the dot; without columns it is `display: contents`, so the row lays out as before
    expect(report().querySelector(".file-row__marks > .file-row__dirty")).not.toBeNull();
    // rows without a mark still have the (empty) slot, so permissions and cells line up across rows
    expect(row("small.json").querySelector(".file-row__marks")?.children).toHaveLength(0);
    patchFiles({ columns: false });
    expect(order(report())).toEqual(["name", "perms", "marks"]);
  });

  it("the details are the row's description, and the full name leads the tooltip", async () => {
    await openData();
    const long = "a-very-long-file-name-that-does-not-fit-next-to-the-columns.tar.zst";
    const item = row(long); // exact accessible name: no size, date or permissions in it
    const description = document.getElementById(item.getAttribute("aria-describedby") ?? "");
    expect(description?.textContent).toContain(`Size: ${formatBytes(12_345_678, "binary")}`);
    expect(description?.textContent).toContain("Modified:");
    expect(item.getAttribute("title")?.split("\n")[0]).toBe(long);
    expect(item.getAttribute("title")).toContain("Size:");
  });

  it("follows sizeUnits and dateFormat live", async () => {
    await openData();
    patchFiles({ sizeUnits: "decimal", dateFormat: "absolute" });
    expect(cell(row("report.csv"), "size")?.textContent).toBe("1.5 MB");
    const report = useStore.getState().workspaces[useStore.getState().sessionOrder[0]!]?.tree.listings["/srv/app/data"]?.listing.entries.find((e) => e.name === "report.csv");
    expect(cell(row("report.csv"), "date")?.textContent).toBe(formatFileDateShort(report!.mtime!, Date.now(), "absolute"));
  });

  it("the setting off removes the cells and the header", async () => {
    await openData();
    expect(screen.getByRole("toolbar", { name: "Sort files" })).toBeTruthy();
    patchFiles({ columns: false });
    expect(screen.queryByRole("toolbar", { name: "Sort files" })).toBeNull();
    expect(cell(row("report.csv"), "size")).toBeNull();
    expect(screen.getByRole("tree").classList.contains("file-tree--columns")).toBe(false);
    expect(row("report.csv").getAttribute("title")?.split("\n")[0]).not.toBe("report.csv");
  });

  it("header: clicking sorts (new key in its natural direction, again flips it) and the choice is remembered", async () => {
    const { user } = await openData();
    const header = screen.getByRole("toolbar", { name: "Sort files" });
    // the state is in the label only (no aria-pressed on top of it)
    expect(within(header).getByRole("button", { name: "Name, sorted ascending" }).hasAttribute("aria-pressed")).toBe(false);
    expect(dataOrder()).toEqual([
      "archive", "a-very-long-file-name-that-does-not-fit-next-to-the-columns.tar.zst", "dump.sql.gz", "empty.txt", "latest.csv", "report.csv", "skewed.log", "small.json",
    ]);

    await user.click(within(header).getByRole("button", { name: "Sort by size" }));
    expect(useStore.getState().filesSort).toEqual({ key: "size", dir: "desc" });
    expect(within(header).getByRole("button", { name: "Size, sorted descending" })).toBeTruthy();
    // folders stay on top (by name); the link has no size and goes last
    expect(dataOrder()).toEqual([
      "archive", "dump.sql.gz", "a-very-long-file-name-that-does-not-fit-next-to-the-columns.tar.zst", "report.csv", "skewed.log", "small.json", "empty.txt", "latest.csv",
    ]);
    expect(JSON.parse(window.localStorage.getItem("shellfin.layout.v1") ?? "{}").filesSort).toEqual({ key: "size", dir: "desc" });

    await user.click(within(header).getByRole("button", { name: "Size, sorted descending" }));
    expect(useStore.getState().filesSort).toEqual({ key: "size", dir: "asc" });

    await user.click(within(header).getByRole("button", { name: "Sort by date modified" }));
    expect(useStore.getState().filesSort).toEqual({ key: "mtime", dir: "desc" });
    // newest first; the future (clock-skewed) file is the newest
    expect(dataOrder().slice(1, 3)).toEqual(["skewed.log", "empty.txt"]);
  });

  it("header keyboard: one Tab stop (the active sort), arrows and Home/End move between the buttons", async () => {
    const { user } = await openData();
    const header = screen.getByRole("toolbar", { name: "Sort files" });
    const buttons = within(header).getAllByRole("button");
    expect(buttons.map((b) => b.tabIndex)).toEqual([0, -1, -1]);
    act(() => buttons[0]!.focus());
    await user.keyboard("{ArrowRight}");
    expect(document.activeElement).toBe(buttons[1]);
    await user.keyboard("{End}");
    expect(document.activeElement).toBe(buttons[2]);
    await user.keyboard("{ArrowRight}");
    expect(document.activeElement).toBe(buttons[0]);
    await user.keyboard("{ArrowLeft}");
    expect(document.activeElement).toBe(buttons[2]);
    await user.keyboard("{Enter}");
    expect(useStore.getState().filesSort).toEqual({ key: "mtime", dir: "desc" });
    expect(within(header).getAllByRole("button").map((b) => b.tabIndex)).toEqual([-1, -1, 0]);
  });

  it("header keyboard skips a button whose column is hidden at this width (the Tab stop moves to the first shown one)", async () => {
    // jsdom has no layout: report a box for every header button except Modified (hidden between 21em and 29em).
    vi.spyOn(HTMLElement.prototype, "getClientRects").mockImplementation(function (this: HTMLElement) {
      const hidden = this.closest(".file-tree-header__cell--mtime") !== null;
      return (hidden ? [] : [new DOMRect(0, 0, 10, 10)]) as unknown as DOMRectList;
    });
    const { user } = await openData();
    act(() => useStore.getState().setFilesSort({ key: "mtime", dir: "desc" }));
    const header = screen.getByRole("toolbar", { name: "Sort files" });
    const [name, size, modified] = within(header).getAllByRole("button");
    // the active sort's column is hidden: the first shown button is the Tab stop, so Tab still reaches the toolbar
    expect([name!.tabIndex, size!.tabIndex, modified!.tabIndex]).toEqual([0, -1, -1]);
    act(() => name!.focus());
    await user.keyboard("{ArrowRight}");
    expect(document.activeElement).toBe(size);
    await user.keyboard("{ArrowRight}"); // wraps past the hidden one
    expect(document.activeElement).toBe(name);
    await user.keyboard("{End}");
    expect(document.activeElement).toBe(size);
    await user.keyboard("{ArrowLeft}{Home}");
    expect(document.activeElement).toBe(name);
    // choosing a shown column makes it the Tab stop
    await user.click(size!);
    expect(useStore.getState().filesSort).toEqual({ key: "size", dir: "desc" });
    expect(within(header).getAllByRole("button").map((b) => b.tabIndex)).toEqual([-1, 0, -1]);
  });

  it("deep nesting keeps every row's cells (the CSS clips the indent, columns.test.ts)", async () => {
    await openData();
    const sessionId = useStore.getState().sessionOrder[0]!;
    // Nine folder levels below /srv/app/data, each listed and expanded.
    const s = useStore.getState();
    let parent = "/srv/app/data";
    const paths: string[] = [];
    for (let level = 0; level < 9; level += 1) {
      const path = `${parent}/level-${level}`;
      paths.push(path);
      parent = path;
    }
    const entry = (path: string, kind: "dir" | "file") => ({
      name: path.slice(path.lastIndexOf("/") + 1), path, kind, linkTargetKind: kind, size: kind === "file" ? 4096 : null, mtime: Math.floor(Date.now() / 1000) - 60,
      permissions: 0o755, uid: 0, gid: 0, owner: null, group: null, lossyName: false,
    });
    act(() => {
      const ws = s.workspaces[sessionId]!;
      const listings = { ...ws.tree.listings };
      const expanded = { ...ws.tree.expanded };
      const dataListing = listings["/srv/app/data"]!;
      listings["/srv/app/data"] = { ...dataListing, listing: { ...dataListing.listing, entries: [...dataListing.listing.entries, entry(paths[0]!, "dir")] } };
      paths.forEach((path, i) => {
        const child = paths[i + 1];
        listings[path] = { listing: { path, entries: [child ? entry(child, "dir") : entry(`${path}/deep.txt`, "file")], truncated: false }, fetchedAtMs: Date.now(), stale: false };
        expanded[path] = true;
      });
      useStore.setState((state) => {
        const tree = state.workspaces[sessionId]!.tree;
        tree.listings = listings;
        tree.expanded = expanded;
      });
    });
    const deep = await screen.findByRole("treeitem", { name: "deep.txt" });
    expect(deep.getAttribute("aria-level")).toBe("12");
    expect(cell(deep, "size")?.textContent).toBe(formatBytes(4096, "binary"));
    expect(cell(deep, "date")?.textContent).not.toBe("");
    // name first, cells last: the order the flex-end clipping relies on
    const children = [...deep.children].map((c) => c.className.toString());
    expect(children.findIndex((c) => c.includes("file-row__name"))).toBeLessThan(children.findIndex((c) => c.includes("file-row__col--size")));
    expect(children.findIndex((c) => c.includes("file-row__col--size"))).toBe(children.findIndex((c) => c.includes("file-row__col--date")) - 1);
  });

  it("registry actions set the sort (checked state); selection and the active descendant survive a re-sort", async () => {
    const { user } = await openData();
    await user.click(row("small.json"));
    const tree = screen.getByRole("tree");
    const active = tree.getAttribute("aria-activedescendant");
    expect(active).toBe(row("small.json").id);

    const state = () => useStore.getState();
    expect(ACTIONS.sortFilesByName.checked?.(state())).toBe(true);
    act(() => runAction("sortFilesByModified"));
    expect(state().filesSort).toEqual({ key: "mtime", dir: "desc" });
    expect(ACTIONS.sortFilesByModified.checked?.(state())).toBe(true);
    expect(ACTIONS.sortFilesByName.checked?.(state())).toBe(false);
    act(() => runAction("sortFilesByModified"));
    expect(state().filesSort).toEqual({ key: "mtime", dir: "asc" });
    act(() => runAction("sortFilesBySize"));
    expect(state().filesSort).toEqual({ key: "size", dir: "desc" });

    await waitFor(() => expect(dataOrder()[1]).toBe("dump.sql.gz"));
    expect(row("small.json").getAttribute("aria-selected")).toBe("true");
    expect(tree.getAttribute("aria-activedescendant")).toBe(row("small.json").id);
  });

  it("the View menu lists the three sort actions", async () => {
    const { user } = await openData();
    await user.click(screen.getByRole("menuitem", { name: "View" }));
    const sortByName = await screen.findByRole("menuitemcheckbox", { name: /Sort files by name/ });
    expect(sortByName.getAttribute("aria-checked")).toBe("true");
    // the checked one also says the direction; the others do not
    expect(sortByName.textContent).toContain("Ascending");
    const bySize = screen.getByRole("menuitemcheckbox", { name: /Sort files by size/ });
    expect(bySize.getAttribute("aria-checked")).toBe("false");
    expect(bySize.textContent).not.toMatch(/Ascending|Descending/);
    await user.click(screen.getByRole("menuitemcheckbox", { name: /Sort files by date modified/ }));
    expect(useStore.getState().filesSort).toEqual({ key: "mtime", dir: "desc" });
  });
});
