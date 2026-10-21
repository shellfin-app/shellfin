import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { Dialog } from "../../components/Dialog";
import { Icon } from "../../components/Icon";
import { DIALOG_KIND } from "../../lib/dialogs";
import { useStore } from "../../state/store";
import { guard } from "../../lib/reportError";
import { closeSession } from "../session/sessionActions";
import { closeTerminalTab, runningTerminals } from "./terminalActions";

/**
 * Closing terminal tabs, or a whole connection, while shells are still running (setting
 * `general.confirmCloseWithTerminals`). The running programs end with the terminal.
 */
export function CloseTerminalsDialog({ sessionId, termIds }: { sessionId: string; termIds?: string[] }) {
  const { t } = useTranslation();
  const closeDialog = useStore((s) => s.closeDialog);
  const count = runningTerminals(sessionId, termIds).length;
  const wholeConnection = termIds === undefined;
  const close = () => closeDialog(DIALOG_KIND.confirmCloseTerminals);

  const confirm = () => {
    close();
    if (wholeConnection) guard(closeSession(sessionId));
    else for (const id of termIds) closeTerminalTab(sessionId, id);
  };

  return (
    <Dialog
      title={wholeConnection ? t("closeTerminals.titleConnection") : t("closeTerminals.titleTabs", { count })}
      tone="warn"
      role="alertdialog"
      width={460}
      icon={<Icon name="terminal" size={20} strokeWidth={1.8} />}
      onClose={close}
      footer={
        <>
          <Button onClick={close} data-autofocus>
            {t("common.cancel")}
          </Button>
          <Button variant="primary" onClick={confirm}>
            {wholeConnection ? t("closeTerminals.closeConnection") : t("closeTerminals.closeTabs", { count })}
          </Button>
        </>
      }
    >
      <p className="dialog__text">{t("closeTerminals.body", { count })}</p>
      <p className="dialog__hint">{t("closeTerminals.hint")}</p>
    </Dialog>
  );
}
