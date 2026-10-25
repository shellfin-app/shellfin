// Translated labels for the macOS native menu (ARCHITECTURE §8.2.3). The backend draws the menu in English until this runs,
// and again whenever the UI language changes. Only macOS has a native menu: elsewhere nothing is sent.
import i18next from "i18next";
import { getBackend } from "../../ipc";
import { escapeNativeLabel, nativeMenuTitle } from "../../lib/menuMnemonic";
import { useStore } from "../../state/store";
import { NATIVE_MENU } from "./shellEvents";

const LABEL_MAX = 80;
const TITLES = ["file", "edit", "view", "connection", "terminal", "window", "help"] as const;

/** Display text -> native label: every `&` doubled (muda strips mnemonic markers on macOS too), within the limit. */
function label(text: string): string {
  return escapeNativeLabel(text, LABEL_MAX);
}

/**
 * Item id -> label (the same text the in-app menu bar shows for that action), plus the `menu.<name>` titles. Labels
 * are in muda's syntax: a literal `&` is `&&`.
 */
export function nativeMenuLabels(): Record<string, string> {
  const t = i18next.t.bind(i18next);
  const labels: Record<string, string> = {};
  for (const [id, entry] of Object.entries(NATIVE_MENU)) {
    if (typeof entry === "string") labels[id] = label(t(`action.${entry}`));
  }
  labels["window.nextTab"] = label(t("menubar.nextConnection"));
  labels["window.prevTab"] = label(t("menubar.previousConnection"));
  // macOS menus have no mnemonics: "ファイル(&F)" is shown as "ファイル", "Ay&uda" as "Ayuda", "A && B" as "A & B".
  for (const name of TITLES) labels[`menu.${name}`] = label(nativeMenuTitle(t(`menubar.${name}`)));
  return labels;
}

/** Sends the current labels on macOS. A failure only leaves the English menu in place, so it is not surfaced. */
export async function syncNativeMenuLabels(): Promise<void> {
  if (useStore.getState().platform !== "macos") return;
  try {
    await getBackend().menuSetLabels(nativeMenuLabels());
  } catch {
    // cosmetic: the native menu keeps its previous labels
  }
}
