import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { Dialog } from "../../components/Dialog";
import { Icon } from "../../components/Icon";
import { useStore } from "../../state/store";
import { answerPasteConfirm } from "./pasteConfirm";

/** "Paste N lines?" shown before multi-line text reaches a shell that is not in bracketed-paste mode. */
export function PasteConfirmDialog() {
  const { t } = useTranslation();
  const dialog = useStore((s) => s.dialog);
  if (dialog?.kind !== "confirmPaste") return null;
  return (
    <Dialog
      title={t("terminal.pasteTitle", { count: dialog.lines })}
      tone="warn"
      role="alertdialog"
      width={520}
      icon={<Icon name="terminal" size={20} strokeWidth={1.8} />}
      onClose={() => answerPasteConfirm(false)}
      footer={
        <>
          <Button onClick={() => answerPasteConfirm(false)} data-autofocus>
            {t("common.cancel")}
          </Button>
          <Button variant="primary" onClick={() => answerPasteConfirm(true)}>
            {t("terminal.pasteAction")}
          </Button>
        </>
      }
    >
      <p className="dialog__text">{t("terminal.pasteBody")}</p>
      <pre className="terminal-paste-preview mono">{dialog.preview}</pre>
    </Dialog>
  );
}
