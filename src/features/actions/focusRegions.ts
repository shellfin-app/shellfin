// Keyboard focus between the main regions of the workbench: sidebar (bookmarks / file tree), editor,
// terminal. F6 / Shift+F6 cycle through the regions that exist; the explicit shortcuts jump to one.
import { SEL_SIDEBAR_HOST_ANY, SEL_SIDEBAR_HOST_CURRENT, SEL_SIDEBAR_TREE } from "../../lib/domIds";
import { focusEditor } from "../editor/editorActions";
import { loadedTerminalRegistry } from "../terminal/terminalLoader";
import { useStore } from "../../state/store";

export type FocusRegion = "sidebar" | "editor" | "terminal";

const ORDER: readonly FocusRegion[] = ["sidebar", "editor", "terminal"];

export function regionOf(el: Element | null): FocusRegion | null {
  if (!el) return null;
  if (el.closest(".sidebar")) return "sidebar";
  if (el.closest(".editor-area")) return "editor";
  if (el.closest(".terminal-panel")) return "terminal";
  return null;
}

function activeWorkspace() {
  const s = useStore.getState();
  return s.activeSessionId ? s.workspaces[s.activeSessionId] : undefined;
}

/** Is there something to focus in this region right now? */
export function regionAvailable(region: FocusRegion): boolean {
  const s = useStore.getState();
  const ws = activeWorkspace();
  switch (region) {
    case "sidebar":
      return s.sidebarOpen;
    case "editor":
      return ws?.activeBuffer != null;
    case "terminal":
      return ws !== undefined && ws.panelOpen && ws.activeTerminal !== null;
  }
}

/** Moves focus into a region (opening it when it is hidden). Returns false when there is nothing to focus. */
export function focusRegion(region: FocusRegion): boolean {
  const s = useStore.getState();
  const ws = activeWorkspace();
  switch (region) {
    case "sidebar": {
      if (!s.sidebarOpen) s.setSidebarOpen(true);
      // The file tree when it is shown, else the bookmark list's current row.
      const tree = document.querySelector<HTMLElement>(SEL_SIDEBAR_TREE);
      const row =
        document.querySelector<HTMLElement>(SEL_SIDEBAR_HOST_CURRENT) ??
        document.querySelector<HTMLElement>(SEL_SIDEBAR_HOST_ANY);
      const target = tree ?? row;
      if (!target) return false;
      target.focus();
      return true;
    }
    case "editor":
      if (!ws?.activeBuffer) return false;
      // The editor is hidden while the terminal panel is maximized: bring it back before focusing it.
      if (ws.panelMaximized && s.activeSessionId) s.setPanelMaximized(s.activeSessionId, false);
      focusEditor();
      return true;
    case "terminal": {
      if (!s.activeSessionId || !ws) return false;
      if (!ws.panelOpen) s.setPanelOpen(s.activeSessionId, true);
      const termId = ws.activeTerminal;
      if (!termId) return false;
      // The terminal view mounts after the panel opens.
      setTimeout(() => loadedTerminalRegistry()?.focusTerminal(termId), 0);
      return true;
    }
  }
}

/** F6 / Shift+F6: the next / previous region that is available, starting from where the focus is now. */
export function cycleRegion(delta: 1 | -1): void {
  const current = regionOf(document.activeElement);
  const start = current ? ORDER.indexOf(current) : -1;
  for (let step = 1; step <= ORDER.length; step += 1) {
    const region = ORDER[(start + delta * step + ORDER.length * 2) % ORDER.length];
    if (region && regionAvailable(region) && focusRegion(region)) return;
  }
}
