// Terminal bell sound (`terminal.bell = "sound"`): a short tone synthesized with Web Audio. No asset, no network,
// nothing for the CSP to allow. Browsers without Web Audio (and jsdom) simply stay silent.

const FREQUENCY_HZ = 880;
const DURATION_S = 0.12;
const VOLUME = 0.08;
/** A program that rings the bell in a loop must not turn into a buzz. */
const MIN_GAP_MS = 120;

let context: AudioContext | null = null;
let lastBeepAt = Number.NEGATIVE_INFINITY;

interface AudioWindow {
  AudioContext?: typeof AudioContext;
  webkitAudioContext?: typeof AudioContext;
}

/** Plays the beep. Never throws. `now` is injectable for tests. */
export function playBeep(now: number = Date.now()): boolean {
  if (now - lastBeepAt < MIN_GAP_MS) return false;
  try {
    const w = window as unknown as AudioWindow;
    const Ctor = w.AudioContext ?? w.webkitAudioContext;
    if (!Ctor) return false;
    context ??= new Ctor();
    const ctx = context;
    if (ctx.state === "suspended") void ctx.resume().catch(() => undefined);
    const osc = ctx.createOscillator();
    const gain = ctx.createGain();
    const t0 = ctx.currentTime;
    osc.type = "sine";
    osc.frequency.value = FREQUENCY_HZ;
    // Short fade in and out so the tone does not click.
    gain.gain.setValueAtTime(0, t0);
    gain.gain.linearRampToValueAtTime(VOLUME, t0 + 0.01);
    gain.gain.linearRampToValueAtTime(0, t0 + DURATION_S);
    osc.connect(gain);
    gain.connect(ctx.destination);
    osc.start(t0);
    osc.stop(t0 + DURATION_S + 0.02);
    lastBeepAt = now;
    return true;
  } catch {
    return false;
  }
}

/** Test hook: forgets the shared context and the rate limit. */
export function resetBeepForTests(): void {
  context = null;
  lastBeepAt = Number.NEGATIVE_INFINITY;
}
