import { EditorView } from "@codemirror/view";
import i18next from "i18next";
import { afterEach, describe, expect, it } from "vitest";
import { setLanguage } from "../../i18n";
import { applyLocaleToAll, bufferOptionsFromSettings, createBufferState, dropBuffer, getBufferState, initBuffer, localeEffect } from "./bufferRegistry";

describe("open editors follow the UI language", () => {
  afterEach(async () => {
    dropBuffer("s:/a/b.txt");
    await setLanguage("en");
  });

  it("stored states get the new CodeMirror phrases and accessible name after a language change", async () => {
    const id = "s:/a/b.txt";
    initBuffer(id, createBufferState("/a/b.txt", "x", bufferOptionsFromSettings(undefined)));
    const english = getBufferState(id)?.phrase("Find");
    expect(english).toBe(i18next.t("editor.cm.find"));

    await setLanguage("uk");
    const ukrainian = i18next.t("editor.cm.find");
    expect(ukrainian).not.toBe(english);
    expect(getBufferState(id)?.phrase("Find")).toBe(english); // not yet
    applyLocaleToAll((bufferId) => (bufferId === id ? "/a/b.txt" : null));
    expect(getBufferState(id)?.phrase("Find")).toBe(ukrainian);
  });

  it("a live view is re-worded by the effect, including the content aria-label", async () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const view = new EditorView({ parent: host, state: createBufferState("/a/b.txt", "x", bufferOptionsFromSettings(undefined)) });
    const before = view.contentDOM.getAttribute("aria-label");
    await setLanguage("uk");
    view.dispatch({ effects: localeEffect("/a/b.txt") });
    const after = view.contentDOM.getAttribute("aria-label");
    expect(after).toBe(i18next.t("editor.contentLabel", { name: "b.txt" }));
    expect(after).not.toBe(before);
    view.destroy();
    host.remove();
  });
});
