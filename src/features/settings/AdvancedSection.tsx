import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { Dialog } from "../../components/Dialog";
import { Select, Setting } from "../../components/Field";
import { Icon } from "../../components/Icon";
import type { LogLevel } from "../../ipc/types";
import { A } from "../../lib/actionIds";
import { runAction } from "../actions/registry";
import type { SectionProps } from "./settingsFields";

/** The log level plus the three maintenance actions. The dialog owns what they change (form, recents, errors). */
export function AdvancedSection({
  form,
  patch,
  busy,
  recentsCleared,
  onClearRecents,
  onResetAll,
}: Pick<SectionProps, "form" | "patch"> & {
  busy: boolean;
  recentsCleared: boolean;
  /** Clears the recent connections; resolves when done (the confirm closes either way). */
  onClearRecents: () => Promise<void>;
  /** Resets every setting to its default; resolves when done (the confirm closes either way). */
  onResetAll: () => Promise<void>;
}) {
  const { t } = useTranslation();
  const [confirmReset, setConfirmReset] = useState(false);
  const [confirmClear, setConfirmClear] = useState(false);
  return (
    <>
      <fieldset className="settings-group">
        <legend>{t("settings.advanced")}</legend>
        <Setting label={t("settings.logLevel")} info={t("settings.logLevelInfo")}>
          <Select value={form.logLevel} onChange={(e) => patch({ logLevel: e.target.value as LogLevel })}>
            <option value="info">{t("settings.logLevelInfoOption")}</option>
            <option value="debug">{t("settings.logLevelDebug")}</option>
          </Select>
        </Setting>
        <div className="settings-actions">
          <div className="settings-action">
            <Button icon="folder" onClick={() => runAction(A.openLogs)}>
              {t("settings.openLogs")}
            </Button>
          </div>
          <div className="settings-action">
            <Button icon="trash" onClick={() => setConfirmClear(true)} disabled={busy}>
              {t("settings.clearRecents")}
            </Button>
            <span className="field__hint" role="status">
              {recentsCleared ? t("settings.recentsCleared") : null}
            </span>
          </div>
          <div className="settings-action">
            <Button className="settings-reset" icon="refresh" onClick={() => setConfirmReset(true)} disabled={busy}>
              {t("settings.resetAll")}
            </Button>
          </div>
        </div>
      </fieldset>
      {confirmReset ? (
        <Dialog
          title={t("settings.resetTitle")}
          tone="danger"
          role="alertdialog"
          width={460}
          icon={<Icon name="refresh" size={20} strokeWidth={1.8} />}
          onClose={() => setConfirmReset(false)}
          footer={
            <>
              <Button onClick={() => setConfirmReset(false)} data-autofocus>
                {t("common.cancel")}
              </Button>
              <Button
                variant="primary"
                disabled={busy}
                onClick={() => {
                  void onResetAll().finally(() => setConfirmReset(false));
                }}
              >
                {t("settings.resetConfirm")}
              </Button>
            </>
          }
        >
          <p className="dialog__text">{t("settings.resetBody")}</p>
        </Dialog>
      ) : null}
      {confirmClear ? (
        <Dialog
          title={t("settings.clearRecentsTitle")}
          tone="danger"
          role="alertdialog"
          width={460}
          icon={<Icon name="trash" size={20} strokeWidth={1.8} />}
          onClose={() => setConfirmClear(false)}
          footer={
            <>
              <Button onClick={() => setConfirmClear(false)} data-autofocus>
                {t("common.cancel")}
              </Button>
              <Button
                variant="primary"
                disabled={busy}
                onClick={() => {
                  void onClearRecents().finally(() => setConfirmClear(false));
                }}
              >
                {t("settings.clearRecentsConfirm")}
              </Button>
            </>
          }
        >
          <p className="dialog__text">{t("settings.clearRecentsBody")}</p>
        </Dialog>
      ) : null}
    </>
  );
}
