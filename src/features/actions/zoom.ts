// View > Increase / Decrease / Reset font size. The size is a real setting (terminal and editor), so it is saved
// through `settings_update` and applies live (the terminal registry and the editor follow the store).
import { getBackend } from "../../ipc";
import type { Settings } from "../../ipc/types";
import { SEL_EDITOR_AREA, SEL_TERMINAL_AREA } from "../../lib/domIds";
import { reportError } from "../../lib/reportError";
import { useStore } from "../../state/store";

export const FONT_MIN = 8;
export const FONT_MAX = 32;
export const FONT_DEFAULT = 13;

export type ZoomTarget = "terminal" | "editor" | "both";

/** Which font a zoom command changes: the focused terminal or editor, otherwise both. */
export function zoomTargetFor(el: Element | null): ZoomTarget {
  if (el?.closest(SEL_TERMINAL_AREA)) return "terminal";
  if (el?.closest(SEL_EDITOR_AREA)) return "editor";
  return "both";
}

function clamp(size: number): number {
  return Math.min(FONT_MAX, Math.max(FONT_MIN, Math.round(size)));
}

function withSizes(settings: Settings, terminal: number, editor: number): Settings {
  return { ...settings, terminal: { ...settings.terminal, fontSize: terminal }, editor: { ...settings.editor, fontSize: editor } };
}

// Zoom updates are serialised, latest wins: every press changes the size in the store at once; ONE update is in
// flight at a time and carries the newest sizes. An answer never sets the size back while a newer press is pending, and
// a failure rolls back only to the last confirmed sizes when nothing newer exists.
let version = 0;
let flushing = false;
let done: Promise<void> = Promise.resolve();
let confirmed = { terminal: FONT_DEFAULT, editor: FONT_DEFAULT };

async function flush(): Promise<void> {
  try {
    for (;;) {
      const sent = version;
      const snapshot = useStore.getState().settings;
      if (!snapshot) return;
      try {
        const answer = await getBackend().settingsUpdate(snapshot);
        confirmed = { terminal: answer.terminal.fontSize, editor: answer.editor.fontSize };
        const now = useStore.getState().settings;
        useStore
          .getState()
          .setSettings(version !== sent && now ? withSizes(answer, now.terminal.fontSize, now.editor.fontSize) : answer);
      } catch (e) {
        reportError(e);
        const now = useStore.getState().settings;
        if (version === sent && now) useStore.getState().setSettings(withSizes(now, confirmed.terminal, confirmed.editor));
      }
      if (version === sent) return;
    }
  } finally {
    flushing = false; // runs in the same turn as the last version check: no press can slip in between
  }
}

/** `delta` 0 resets to the default size. */
export async function zoomFonts(delta: number, target: ZoomTarget = zoomTargetFor(document.activeElement)): Promise<void> {
  const settings = useStore.getState().settings;
  if (!settings) return;
  const next = (current: number) => (delta === 0 ? FONT_DEFAULT : clamp(current + delta));
  const terminal = target === "editor" ? settings.terminal.fontSize : next(settings.terminal.fontSize);
  const editor = target === "terminal" ? settings.editor.fontSize : next(settings.editor.fontSize);
  if (terminal === settings.terminal.fontSize && editor === settings.editor.fontSize) return;
  if (!flushing) confirmed = { terminal: settings.terminal.fontSize, editor: settings.editor.fontSize };
  version += 1;
  // Optimistic: the text resizes at once, the backend answer (validated settings) replaces it.
  useStore.getState().setSettings(withSizes(settings, terminal, editor));
  if (!flushing) {
    flushing = true;
    done = flush();
  }
  await done;
}
