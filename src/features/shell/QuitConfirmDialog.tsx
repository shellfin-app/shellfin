import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { Dialog } from "../../components/Dialog";
import { Icon } from "../../components/Icon";
import { formatList } from "../../i18n/format";
import { basename } from "../../lib/remotePath";
import { guard } from "../../lib/reportError";
import { displayServerText } from "../../lib/serverText";
import { useStore } from "../../state/store";
import { countOpenConnections, quitCancel, quitDiscard, quitSaveAll } from "../session/quitFlow";

/** Unsaved changes on quit (§8.9): Save all / Discard / Cancel. */
export function QuitConfirmDialog() {
  const { t } = useTranslation();
  const buffers = useStore((s) => s.buffers);
  const focusTick = useStore((s) => s.quitFocusTick);
  const [busy, setBusy] = useState(false);
  const names = Object.values(buffers)
    .filter((b) => b.dirty)
    .map((b) => displayServerText(basename(b.path), 128));

  // Nothing unsaved: the question comes from `general.confirmQuit`; running shells and open connections are named.
  const terminals = useStore((s) => Object.values(s.terminals).filter((m) => m.status === "running").length);
  const connections = useStore(countOpenConnections);
  const terminalsOnly = names.length === 0;

  const run = (action: () => Promise<void>) => {
    setBusy(true);
    guard(action().finally(() => setBusy(false)));
  };

  return (
    <Dialog
      title={terminalsOnly ? (terminals > 0 ? t("quit.titleTerminals") : t("quit.titleConfirm")) : t("quit.title")}
      tone="warn"
      role="alertdialog"
      icon={<Icon name="shield" size={20} strokeWidth={1.8} />}
      refocusKey={focusTick}
      onClose={() => run(quitCancel)}
      footer={
        terminalsOnly ? (
          <>
            <span className="spacer" />
            <Button onClick={() => run(quitCancel)} disabled={busy} data-autofocus>
              {t("common.cancel")}
            </Button>
            <Button variant="primary" onClick={() => run(quitDiscard)} disabled={busy}>
              {t("quit.quitAnyway")}
            </Button>
          </>
        ) : (
          <>
            <Button variant="ghost" onClick={() => run(quitDiscard)} disabled={busy}>
              {t("quit.discard")}
            </Button>
            <span className="spacer" />
            <Button onClick={() => run(quitCancel)} disabled={busy}>
              {t("common.cancel")}
            </Button>
            <Button variant="primary" onClick={() => run(quitSaveAll)} disabled={busy} data-autofocus>
              {t("quit.saveAll")}
            </Button>
          </>
        )
      }
    >
      {terminalsOnly ? null : <p className="dialog__text">{t("quit.body", { count: names.length, files: formatList(names) })}</p>}
      {terminals > 0 ? <p className={terminalsOnly ? "dialog__text" : "dialog__hint"}>{t("quit.terminals", { count: terminals })}</p> : null}
      {terminalsOnly && terminals === 0 ? (
        <p className="dialog__text">{connections > 0 ? t("quit.connections", { count: connections }) : t("quit.plain")}</p>
      ) : null}
    </Dialog>
  );
}
