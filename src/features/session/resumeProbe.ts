// Detects "the machine was asleep / the network came back" and asks the backend to probe every connected session
// (docs/ARCHITECTURE.md §5.7: `session_probe_all` on `online` and when the window becomes visible after > 60 s
// hidden). Two probes never run closer than MIN_GAP_MS; sessions that are fine only cost one ping.
import { getBackend } from "../../ipc";
import { useStore } from "../../state/store";

export const HIDDEN_PROBE_AFTER_MS = 60_000;
/** Timer gap that can only mean the process was suspended (timers do not run while the machine sleeps). */
export const SUSPEND_GAP_MS = 30_000;
const HEARTBEAT_MS = 5_000;
const MIN_GAP_MS = 10_000;

export interface ResumeProbeOptions {
  now?: () => number;
  win?: Window;
  doc?: Document;
}

function hasLiveSession(): boolean {
  const { sessionOrder, sessionsById } = useStore.getState();
  return sessionOrder.some((id) => {
    const phase = sessionsById[id]?.state.phase;
    return phase === "connected";
  });
}

/** Installs the listeners; returns the uninstall function. */
export function installResumeProbe(options: ResumeProbeOptions = {}): () => void {
  const now = options.now ?? Date.now;
  const win = options.win ?? window;
  const doc = options.doc ?? document;
  let lastProbe = Number.NEGATIVE_INFINITY;
  let hiddenAt: number | null = null;
  let lastBeat = now();

  const probe = () => {
    const t = now();
    if (t - lastProbe < MIN_GAP_MS || !hasLiveSession()) return;
    lastProbe = t;
    getBackend()
      .sessionProbeAll()
      .catch(() => undefined); // best effort: a failing probe must never surface as an error
  };

  const onOnline = () => probe();
  const onVisibility = () => {
    if (doc.visibilityState === "hidden") {
      hiddenAt = now();
      return;
    }
    const wasHidden = hiddenAt;
    hiddenAt = null;
    if (wasHidden !== null && now() - wasHidden > HIDDEN_PROBE_AFTER_MS) probe();
  };
  const beat = setInterval(() => {
    const t = now();
    const gap = t - lastBeat;
    lastBeat = t;
    // Background throttling stretches timers too, but only while hidden; the visibility rule covers that case.
    if (gap > SUSPEND_GAP_MS && doc.visibilityState !== "hidden") probe();
  }, HEARTBEAT_MS);

  win.addEventListener("online", onOnline);
  doc.addEventListener("visibilitychange", onVisibility);
  return () => {
    clearInterval(beat);
    win.removeEventListener("online", onOnline);
    doc.removeEventListener("visibilitychange", onVisibility);
  };
}
