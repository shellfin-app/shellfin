import { useId, useRef, type KeyboardEvent } from "react";
import { useTranslation } from "react-i18next";
import type { BookmarkColor } from "../../ipc/types";
import { BOOKMARK_COLORS, colorVar } from "../../lib/bookmarkColor";

/** The picker value: a palette name or "" (no color). */
export type ColorChoice = BookmarkColor | "";

const CHOICES: readonly ColorChoice[] = ["", ...BOOKMARK_COLORS];

/**
 * 8-color palette plus "none" as one radio group (arrow keys move and select, one tab stop). Every swatch carries
 * its color name as label and tooltip: the color is never the only signal.
 */
export function ColorPicker({ value, onChange, disabled }: { value: ColorChoice; onChange: (value: ColorChoice) => void; disabled?: boolean }) {
  const { t } = useTranslation();
  const labelId = useId();
  const refs = useRef<(HTMLButtonElement | null)[]>([]);

  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>, index: number) => {
    const delta = e.key === "ArrowRight" || e.key === "ArrowDown" ? 1 : e.key === "ArrowLeft" || e.key === "ArrowUp" ? -1 : 0;
    if (delta === 0) return;
    e.preventDefault();
    const next = (index + delta + CHOICES.length) % CHOICES.length;
    const choice = CHOICES[next];
    if (choice === undefined) return;
    onChange(choice);
    refs.current[next]?.focus();
  };

  return (
    <div className="field">
      <span id={labelId} className="field__label">
        {t("bookmarkColor.label")}
      </span>
      <div className="color-picker" role="radiogroup" aria-labelledby={labelId}>
        {CHOICES.map((choice, i) => {
          const name = choice === "" ? t("bookmarkColor.none") : t(`bookmarkColor.${choice}`);
          const checked = choice === value;
          return (
            <button
              key={choice || "none"}
              ref={(el) => {
                refs.current[i] = el;
              }}
              type="button"
              role="radio"
              aria-checked={checked}
              aria-label={name}
              title={name}
              disabled={disabled}
              tabIndex={checked ? 0 : -1}
              className={`color-picker__swatch${checked ? " color-picker__swatch--checked" : ""}${choice === "" ? " color-picker__swatch--none" : ""}`}
              style={choice === "" ? undefined : { background: colorVar(choice) }}
              onClick={() => onChange(choice)}
              onKeyDown={(e) => onKeyDown(e, i)}
            />
          );
        })}
      </div>
    </div>
  );
}
