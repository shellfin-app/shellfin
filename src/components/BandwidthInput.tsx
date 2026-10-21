import { useTranslation } from "react-i18next";
import { BANDWIDTH_UNITS, type BandwidthField, type BandwidthUnit } from "../lib/bandwidth";
import { FieldGroup, Input, Select } from "./Field";

/** A bandwidth limit: whole number + unit (KiB/s or MiB/s). `0` means unlimited (the caller says so in `hint`). */
export function BandwidthInput({
  label,
  field,
  onChange,
  invalid = false,
  hint,
  info,
  disabled = false,
}: {
  label: string;
  field: BandwidthField;
  onChange: (field: BandwidthField) => void;
  invalid?: boolean;
  hint?: string;
  /** Longer explanation behind a (?) tip. */
  info?: string;
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  return (
    <FieldGroup label={label} info={info} error={invalid ? t("settings.bandwidthInvalid") : null}>
      <span className="bandwidth">
        <Input
          mono
          inputMode="numeric"
          aria-label={label}
          value={field.value}
          disabled={disabled}
          aria-invalid={invalid ? true : undefined}
          onChange={(e) => onChange({ ...field, value: e.target.value })}
        />
        <Select
          aria-label={t("settings.bandwidthUnit")}
          value={field.unit}
          disabled={disabled}
          onChange={(e) => onChange({ ...field, unit: e.target.value as BandwidthUnit })}
        >
          {BANDWIDTH_UNITS.map((unit) => (
            <option key={unit} value={unit}>
              {unit === "KiB" ? t("settings.unitKiB") : t("settings.unitMiB")}
            </option>
          ))}
        </Select>
      </span>
      {hint ? <span className="field__hint">{hint}</span> : null}
    </FieldGroup>
  );
}
