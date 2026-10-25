import { useEffect, useId, useRef, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { IconButton } from "./IconButton";
import { AUTOFOCUS_SELECTOR, FOCUSABLE_SELECTOR } from "../lib/dom";
import { compositionCount, isCompositionEvent, isImeKeyEvent } from "../lib/ime";
import "./dialog.css";

/** Open dialogs, topmost last: only the topmost one traps Tab and handles Escape. */
const stack: symbol[] = [];

export interface DialogProps {
  /** Already translated title (also the accessible name). */
  title: string;
  /** Optional line under the title (mono): paths, endpoints ... */
  subtitle?: ReactNode;
  /** Header icon slot (decorative). */
  icon?: ReactNode;
  tone?: "default" | "warn" | "danger";
  role?: "dialog" | "alertdialog";
  /** Escape key / close button. Omit for dialogs that must be answered explicitly. */
  onClose?: () => void;
  /** Width in px. */
  width?: number;
  /** Fixed height in px (still capped by the viewport); omit to size to the content. */
  height?: number;
  children: ReactNode;
  footer?: ReactNode;
  /** Re-focus the dialog when this value changes (e.g. repeated quit requests). */
  refocusKey?: number;
  /**
   * Ignore keyboard input for this many ms after the dialog opens. Used for authentication prompts that can
   * appear while the user is typing elsewhere: stray keystrokes must never become an answer.
   */
  inputGuardMs?: number;
  /** Header close button (✕) next to the title; needs `onClose`. */
  showClose?: boolean;
}

/** Modal dialog with focus trap, Escape handling and focus restore (visible focus comes from global.css). */
export function Dialog({
  title,
  subtitle,
  icon,
  tone = "default",
  role = "dialog",
  onClose,
  width = 560,
  height,
  children,
  footer,
  refocusKey,
  inputGuardMs = 0,
  showClose = false,
}: DialogProps) {
  const { t } = useTranslation();
  const titleId = useId();
  const rootRef = useRef<HTMLDivElement>(null);
  const closeRef = useRef(onClose);
  const guardUntil = useRef(0);
  useEffect(() => {
    closeRef.current = onClose;
  });

  useEffect(() => {
    const token = Symbol("dialog");
    stack.push(token);
    guardUntil.current = inputGuardMs > 0 ? Date.now() + inputGuardMs : 0;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const root = rootRef.current;
    const first = root?.querySelector<HTMLElement>(AUTOFOCUS_SELECTOR) ?? root?.querySelector<HTMLElement>(FOCUSABLE_SELECTOR);
    first?.focus();

    const swallowDuringGuard = (e: KeyboardEvent) => {
      if (stack[stack.length - 1] !== token || Date.now() >= guardUntil.current) return;
      e.preventDefault();
      e.stopImmediatePropagation();
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (stack[stack.length - 1] !== token || !root) return;
      if (e.key === "Tab") {
        const items = [...root.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)];
        const firstItem = items[0];
        const lastItem = items[items.length - 1];
        if (!firstItem || !lastItem) return;
        if (e.shiftKey && document.activeElement === firstItem) {
          e.preventDefault();
          lastItem.focus();
        } else if (!e.shiftKey && document.activeElement === lastItem) {
          e.preventDefault();
          firstItem.focus();
        } else if (!root.contains(document.activeElement)) {
          e.preventDefault();
          firstItem.focus();
        }
      }
    };
    // Escape runs in the bubble phase, after React's handlers (they listen on #root): inner widgets (InfoTip, context
    // menus, selects, comboboxes) get it first and call preventDefault(). Only an unhandled Escape closes the dialog.
    const onEscape = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented || stack[stack.length - 1] !== token || !root || !closeRef.current) return;
      if (isImeKeyEvent(e)) return; // Escape cancels the IME conversion in a field, not the dialog
      const target = e.target;
      if (target instanceof Node && target !== document.body && target !== document.documentElement && !root.contains(target)) return;
      e.preventDefault();
      e.stopPropagation();
      closeRef.current();
    };
    // IME: the commit of a composition that started BEFORE this dialog opened (typically in the terminal underneath)
    // must not land in the dialog's field and become an answer. Unlike the key guard this is not time-bound: any
    // composition event aimed at the dialog whose composition began earlier is dropped, and the field value that
    // the browser already changed is put back. Compositions started inside the dialog are the user's own input.
    const compositionsAtOpen = compositionCount();
    const valueBefore = new WeakMap<EventTarget, string>();
    const recordValue = (e: Event) => {
      const el = e.target;
      if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) valueBefore.set(el, el.value);
    };
    const swallowStaleComposition = (e: Event) => {
      if (inputGuardMs <= 0 || stack[stack.length - 1] !== token || !root) return;
      if (!(e.target instanceof Node) || !root.contains(e.target) || !isCompositionEvent(e)) return;
      if (compositionCount() > compositionsAtOpen) return;
      e.preventDefault();
      e.stopImmediatePropagation();
      const el = e.target;
      const before = valueBefore.get(el);
      if (e.type === "input" && before !== undefined && (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement)) {
        const proto = el instanceof HTMLInputElement ? HTMLInputElement.prototype : HTMLTextAreaElement.prototype;
        Object.getOwnPropertyDescriptor(proto, "value")?.set?.call(el, before); // bypasses React's value tracker
      }
    };
    const compositionEvents = ["compositionupdate", "compositionend", "beforeinput", "input"] as const;
    document.addEventListener("beforeinput", recordValue, true);
    for (const type of compositionEvents) document.addEventListener(type, swallowStaleComposition, true);
    // Registered first, so it runs before the Escape/Tab handling below.
    document.addEventListener("keydown", swallowDuringGuard, true);
    document.addEventListener("keyup", swallowDuringGuard, true);
    document.addEventListener("keypress", swallowDuringGuard, true);
    document.addEventListener("keydown", onKeyDown, true);
    document.addEventListener("keydown", onEscape);
    return () => {
      document.removeEventListener("keydown", onEscape);
      document.removeEventListener("keydown", swallowDuringGuard, true);
      document.removeEventListener("keyup", swallowDuringGuard, true);
      document.removeEventListener("keypress", swallowDuringGuard, true);
      document.removeEventListener("keydown", onKeyDown, true);
      document.removeEventListener("beforeinput", recordValue, true);
      for (const type of compositionEvents) document.removeEventListener(type, swallowStaleComposition, true);
      const i = stack.indexOf(token);
      if (i >= 0) stack.splice(i, 1);
      if (previous?.isConnected) previous.focus();
    };
  // eslint-disable-next-line react-hooks/exhaustive-deps -- the guard is armed once, when the dialog opens
  }, []);

  useEffect(() => {
    if (refocusKey === undefined) return;
    const root = rootRef.current;
    (root?.querySelector<HTMLElement>(AUTOFOCUS_SELECTOR) ?? root?.querySelector<HTMLElement>(FOCUSABLE_SELECTOR))?.focus();
  }, [refocusKey]);

  return (
    <div className="dialog-backdrop">
      <div
        ref={rootRef}
        className={`dialog dialog--${tone}`}
        role={role}
        aria-modal="true"
        aria-labelledby={titleId}
        style={height === undefined ? { width } : { width, height }}
      >
        <div className="dialog__header">
          {icon ? <span className={`dialog__icon dialog__icon--${tone}`}>{icon}</span> : null}
          <div className="dialog__heading">
            <h2 id={titleId} className="dialog__title">
              {title}
            </h2>
            {subtitle ? <span className="dialog__subtitle mono">{subtitle}</span> : null}
          </div>
          {showClose && onClose ? (
            <IconButton className="dialog__close" icon="close" iconSize={14} size={28} label={t("common.close")} onClick={onClose} />
          ) : null}
        </div>
        <div className="dialog__body">{children}</div>
        {footer ? <div className="dialog__footer">{footer}</div> : null}
      </div>
    </div>
  );
}
