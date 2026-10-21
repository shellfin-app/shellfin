import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { Dialog } from "../../components/Dialog";
import { Icon } from "../../components/Icon";
import { displayName } from "../../lib/serverText";
import { useStore } from "../../state/store";
import { answerConnectConfirm } from "./connectConfirm";

/** "Confirm before connecting" (bookmark setting): shown before a user-initiated connect to that bookmark. */
export function ConnectConfirmDialog() {
  const { t } = useTranslation();
  const dialog = useStore((s) => s.dialog);
  if (dialog?.kind !== "confirmConnect") return null;
  return (
    <Dialog
      title={t("connectConfirm.title", { name: displayName(dialog.name) })}
      tone="warn"
      role="alertdialog"
      width={460}
      icon={<Icon name="plug" size={20} strokeWidth={1.8} />}
      onClose={() => answerConnectConfirm(false)}
      footer={
        <>
          <Button onClick={() => answerConnectConfirm(false)} data-autofocus>
            {t("common.cancel")}
          </Button>
          <Button variant="primary" onClick={() => answerConnectConfirm(true)}>
            {t("connectConfirm.action")}
          </Button>
        </>
      }
    >
      <p className="dialog__text">{t("connectConfirm.body")}</p>
    </Dialog>
  );
}
