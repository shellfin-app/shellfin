// Building blocks shared by the Settings sections. One setting per row; help text is at most one short line (`hint`),
// anything longer sits behind a (?) tip (`info`).
import type { CSSProperties, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { formatBytes } from "../../i18n/format";
import { Input, Select, Setting } from "../../components/Field";
import { FONT_FAMILY_MAX, terminalFontStack, validateFontFamily } from "../../lib/fontFamily";
import {
  SCROLLBACK_MEMORY_HINT_FROM,
  SETTINGS_RANGES,
  scrollbackMemoryMiB,
  type SettingsFormState,
} from "../../lib/settingsForm";
import type { AppearanceForm } from "../../lib/settingsFormAppearance";

/** What every section receives from the dialog, which owns the one form model. */
export interface SectionProps {
  form: SettingsFormState;
  /** Merges a partial update into the form (functional, so quick successive edits never overwrite each other). */
  patch: (p: Partial<SettingsFormState>) => void;
  /** Sets one integer field (kept as text while editing; `SETTINGS_RANGES` holds its range). */
  setNumber: (path: string, value: string) => void;
  /** Merges a partial update into the appearance part of the form (theme, accent, size, terminal background). */
  patchLook: (p: Partial<AppearanceForm>) => void;
  /** Field paths with an unusable value after a failed save. */
  invalid: readonly string[];
}

const CUSTOM = "\u0000custom";

/** Integer field. The allowed range shows when the value is wrong. */
export function NumberField({ path, label, form, invalid, setNumber }: SectionProps & { path: string; label: string }) {
  const { t } = useTranslation();
  const rule = SETTINGS_RANGES[path];
  const bad = invalid.includes(path);
  return (
    <Setting label={label} error={bad && rule ? t("settings.rangeInvalid", { min: rule.min, max: rule.max }) : null}>
      <Input
        mono
        inputMode="numeric"
        value={form.numbers[path] ?? ""}
        aria-invalid={bad ? true : undefined}
        onChange={(e) => setNumber(path, e.target.value)}
      />
    </Setting>
  );
}

/** What the font pickers need besides the form: the installed fonts and whether "Custom…" is chosen. */
export interface FontPickerProps {
  installed: string[];
  custom: boolean;
  setCustom: (custom: boolean) => void;
}

/**
 * Font picker shared by the terminal and the editor: bundled / installed monospace fonts / free text (validated by
 * `lib/fontFamily.ts`). `custom` is owned by the dialog (reset clears it, and it survives a section switch).
 */
export function FontFamilyField({
  value,
  onChange,
  installed,
  custom,
  setCustom,
  invalid,
}: FontPickerProps & {
  value: string | null;
  onChange: (value: string | null) => void;
  invalid: boolean;
}) {
  const { t } = useTranslation();
  const error = custom && value !== null ? validateFontFamily(value) : null;
  const selected = custom ? CUSTOM : (value ?? "");
  return (
    <>
      <Setting label={t("settings.fontFamily")}>
        <Select
          value={selected}
          onChange={(e) => {
            const v = e.target.value;
            if (v === CUSTOM) {
              setCustom(true);
              onChange(value ?? "");
            } else {
              setCustom(false);
              onChange(v === "" ? null : v);
            }
          }}
        >
          <option value="">{t("settings.fontBundled")}</option>
          {installed.map((name) => (
            <option key={name} value={name}>
              {name}
            </option>
          ))}
          <option value={CUSTOM}>{t("settings.fontCustom")}</option>
        </Select>
      </Setting>
      {custom ? (
        <Setting
          label={t("settings.fontCustomName")}
          hint={t("settings.fontCustomHint")}
          error={error !== null || invalid ? t(`settings.fontError.${error ?? "badChars"}`) : null}
        >
          <Input
            mono
            value={value ?? ""}
            maxLength={FONT_FAMILY_MAX + 16}
            aria-invalid={error !== null || invalid ? true : undefined}
            onChange={(e) => onChange(e.target.value)}
          />
        </Setting>
      ) : null}
    </>
  );
}

/** Neutral sample text (not terminal-looking): letters, look-alike glyphs and code punctuation. */
const SAMPLE = "The quick brown fox jumps over the lazy dog — 0O 1lI {}[] => != ;:";

/**
 * A live sample in the chosen size and family, so they can be judged before saving. `size` is raw field text;
 * `family` is the font text (an invalid one previews as the bundled font).
 */
export function FontPreview({
  size,
  label,
  family,
  ligatures = false,
}: {
  size: string | undefined;
  label: string;
  family?: string | null;
  /** Editor preview only: the terminal never uses ligatures. */
  ligatures?: boolean;
}) {
  const px = Number(size);
  const valid = Number.isFinite(px) && px >= 8 && px <= 40;
  const style: CSSProperties = {};
  if (valid) style.fontSize = `${px}px`;
  style.fontFamily = terminalFontStack(family);
  style.fontVariantLigatures = ligatures ? "normal" : "none";
  return (
    <pre className="settings-preview mono" aria-label={label} style={style}>
      {SAMPLE}
    </pre>
  );
}

/** The family text, or `null` (= the bundled font) when it is not usable. */
export function usableFontFamily(family: string | null): string | null {
  return family !== null && validateFontFamily(family) !== null ? null : family;
}

/** Scrollback memory hint (above 100 000 lines). */
export function ScrollbackHint({ lines }: { lines: number }): ReactNode {
  const { t } = useTranslation();
  if (!Number.isFinite(lines) || lines <= SCROLLBACK_MEMORY_HINT_FROM) return null;
  const size = formatBytes(scrollbackMemoryMiB(lines) * 1024 * 1024);
  return (
    <p className="field__hint" role="status">
      {t("settings.scrollbackMemory", { size })}
    </p>
  );
}
