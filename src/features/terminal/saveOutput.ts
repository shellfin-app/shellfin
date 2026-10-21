// Terminal > Save output…: the backend shows the native Save dialog and writes the file; the page only sends
// the text. No path ever comes from the page.
import { t } from "../../i18n/translate";
import { getBackend } from "../../ipc";
import { toIpcError } from "../../ipc/call";
import { OUTPUT_CANCELLED, OUTPUT_TOO_LARGE, outputSavedNotice } from "../../lib/notices";
import { useStore } from "../../state/store";
import { loadedTerminalRegistry } from "./terminalLoader";

/** The backend refuses more than 64 MiB of text; check first so the user gets a plain message. */
const MAX_BYTES = 64 * 1024 * 1024;
const DEFAULT_NAME = "terminal-output.txt";

/** Serializes nothing itself: `text` is the plain-text buffer (see `terminalText`). Resolves when the user decided. */
export async function saveTerminalText(text: string): Promise<void> {
  const store = useStore.getState();
  const contents = text.endsWith("\n") ? text : `${text}\n`;
  if (new TextEncoder().encode(contents).length > MAX_BYTES) {
    store.pushInfo(OUTPUT_TOO_LARGE);
    return;
  }
  try {
    const outcome = await getBackend().appSaveTextFile(DEFAULT_NAME, contents, t("terminal.saveOutputTitle"));
    store.pushInfo(outcome.saved ? outputSavedNotice(outcome.fileName ?? DEFAULT_NAME) : OUTPUT_CANCELLED);
  } catch (e) {
    // The backend's own size limit is reported as an invalid argument: say "too large" instead of a generic error.
    const error = toIpcError(e).error;
    if (error.code === "invalidArgument" && error.params.field === "contents") store.pushInfo(OUTPUT_TOO_LARGE);
    else throw e;
  }
}

/** "Save output…" for a specific terminal (menu bar action, terminal area menu and terminal tab menu share this). */
export async function saveOutputOf(termId: string): Promise<void> {
  const text = loadedTerminalRegistry()?.terminalText(termId);
  if (text !== undefined) await saveTerminalText(text);
}
