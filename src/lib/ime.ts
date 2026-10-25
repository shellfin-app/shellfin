// IME composition tracking. An authentication prompt can open while the user is composing text
// with an IME elsewhere (typically in the terminal). Committing that composition (`compositionend` + the
// resulting `input`) must never land in the prompt's field and become an answer. We therefore count
// started compositions; a guarded dialog ignores the events of any composition that started before it opened.

// Number of compositions started so far (a counter, not a clock: two events in the same millisecond still order).
let started = 0;
let installed = false;
/** A composition is running (between `compositionstart` and `compositionend`). */
let composing = false;
/** `performance.now()` of the last `compositionend`, or -Infinity. */
let endedAt = Number.NEGATIVE_INFINITY;

/**
 * How long after `compositionend` a `keyCode` 229 key still belongs to the IME: WKWebView delivers the keydown of the
 * Enter that confirmed a conversion right after `compositionend`, with `isComposing` false and `keyCode` 229.
 */
export const IME_CONFIRM_WINDOW_MS = 100;

function install(): void {
  if (installed || typeof document === "undefined") return;
  installed = true;
  document.addEventListener(
    "compositionstart",
    () => {
      started += 1;
      composing = true;
    },
    true,
  );
  document.addEventListener(
    "compositionend",
    () => {
      composing = false;
      endedAt = performance.now();
    },
    true,
  );
}

install();

/** How many compositions have started so far. A dialog remembers the value at open time. */
export function compositionCount(): number {
  return started;
}

/**
 * True for a key event that the IME handles: a key pressed while a composition is active (`isComposing`), or a
 * `keyCode` 229 ("Process") key while a composition is running or within `IME_CONFIRM_WINDOW_MS` after it ended
 * (WKWebView's confirming Enter). A 229 key outside a composition is an ordinary key: on Linux, IBus and Fcitx5 can
 * route every key through the IME and report 229 for all of them, and Escape, shortcuts, Alt+letter and F10 must
 * keep working there. Enter and Escape handlers on text fields, dialogs and global shortcuts ignore IME keys, so
 * confirming a conversion never submits and cancelling it never closes (ARCHITECTURE §8.7). Takes a DOM event or a
 * React synthetic one.
 */
export function isImeKeyEvent(e: KeyboardEvent | { nativeEvent: KeyboardEvent }): boolean {
  const ev = "nativeEvent" in e ? e.nativeEvent : e;
  if (ev.isComposing) return true;
  if (ev.keyCode !== 229) return false;
  return composing || performance.now() - endedAt <= IME_CONFIRM_WINDOW_MS;
}

/** True for events that belong to an IME composition (including its committing `input`). */
export function isCompositionEvent(e: Event): boolean {
  if (e.type.startsWith("composition")) return true;
  if (e instanceof InputEvent) return e.isComposing || e.inputType === "insertCompositionText";
  return false;
}
