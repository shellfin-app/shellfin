import type { TFunction } from "i18next";
import { displayServerText } from "../../lib/serverText";
import type { SessionEntry } from "../../state/slices/sessions";

/**
 * The name a connection goes by in the UI (tab, tooltip, prompt attribution): a rename wins; otherwise the host
 * label, with " (n)" from the second tab with the same name on. The label is server text and is sanitised first.
 */
export function sessionTitle(entry: Pick<SessionEntry, "tabName" | "label" | "instance">, t: TFunction): string {
  if (entry.tabName !== undefined) return displayServerText(entry.tabName, 256);
  const name = displayServerText(entry.label, 256);
  return entry.instance >= 2 ? t("shell.tabInstance", { name, n: entry.instance }) : name;
}
