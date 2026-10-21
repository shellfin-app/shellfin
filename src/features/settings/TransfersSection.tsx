import { useTranslation } from "react-i18next";
import { BandwidthInput } from "../../components/BandwidthInput";
import { Checkbox, Select, Setting } from "../../components/Field";
import type { ConflictPolicy } from "../../ipc/types";
import { BANDWIDTH_FIELDS, FIELD } from "../../lib/settingsForm";
import { NumberField, type SectionProps } from "./settingsFields";

export function TransfersSection(props: SectionProps) {
  const { t } = useTranslation();
  const { form, patch, invalid } = props;
  return (
    <fieldset className="settings-group">
      <legend>{t("settings.transfers")}</legend>
      <BandwidthInput
        label={t("settings.uploadLimit")}
        field={form.uploadLimit}
        invalid={invalid.includes(BANDWIDTH_FIELDS.UPLOAD)}
        hint={t("settings.bandwidthHint")}
        info={t("settings.bandwidthInfo")}
        onChange={(uploadLimit) => patch({ uploadLimit })}
      />
      <BandwidthInput
        label={t("settings.downloadLimit")}
        field={form.downloadLimit}
        invalid={invalid.includes(BANDWIDTH_FIELDS.DOWNLOAD)}
        hint={t("settings.bandwidthHint")}
        info={t("settings.bandwidthInfo")}
        onChange={(downloadLimit) => patch({ downloadLimit })}
      />
      <NumberField {...props} path={FIELD.MAX_CONCURRENT} label={t("settings.maxConcurrent")} />
      <Setting label={t("settings.onConflict")} info={t("settings.onConflictInfo")}>
        <Select value={form.onConflict} onChange={(e) => patch({ onConflict: e.target.value as ConflictPolicy })}>
          <option value="ask">{t("settings.onConflictAsk")}</option>
          <option value="overwrite">{t("settings.onConflictOverwrite")}</option>
          <option value="skip">{t("settings.onConflictSkip")}</option>
        </Select>
      </Setting>
      <Checkbox
        label={t("settings.preserveTimestamps")}
        info={t("settings.preserveTimestampsInfo")}
        checked={form.preserveTimestamps}
        onChange={(preserveTimestamps) => patch({ preserveTimestamps })}
      />
      <Checkbox
        label={t("settings.notifyOnComplete")}
        checked={form.notifyOnComplete}
        onChange={(notifyOnComplete) => patch({ notifyOnComplete })}
      />
    </fieldset>
  );
}
