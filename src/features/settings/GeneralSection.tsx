import { useTranslation } from "react-i18next";
import { Checkbox, Select, Setting } from "../../components/Field";
import { LANGUAGE_ENDONYMS, LANGUAGE_STATUS, SUPPORTED_LANGUAGES, resolveLocale, type SupportedLanguage } from "../../i18n";
import { getBackend } from "../../ipc";
import type { ConfirmQuit, StartupAction } from "../../ipc/types";
import { HELP_LINKS } from "../../lib/helpLinks";
import { CONFIRM_QUIT } from "../../lib/identifiers";
import { guard } from "../../lib/reportError";
import type { SectionProps } from "./settingsFields";

/** Whether the language a locale setting resolves to ("system" included) is still marked beta. */
export function isBetaLanguage(locale: string): boolean {
  const lng = resolveLocale(locale) as SupportedLanguage;
  return LANGUAGE_STATUS[lng] === "beta";
}

export function GeneralSection({ form, patch }: SectionProps) {
  const { t } = useTranslation();
  const beta = isBetaLanguage(form.locale);
  // Through the native confirmation of `app_open_external` (http/https only), like the Help menu links.
  const openFeedback = () => {
    const url = HELP_LINKS.translationFeedback;
    guard(getBackend().appOpenExternal(url, t("help.openTitle"), t("help.openBody", { url })));
  };
  return (
    <fieldset className="settings-group">
      <legend>{t("settings.general")}</legend>
      <Setting label={t("settings.language")}>
        <Select value={form.locale} onChange={(e) => patch({ locale: e.target.value })}>
          <option value="system">{t("settings.languageSystem")}</option>
          {SUPPORTED_LANGUAGES.map((lng) =>
            // The endonym stays untranslated. A beta option mixes it with "(beta)" in the current UI language, and an
            // <option> cannot hold a nested lang span, so only a complete one carries `lang`.
            LANGUAGE_STATUS[lng] === "beta" ? (
              <option key={lng} value={lng}>
                {t("settings.languageBeta", { name: LANGUAGE_ENDONYMS[lng] })}
              </option>
            ) : (
              <option key={lng} value={lng} lang={lng}>
                {LANGUAGE_ENDONYMS[lng]}
              </option>
            ),
          )}
        </Select>
      </Setting>
      {beta ? (
        <p className="field__hint">
          {t("settings.languageBetaHint")}{" "}
          <button type="button" className="link-button" onClick={openFeedback}>
            {t("settings.languageBetaFeedback")}
          </button>
        </p>
      ) : null}
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
