import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { Dialog } from "../../components/Dialog";
import { Icon } from "../../components/Icon";
import { formatList } from "../../i18n/format";
import { DIALOG_KIND } from "../../lib/dialogs";
import { basename } from "../../lib/remotePath";
import { guard } from "../../lib/reportError";
import { displayName, displayServerText } from "../../lib/serverText";
import { useStore } from "../../state/store";
import { closeSession, closeSessionThenReopen } from "../session/sessionActions";
import { closeBuffer, dirtyBuffers, saveAllBuffers, saveBuffer } from "./editorActions";

/** Closing a tab with unsaved changes: Save / Don't save / Cancel. */
export function CloseBufferDialog({ bufferId }: { bufferId: string }) {
  const { t } = useTranslation();
  const meta = useStore((s) => s.buffers[bufferId]);
  const connected = useStore((s) => (meta ? s.sessionsById[meta.sessionId]?.state.phase === "connected" : false));
  const closeDialog = useStore((s) => s.closeDialog);
  const [busy, setBusy] = useState(false);
  if (!meta) return null;
  const name = displayName(basename(meta.path));
  const close = () => closeDialog(DIALOG_KIND.closeBuffer);

  const save = () => {
    setBusy(true);
    close();
    guard(
      saveBuffer(bufferId).then((outcome) => {
        // Saved: close the tab. A conflict opens its own dialog, a queued save keeps the tab open.
        if (outcome === "saved") closeBuffer(meta.sessionId, bufferId);
      }),
    );
  };

  return (
    <Dialog
      title={t("closeBuffer.title", { name })}
      tone="warn"
      role="alertdialog"
      width={460}
      icon={<Icon name="shield" size={20} strokeWidth={1.8} />}
      onClose={close}
      footer={
        <>
          <Button
            variant="ghost"
            disabled={busy}
            onClick={() => {
              close();
              closeBuffer(meta.sessionId, bufferId);
            }}
          >
            {t("closeBuffer.discard")}
          </Button>
          <span className="spacer" />
          <Button onClick={close} disabled={busy}>
            {t("common.cancel")}
          </Button>
          <Button variant="primary" onClick={save} disabled={busy || !connected} data-autofocus={connected ? true : undefined}>
            {t("closeBuffer.save")}
          </Button>
        </>
      }
    >
      <p className="dialog__text">{t("closeBuffer.body", { name })}</p>
      {connected ? null : <p className="dialog__warn">{t("closeBuffer.offline")}</p>}
    </Dialog>
  );
}

/** Closing a connection whose tabs have unsaved changes. */
export function CloseSessionDialog({ sessionId, reopen = false }: { sessionId: string; reopen?: boolean }) {
  const { t } = useTranslation();
  const buffers = useStore((s) => s.buffers);
  const connected = useStore((s) => s.sessionsById[sessionId]?.state.phase === "connected");
  const closeDialog = useStore((s) => s.closeDialog);
  const [busy, setBusy] = useState(false);
  const names = Object.values(buffers)
    .filter((b) => b.sessionId === sessionId && b.dirty)
    .map((b) => displayServerText(basename(b.path), 128));
  const close = () => closeDialog(DIALOG_KIND.closeSession);
  // "Reconnect to apply" (ARCHITECTURE §7.6): the same close, then the bookmark is opened again.
  const finish = reopen ? closeSessionThenReopen : closeSession;

  return (
    <Dialog
      title={t("closeSession.title")}
      tone="warn"
      role="alertdialog"
      width={480}
      icon={<Icon name="shield" size={20} strokeWidth={1.8} />}
      onClose={close}
      footer={
        <>
          <Button
            variant="ghost"
            disabled={busy}
            onClick={() => {
              close();
              guard(finish(sessionId));
            }}
          >
            {t("closeSession.discard")}
          </Button>
          <span className="spacer" />
          <Button onClick={close} disabled={busy}>
            {t("common.cancel")}
          </Button>
          <Button
            variant="primary"
            disabled={busy || !connected}
            data-autofocus={connected ? true : undefined}
            onClick={() => {
              setBusy(true);
              guard(
                saveAllBuffers({ sessionId })
                  .then(() => {
                    close();
                    if (dirtyBuffers(sessionId).length === 0) return finish(sessionId);
                    return undefined;
                  })
                  .finally(() => setBusy(false)),
              );
            }}
          >
            {t("quit.saveAll")}
          </Button>
        </>
      }
    >
      <p className="dialog__text">{t("closeSession.body", { count: names.length, files: formatList(names) })}</p>
      {connected ? null : <p className="dialog__warn">{t("closeBuffer.offline")}</p>}
    </Dialog>
  );
}
