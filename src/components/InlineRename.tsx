import { useEffect, useRef, type KeyboardEvent } from "react";
import { isImeKeyEvent } from "../lib/ime";

/**
 * Inline name editor for tabs. Enter commits (empty = `null`, i.e. back to the default name), Escape and blur
 * cancel (`undefined`).
 */
export function InlineRename({
  initial,
  label,
  maxLength = 64,
  className,
  onDone,
}: {
  initial: string;
  /** Already translated accessible name. */
  label: string;
  maxLength?: number;
  className?: string;
  onDone: (name: string | null | undefined) => void;
}) {
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    ref.current?.focus();
    ref.current?.select();
  }, []);
  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (isImeKeyEvent(e)) return; // Enter / Escape confirm or cancel the IME conversion, not the rename
    if (e.key === "Enter") {
      e.preventDefault();
      const value = e.currentTarget.value.trim();
      onDone(value === "" ? null : value);
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onDone(undefined);
    }
  };
  return (
    <input
      ref={ref}
      className={className}
      defaultValue={initial}
      maxLength={maxLength}
      aria-label={label}
      onKeyDown={onKeyDown}
      onBlur={() => onDone(undefined)}
    />
  );
}
