// Regression gate for Tauri's `freezePrototype: true` (docs/ARCHITECTURE.md §9.1).
//
// Tauri runs `Object.freeze(Object.prototype)` before any page script. jsdom unit tests never do, which hid a
// startup crash in xterm.js. This script loads the BUILT bundle (default: dist-smoke, a `--mode smoke` build
// that uses MockBackend) in jsdom with the prototype frozen, then drives the UI like a user: connect to a host
// (xterm.js + WebGL addon get evaluated and a terminal mounts), open a file (CodeMirror mounts), edit and save it,
// create a file through the tree context menu, start an upload and open the transfers popover, drag a bookmark,
// open the terminal find bar, the command palette (cmdk) and every Settings section, and switch the language to
// Ukrainian (lazy translation chunk). Any uncaught
// exception, unhandled rejection or window error fails the run.
//
// Usage: node scripts/freeze-check.mjs [distDir]      (see `npm run test:freeze`)
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import process from "node:process";
import { pathToFileURL } from "node:url";
import { JSDOM, VirtualConsole } from "jsdom";

const dist = resolve(process.argv[2] ?? "dist-smoke");
const html = readFileSync(resolve(dist, "index.html"), "utf8");
const entry = /<script[^>]+type="module"[^>]+src="([^"]+)"/.exec(html)?.[1];
if (!entry) throw new Error(`no module script in ${dist}/index.html`);

const failures = [];
const fail = (what, e) => failures.push(`${what}: ${e && e.stack ? e.stack.split("\n").slice(0, 3).join(" | ") : String(e)}`);

const virtualConsole = new VirtualConsole();
virtualConsole.on("jsdomError", (e) => {
  if (!/Not implemented/.test(String(e && e.message))) fail("jsdomError", e);
});
const dom = new JSDOM(`<!doctype html><html lang="en"><body><div id="root"></div></body></html>`, {
  url: "http://tauri.localhost/",
  pretendToBeVisual: true,
  virtualConsole,
});
const w = dom.window;
// jsdom-global style: expose every window property that Node does not already define.
for (const k of Object.getOwnPropertyNames(w)) {
  const force = ["window", "document", "navigator", "location", "self", "Event", "EventTarget", "CustomEvent", "MessageEvent"];
  if (k in globalThis && !force.includes(k)) continue;
  // VS Code's scheduler (vendored in xterm) uses postMessage only if globalThis also has addEventListener.
  if (k === "postMessage") continue;
  try {
    globalThis[k] = w[k];
  } catch {
    /* read-only in this Node version */
  }
}
globalThis.window = w;
// Node 21+ has its own `navigator` (read-only, language from the OS). The app picks its UI language from
// `navigator.languages`, so pin the jsdom one (en-US): the run must not depend on the machine's language.
Object.defineProperty(globalThis, "navigator", { value: w.navigator, configurable: true });
globalThis.DOMRect ??= class DOMRect {
  constructor(x = 0, y = 0, width = 0, height = 0) {
    Object.assign(this, { x, y, width, height, top: y, left: x, right: x + width, bottom: y + height });
  }
};
globalThis.self = globalThis;
globalThis.ResizeObserver = class {
  observe() {}
  unobserve() {}
  disconnect() {}
};
w.ResizeObserver = globalThis.ResizeObserver;
w.matchMedia ??= () => ({
  matches: false, addListener() {}, removeListener() {}, addEventListener() {}, removeEventListener() {},
});
globalThis.matchMedia = w.matchMedia;
w.document.fonts ??= { load: async () => [] };
// CodeMirror measures text ranges for its selection layer; jsdom has no layout.
w.Range.prototype.getClientRects ??= () => [];
// cmdk scrolls the active item into view; jsdom has no layout.
w.Element.prototype.scrollIntoView ??= () => {};
w.Range.prototype.getBoundingClientRect ??= () => new globalThis.DOMRect(0, 0, 0, 0);

// The preload helper also waits for the `load` event of every <link rel=stylesheet> of a lazy chunk; jsdom does
// not load subresources, so acknowledge them.
new w.MutationObserver((records) => {
  for (const r of records) {
    for (const n of r.addedNodes) {
      if (n.nodeName === "LINK") setTimeout(() => n.dispatchEvent(new w.Event("load")), 0);
    }
  }
}).observe(w.document.head, { childList: true });

// Vite's dynamic-import preload helper fetch()es chunk URLs relative to a page origin that does not exist here.
globalThis.fetch = async () => new Response("", { status: 200 });

process.on("uncaughtException", (e) => fail("uncaughtException", e));
process.on("unhandledRejection", (e) => fail("unhandledRejection", e));
w.addEventListener("error", (ev) => fail("window error", ev.error ?? ev.message));

Object.freeze(Object.prototype); // what Tauri's freeze_prototype.js does

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function waitFor(what, probe, timeoutMs = 8000) {
  const t0 = Date.now();
  while (Date.now() - t0 < timeoutMs) {
    const v = probe();
    if (v) return v;
    await sleep(50);
  }
  fail("timeout", `waiting for ${what}`);
  return null;
}

try {
  await import(pathToFileURL(resolve(dist, "." + entry)).href);
} catch (e) {
  fail("entry module evaluation", e);
}

const doc = w.document;
try {
if (failures.length === 0) {
  await waitFor("app shell", () => doc.querySelector(".shell"));
  const host = await waitFor("host row", () => doc.querySelector('button[aria-label="Connect to prod-web-01"]'));
  // Bookmark drag (pointer events, a window-level listener set): press a bookmark row, move past the threshold over
  // the list, release. Rows without a bookmark ignore the press, so every row is tried.
  const pointer = (type, target, x, y) =>
    target.dispatchEvent(new w.MouseEvent(type, { bubbles: true, cancelable: true, button: 0, clientX: x, clientY: y }));
  for (const row of doc.querySelectorAll(".host-row__main")) {
    pointer("pointerdown", row, 10, 10);
    pointer("pointermove", w, 10, 40);
    pointer("pointermove", w, 10, 60);
    pointer("pointerup", w, 10, 60);
  }
  await sleep(50);
  // A single click only selects a bookmark row; a double click connects.
  host?.click();
  host?.dispatchEvent(new w.MouseEvent("dblclick", { bubbles: true, cancelable: true }));
  const term = await waitFor("xterm terminal (lazy chunk + addons)", () => doc.querySelector(".xterm-view .terminal-host .xterm"));
  if (term) {
    // Terminal features under a frozen prototype: sanitised paste, find bar (search addon), a second terminal.
    const textarea = term.querySelector("textarea");
    if (textarea) {
      const paste = new w.Event("paste", { bubbles: true, cancelable: true });
      Object.defineProperty(paste, "clipboardData", { value: { getData: () => "echo hi\x1b[201~" } });
      textarea.dispatchEvent(paste);
      textarea.dispatchEvent(
        new w.KeyboardEvent("keydown", { key: "F", code: "KeyF", keyCode: 70, ctrlKey: true, shiftKey: true, bubbles: true, cancelable: true }),
      );
      const input = await waitFor("terminal find bar", () => doc.querySelector(".terminal-find input"));
      if (input) {
        input.value = "prod";
        input.dispatchEvent(new w.Event("input", { bubbles: true }));
      }
      doc.querySelector('.terminal-panel button[aria-label="New terminal"]')?.click();
      await waitFor("second terminal tab", () => doc.querySelectorAll('.terminal-panel [role="tab"]').length === 2);
    }
    const row = await waitFor("file tree row", () =>
      [...doc.querySelectorAll('[role="treeitem"]')].find((b) => b.textContent?.includes("docker-compose.yml")),
    );
    row?.click();
    const press = (target, init) =>
      target.dispatchEvent(new w.KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }));
    const editor = await waitFor("CodeMirror editor", () => doc.querySelector(".code-editor .cm-editor"));
    if (editor) {
      // Edit and save under the frozen prototype. Enter is CodeMirror's own default keymap; Mod+S is the app's shortcut.
      const content = doc.querySelector(".code-editor .cm-content");
      press(content, { key: "Enter", code: "Enter", keyCode: 13 });
      await waitFor("unsaved marker after an edit", () => doc.querySelector(".editor-tab__dirty"));
      press(content, { key: "s", code: "KeyS", keyCode: 83, ctrlKey: true });
      await waitFor("saved (marker gone)", () => !doc.querySelector(".editor-tab__dirty"));
    }

    // File tree operations: context menu -> New file -> naming input -> the row appears (virtualized list, menu store).
    const treeRow = [...doc.querySelectorAll('[role="treeitem"]')].find((b) => b.textContent?.includes("deploy.sh"));
    if (treeRow) {
      treeRow.dispatchEvent(new w.MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 20, clientY: 20 }));
      await waitFor("tree context menu", () => doc.querySelector('[role="menu"]'));
      const newFile = [...doc.querySelectorAll('[role="menuitem"]')].find((b) => b.textContent?.startsWith("New file"));
      newFile?.click();
      const input = await waitFor("naming input", () => doc.querySelector(".file-row__input"));
      if (input) {
        Object.getOwnPropertyDescriptor(w.HTMLInputElement.prototype, "value").set.call(input, "freeze.txt");
        input.dispatchEvent(new w.Event("input", { bubbles: true }));
        press(input, { key: "Enter", code: "Enter", keyCode: 13 });
        await waitFor("created file row", () =>
          [...doc.querySelectorAll('[role="treeitem"]')].find((b) => b.textContent?.includes("freeze.txt")),
        );
      }
    }

    // Transfers: Upload files… (mock picker) -> status-bar button -> popover.
    doc.querySelector('.files-toolbar button[aria-label="Upload files…"]')?.click();
    const transfers = await waitFor("transfers button", () => doc.querySelector(".status-bar .status-bar__button[aria-haspopup]"));
    transfers?.click();
    await waitFor("transfers popover", () => doc.querySelector(".transfers-popover"));
    press(doc.body, { key: "Escape", code: "Escape", keyCode: 27 });

    // Command palette (cmdk, lazy chunk): open with the shortcut, filter, close.
    press(doc.body, { key: "k", code: "KeyK", keyCode: 75, ctrlKey: true });
    const paletteInput = await waitFor("command palette (cmdk)", () => doc.querySelector("[cmdk-input]"));
    if (paletteInput) {
      Object.getOwnPropertyDescriptor(w.HTMLInputElement.prototype, "value").set.call(paletteInput, "settings");
      paletteInput.dispatchEvent(new w.Event("input", { bubbles: true }));
      await waitFor("palette items", () => doc.querySelector("[cmdk-item]"));
      press(paletteInput, { key: "Escape", code: "Escape", keyCode: 27 });
      await waitFor("palette closed", () => !doc.querySelector("[cmdk-input]"));
    }

    // Settings dialog (lazy chunk with every section): switch sections, then a language switch to Ukrainian, which
    // loads the translation chunk lazily and re-renders the whole UI.
    press(doc.body, { key: ",", code: "Comma", keyCode: 188, ctrlKey: true });
    await waitFor("settings dialog", () => doc.querySelector(".settings-layout"));
    for (const section of ["terminal", "editor", "files", "transfers", "connection", "advanced", "appearance", "general"]) {
      doc.querySelector(`#settings-tab-${section}`)?.click();
      await waitFor(`settings section ${section}`, () => doc.querySelector("#settings-panel fieldset, #settings-panel .settings-group"));
    }
    const language = doc.querySelector("#settings-panel select");
    if (language) {
      Object.getOwnPropertyDescriptor(w.HTMLSelectElement.prototype, "value").set.call(language, "uk");
      language.dispatchEvent(new w.Event("change", { bubbles: true }));
      const save = [...doc.querySelectorAll(".dialog button")].find((b) => b.textContent?.trim() === "Save");
      save?.click();
      await waitFor("Ukrainian UI (lazy translation chunk)", () => doc.documentElement.lang === "uk");
      await waitFor("settings dialog closed", () => !doc.querySelector(".settings-layout"));
    }
  }
}
} catch (e) {
  // A script error must fail the run, not leave timers (the app has intervals) keeping the process alive.
  fail("scenario", e);
}
await sleep(300); // let late rejections surface

if (failures.length > 0) {
  for (const sel of [".notices", ".error-boundary", ".terminal-panel__body"]) {
    const el = doc.querySelector(sel);
    if (el) console.error(`[state] ${sel}: ${el.innerHTML.slice(0, 300)}`);
  }
  console.error(`freeze-check FAILED (${dist}):\n  ${failures.join("\n  ")}`);
  process.exit(1);
}
console.log(`freeze-check OK (${dist}): shell, xterm terminal, CodeMirror edit/save, file tree menu, transfers, command palette, Settings and a switch to Ukrainian run with Object.prototype frozen`);
process.exit(0);
