// IME composition tracking. An authentication prompt can open while the user is composing text
// with an IME elsewhere (typically in the terminal). Committing that composition (`compositionend` + the
// resulting `input`) must never land in the prompt's field and become an answer. We therefore count
// started compositions; a guarded dialog ignores the events of any composition that started before it opened.

// Number of compositions started so far (a counter, not a clock: two events in the same millisecond still order).
let started = 0;
let installed = false;

function install(): void {
  if (installed || typeof document === "undefined") return;
  installed = true;
  document.addEventListener(
    "compositionstart",
    () => {
      started += 1;
    },
    true,
  );
}

install();

/** How many compositions have started so far. A dialog remembers the value at open time. */
export function compositionCount(): number {
  return started;
}

/** True for events that belong to an IME composition (including its committing `input`). */
export function isCompositionEvent(e: Event): boolean {
  if (e.type.startsWith("composition")) return true;
  if (e instanceof InputEvent) return e.isComposing || e.inputType === "insertCompositionText";
  return false;
}
