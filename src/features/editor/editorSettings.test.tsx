// Editor settings (ARCHITECTURE §8.11): detect indentation, auto-close brackets, bracket matching, whitespace, folding,
// line endings of new files. All switches apply live (CodeMirror compartments), also to buffers that are not shown.
import { EditorState } from "@codemirror/state";
import { indentUnit } from "@codemirror/language";
import { EditorView } from "@codemirror/view";
import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../../ipc";
import type { Settings } from "../../ipc/types";
import { useStore } from "../../state/store";
import { settleApp, setupConnected } from "../../test/harness";
import { applySettingsToAll, bufferOptionsFromSettings, createBufferState, getBufferState, settingsEffects } from "./bufferRegistry";

afterEach(async () => {
  await settleApp();
  setBackend(null);
  vi.restoreAllMocks();
});

const row = (name: RegExp | string) => screen.getByRole("treeitem", { name });
const view = (): EditorView => {
  const el = document.querySelector<HTMLElement>(".cm-editor");
  const found = el ? EditorView.findFromDOM(el) : null;
  if (!found) throw new Error("no editor view");
  return found;
};

function patchEditor(p: Partial<Settings["editor"]>): void {
  const settings = useStore.getState().settings;
  if (!settings) throw new Error("no settings");
  act(() => useStore.getState().setSettings({ ...settings, editor: { ...settings.editor, ...p } }));
}

async function open(user: Awaited<ReturnType<typeof setupConnected>>["user"], name: string) {
  await user.click(row(name));
  await waitFor(() => expect(document.querySelector(".cm-editor")).toBeTruthy());
}

const defaults = (over: Partial<Settings["editor"]> = {}) => bufferOptionsFromSettings({ ...(over as Settings["editor"]) });

describe("indentation detection", () => {
  const FOUR = "def f():\n    a = 1\n    if a:\n        b = 2\n    return a\n";
  const TABS = "func f() {\n\ta := 1\n\tif a {\n\t\tb := 2\n\t}\n}\n";

  it("uses the file's own indentation when it is clear", () => {
    const four = createBufferState("/a.py", FOUR, defaults({ tabSize: 2 }));
    expect(four.facet(indentUnit)).toBe("    ");
    expect(four.tabSize).toBe(4);
    const tabs = createBufferState("/a.go", TABS, defaults({ tabSize: 2 }));
    expect(tabs.facet(indentUnit)).toBe("\t");
  });

  it("falls back to the settings when the text is ambiguous, or when detection is off", () => {
    const flat = createBufferState("/a.txt", "one\ntwo\n", defaults({ tabSize: 3 }));
    expect(flat.facet(indentUnit)).toBe("   ");
    expect(flat.tabSize).toBe(3);
    const off = createBufferState("/a.py", FOUR, defaults({ tabSize: 2, detectIndentation: false }));
    expect(off.facet(indentUnit)).toBe("  ");
    expect(off.tabSize).toBe(2);
  });

  it("toggling it applies to stored buffers too", () => {
    const state = createBufferState("/a.py", FOUR, defaults({ tabSize: 2, detectIndentation: false }));
    expect(state.facet(indentUnit)).toBe("  ");
    const on = state.update({ effects: settingsEffects(defaults({ tabSize: 2, detectIndentation: true }), state.doc) }).state;
    expect(on.facet(indentUnit)).toBe("    ");
    const back = on.update({ effects: settingsEffects(defaults({ tabSize: 2, detectIndentation: false }), on.doc) }).state;
    expect(back.facet(indentUnit)).toBe("  ");
  });
});

describe("live switches in the editor", () => {
  it("auto-close brackets is off by default and closes brackets and quotes when on", async () => {
    const { user } = await setupConnected();
    await open(user, "README.md");
    const handlers = () => view().state.facet(EditorView.inputHandler);
    const type = (text: string) => {
      const v = view();
      const pos = v.state.selection.main.head;
      let handled = false;
      for (const h of handlers()) {
        if (h(v, pos, pos, text, () => v.state.update({ changes: { from: pos, insert: text } }))) {
          handled = true;
          break;
        }
      }
      return handled;
    };
    // At the end of the document (CodeMirror only closes a bracket before whitespace or a closing character).
    const toEnd = () => act(() => view().dispatch({ selection: { anchor: view().state.doc.length } }));
    toEnd();
    const before = view().state.doc.length;
    expect(type("(")).toBe(false); // off: the editor would insert only the bracket
    expect(view().state.doc.length).toBe(before);

    patchEditor({ autoCloseBrackets: true });
    toEnd();
    expect(type("(")).toBe(true);
    expect(view().state.doc.sliceString(before, before + 2)).toBe("()");

    patchEditor({ autoCloseBrackets: false });
    expect(type("[")).toBe(false);
  });

  it("the fold gutter follows 'Code folding'", async () => {
    const { user } = await setupConnected();
    await open(user, "docker-compose.yml");
    await waitFor(() => expect(document.querySelector(".cm-foldGutter")).toBeTruthy());
    patchEditor({ codeFolding: false });
    await waitFor(() => expect(document.querySelector(".cm-foldGutter")).toBeNull());
    patchEditor({ codeFolding: true });
    await waitFor(() => expect(document.querySelector(".cm-foldGutter")).toBeTruthy());
  });

  it("matching brackets are highlighted only while the switch is on", async () => {
    const { user } = await setupConnected();
    await open(user, "deploy.sh");
    act(() => {
      const v = view();
      v.dispatch({ changes: { from: 0, insert: "(a)\n" }, selection: { anchor: 1 } });
    });
    await waitFor(() => expect(document.querySelector(".cm-matchingBracket")).toBeTruthy());
    patchEditor({ bracketMatching: false });
    await waitFor(() => expect(document.querySelector(".cm-matchingBracket")).toBeNull());
    patchEditor({ bracketMatching: true });
    await waitFor(() => expect(document.querySelector(".cm-matchingBracket")).toBeTruthy());
  });

  it("show whitespace switches live between all, the active line only and none", async () => {
    const { user } = await setupConnected();
    await open(user, "README.md"); // "# app\n\nDeployment notes.\n", cursor on line 1
    const markedLines = () =>
      [...document.querySelectorAll(".cm-highlightSpace")].map((el) => el.closest(".cm-line")?.textContent).sort();
    expect(document.querySelector(".cm-highlightSpace, .cm-highlightTab")).toBeNull();
    patchEditor({ showWhitespace: "all" });
    await waitFor(() => expect(markedLines()).toEqual(["# app", "Deployment notes."]));
    patchEditor({ showWhitespace: "activeLine" });
    await waitFor(() => expect(markedLines()).toEqual(["# app"]));
    const v = view();
    act(() => v.dispatch({ selection: { anchor: v.state.doc.line(3).from } }));
    await waitFor(() => expect(markedLines()).toEqual(["Deployment notes."]));
    patchEditor({ showWhitespace: "none" });
    await waitFor(() => expect(document.querySelector(".cm-highlightSpace")).toBeNull());
  });

  it("a stored boolean from an earlier build still works (true = all, false = none)", () => {
    expect(bufferOptionsFromSettings({ showWhitespace: true as unknown as Settings["editor"]["showWhitespace"] }).showWhitespace).toBe("all");
    expect(bufferOptionsFromSettings({ showWhitespace: false as unknown as Settings["editor"]["showWhitespace"] }).showWhitespace).toBe("none");
    expect(bufferOptionsFromSettings(undefined).showWhitespace).toBe("none");
  });

  it("buffers that are not on screen get the new switches too", async () => {
    const { user, sessionId } = await setupConnected();
    await open(user, "README.md");
    await open(user, "deploy.sh");
    const id = `${sessionId}:/srv/app/README.md`;
    const stored = getBufferState(id);
    expect(stored).toBeTruthy();
    const options = defaults({ autoCloseBrackets: true, showWhitespace: "all" });
    applySettingsToAll(options, `${sessionId}:/srv/app/deploy.sh`);
    const after = getBufferState(id) as EditorState;
    expect(after.facet(EditorView.inputHandler).length).toBeGreaterThan((stored as EditorState).facet(EditorView.inputHandler).length);
  });
});

describe("line endings of new files", () => {
  async function createFile(user: Awaited<ReturnType<typeof setupConnected>>["user"], name: string) {
    await user.pointer({ keys: "[MouseRight]", target: row("app") });
    const menu = await screen.findByRole("menu");
    await user.click(within(menu).getByRole("menuitem", { name: "New file…" }));
    await user.type(await screen.findByRole("textbox", { name: "New file name" }), `${name}{Enter}`);
  }

  it("LF (default): a new file starts with LF", async () => {
    const { user, sessionId } = await setupConnected();
    await createFile(user, "lf.txt");
    await waitFor(() => expect(useStore.getState().buffers[`${sessionId}:/srv/app/lf.txt`]?.load).toBe("ready"));
    expect(useStore.getState().buffers[`${sessionId}:/srv/app/lf.txt`]?.eol).toBe("lf");
  });

  it("CRLF: a new file starts with CRLF and is not marked unsaved; existing files keep their own endings", async () => {
    const { user, sessionId } = await setupConnected();
    patchEditor({ newFileEol: "crlf" });
    await createFile(user, "crlf.txt");
    const id = `${sessionId}:/srv/app/crlf.txt`;
    await waitFor(() => expect(useStore.getState().buffers[id]?.load).toBe("ready"));
    expect(useStore.getState().buffers[id]).toMatchObject({ eol: "crlf", dirty: false, metaDirty: false });

    await open(user, "README.md");
    expect(useStore.getState().buffers[`${sessionId}:/srv/app/README.md`]?.eol).toBe("lf");
  });
});
