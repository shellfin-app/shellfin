import { useTranslation } from "react-i18next";
import { Checkbox, Select, Setting } from "../../components/Field";
import type { DateFormat, DoubleClickAction, SizeUnits } from "../../ipc/types";
import { AUTO_REFRESH_CHOICES } from "../../lib/settingsForm";
import type { SectionProps } from "./settingsFields";

export function FilesSection({ form, patch }: SectionProps) {
  const { t } = useTranslation();
  return (
    <fieldset className="settings-group">
      <legend>{t("settings.files")}</legend>
      <Checkbox label={t("settings.showHidden")} checked={form.showHidden} onChange={(showHidden) => patch({ showHidden })} />
      <Checkbox label={t("settings.foldersFirst")} checked={form.foldersFirst} onChange={(foldersFirst) => patch({ foldersFirst })} />
      <Checkbox label={t("settings.confirmDelete")} checked={form.confirmDelete} onChange={(confirmDelete) => patch({ confirmDelete })} />
      <Setting label={t("settings.doubleClick")}>
        <Select value={form.doubleClick} onChange={(e) => patch({ doubleClick: e.target.value as DoubleClickAction })}>
          <option value="open">{t("settings.doubleClickOpen")}</option>
          <option value="download">{t("settings.doubleClickDownload")}</option>
        </Select>
      </Setting>
      <Setting label={t("settings.sizeUnits")} info={t("settings.sizeUnitsInfo")}>
        <Select value={form.sizeUnits} onChange={(e) => patch({ sizeUnits: e.target.value as SizeUnits })}>
          <option value="binary">{t("settings.sizeUnitsBinary")}</option>
          <option value="decimal">{t("settings.sizeUnitsDecimal")}</option>
        </Select>
      </Setting>
      <Setting label={t("settings.dateFormat")}>
        <Select value={form.dateFormat} onChange={(e) => patch({ dateFormat: e.target.value as DateFormat })}>
          <option value="relative">{t("settings.dateFormatRelative")}</option>
          <option value="absolute">{t("settings.dateFormatAbsolute")}</option>
        </Select>
      </Setting>
      <Setting label={t("settings.autoRefresh")} info={t("settings.autoRefreshInfo")}>
        <Select value={form.autoRefresh} onChange={(e) => patch({ autoRefresh: e.target.value })}>
          {AUTO_REFRESH_CHOICES.map((s) => (
            <option key={s} value={String(s)}>
              {s === 0 ? t("settings.autoRefreshOff") : t("settings.autoRefreshSeconds", { seconds: s })}
            </option>
          ))}
        </Select>
      </Setting>
      <Checkbox
        label={t("settings.showPermissions")}
        info={t("settings.showPermissionsInfo")}
        checked={form.showPermissions}
        onChange={(showPermissions) => patch({ showPermissions })}
      />
      <Checkbox
        label={t("settings.fileColumns")}
        info={t("settings.fileColumnsInfo")}
        checked={form.fileColumns}
        onChange={(fileColumns) => patch({ fileColumns })}
      />
    </fieldset>
  );
}
