// "Confirm before connecting" (bookmark field `confirmConnect`): a store-driven dialog; the caller awaits the answer.
// Only user-initiated connects ask. Automatic reconnect and `session_reconnect_now` never do (ARCHITECTURE §8.11).
import { CONFIRM_CONNECT_KIND } from "../../lib/dialogs";
import { useStore } from "../../state/store";

interface Pending {
  resolve: (ok: boolean) => void;
  unsubscribe: () => void;
}

let pending: Pending | null = null;

function take(): Pending | null {
  const p = pending;
  pending = null;
  p?.unsubscribe();
  return p;
}

/**
 * Opens the question for the bookmark called `name`. Resolves `true` when the user wants to connect. The promise
 * always settles: a dialog that is replaced by another one (quit confirmation, settings, a newer question) counts
 * as "no", so a caller such as the startup restore never waits forever.
 */
export async function requestConnectConfirm(name: string, options: { background?: boolean } = {}): Promise<boolean> {
  // A question nobody asked for right now (startup restore) never replaces a dialog the user has open: it waits
  // until the screen is free. No UI is opened by this wait itself.
  if (options.background === true) await whenNoDialog();
  return askConnectConfirm(name);
}

function whenNoDialog(): Promise<void> {
  if (useStore.getState().dialog === null) return Promise.resolve();
  return new Promise<void>((resolve) => {
    const unsubscribe = useStore.subscribe((s) => {
      if (s.dialog !== null) return;
      unsubscribe();
      resolve();
    });
  });
}

function askConnectConfirm(name: string): Promise<boolean> {
  take()?.resolve(false); // a newer question supersedes an unanswered one
  return new Promise<boolean>((resolve) => {
    useStore.getState().openDialog({ kind: CONFIRM_CONNECT_KIND, name });
    const shown = useStore.getState().dialog;
    // openDialog is ignored while the quit confirmation owns the screen.
    if (shown?.kind !== CONFIRM_CONNECT_KIND) {
      resolve(false);
      return;
    }
    const unsubscribe = useStore.subscribe((s) => {
      if (s.dialog !== shown) take()?.resolve(false);
    });
    pending = { resolve, unsubscribe };
  });
}

export function answerConnectConfirm(ok: boolean): void {
  const p = take();
  useStore.getState().closeDialog(CONFIRM_CONNECT_KIND);
  p?.resolve(ok);
}
