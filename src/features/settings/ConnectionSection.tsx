import { useTranslation } from "react-i18next";
import { Checkbox, Input, Setting } from "../../components/Field";
import { DEFAULT_USER_MAX, FIELD } from "../../lib/settingsForm";
import { NumberField, type SectionProps } from "./settingsFields";

export function ConnectionSection(props: SectionProps) {
  const { t } = useTranslation();
  const { form, patch, invalid } = props;
  return (
    <fieldset className="settings-group">
      <legend>{t("settings.connection")}</legend>
      <NumberField {...props} path={FIELD.KEEPALIVE_INTERVAL} label={t("settings.keepaliveInterval")} />
      <NumberField {...props} path={FIELD.KEEPALIVE_COUNT} label={t("settings.keepaliveCount")} />
      <NumberField {...props} path={FIELD.CONNECT_TIMEOUT} label={t("settings.connectTimeout")} />
      <NumberField {...props} path={FIELD.PROMPT_TIMEOUT} label={t("settings.promptTimeout")} />
      <NumberField {...props} path={FIELD.RECONNECT_ATTEMPTS} label={t("settings.reconnectAttempts")} />
      <Checkbox label={t("settings.reconnectEnabled")} checked={form.reconnectEnabled} onChange={(reconnectEnabled) => patch({ reconnectEnabled })} />
      <Setting
        label={t("settings.defaultUser")}
        info={t("settings.defaultUserInfo")}
        error={invalid.includes(FIELD.DEFAULT_USER) ? t("settings.defaultUserInvalid", { max: DEFAULT_USER_MAX }) : null}
      >
        <Input
          mono
          value={form.defaultUser}
          spellCheck={false}
          autoComplete="off"
          aria-invalid={invalid.includes(FIELD.DEFAULT_USER) ? true : undefined}
          onChange={(e) => patch({ defaultUser: e.target.value })}
        />
      </Setting>
      <Checkbox
        label={t("settings.readSshConfig")}
        info={t("settings.readSshConfigInfo")}
        checked={form.readSshConfig}
        onChange={(readSshConfig) => patch({ readSshConfig })}
      />
      <Checkbox
        label={t("settings.rememberPasswords")}
        info={t("settings.rememberPasswordsInfo")}
        checked={form.rememberPasswords}
        onChange={(rememberPasswords) => patch({ rememberPasswords })}
      />
    </fieldset>
  );
}
