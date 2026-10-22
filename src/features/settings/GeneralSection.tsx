import { useTranslation } from "react-i18next";
import { Checkbox, Select, Setting } from "../../components/Field";
import { LANGUAGE_ENDONYMS, SUPPORTED_LANGUAGES } from "../../i18n";
import type { ConfirmQuit, StartupAction } from "../../ipc/types";
import { CONFIRM_QUIT } from "../../lib/identifiers";
import type { SectionProps } from "./settingsFields";

export function GeneralSection({ form, patch }: SectionProps) {
  const { t } = useTranslation();
  return (
    <fieldset className="settings-group">
      <legend>{t("settings.general")}</legend>
      <Setting label={t("settings.language")}>
        <Select value={form.locale} onChange={(e) => patch({ locale: e.target.value })}>
          <option value="system">{t("settings.languageSystem")}</option>
          {SUPPORTED_LANGUAGES.map((lng) => (
            <option key={lng} value={lng} lang={lng}>
              {LANGUAGE_ENDONYMS[lng]}
            </option>
          ))}
        </Select>
      </Setting>
      <Checkbox
        label={t("settings.confirmCloseWithTerminals")}
        info={t("settings.confirmCloseWithTerminalsInfo")}
        checked={form.confirmCloseWithTerminals}
        onChange={(confirmCloseWithTerminals) => patch({ confirmCloseWithTerminals })}
      />
      <Setting label={t("settings.confirmQuit")} info={t("settings.confirmQuitInfo")}>
        <Select value={form.confirmQuit} onChange={(e) => patch({ confirmQuit: e.target.value as ConfirmQuit })}>
          <option value="always">{t("settings.confirmQuitAlways")}</option>
          <option value={CONFIRM_QUIT.withConnections}>{t("settings.confirmQuitWithConnections")}</option>
          <option value="never">{t("settings.confirmQuitNever")}</option>
        </Select>
      </Setting>
      <Setting label={t("settings.onStartup")} info={t("settings.onStartupInfo")}>
        <Select value={form.onStartup} onChange={(e) => patch({ onStartup: e.target.value as StartupAction })}>
          <option value="welcome">{t("settings.onStartupWelcome")}</option>
          <option value="restore">{t("settings.onStartupRestore")}</option>
        </Select>
      </Setting>
      <Checkbox
        label={t("settings.notifyOnDisconnect")}
        checked={form.notifyOnDisconnect}
        onChange={(notifyOnDisconnect) => patch({ notifyOnDisconnect })}
      />
      {/* Read by the backend at every check (no restart needed). */}
      <Checkbox
        label={t("settings.autoUpdate")}
        info={t("settings.autoUpdateInfo")}
        checked={form.autoUpdate}
        onChange={(autoUpdate) => patch({ autoUpdate })}
      />
    </fieldset>
  );
}
