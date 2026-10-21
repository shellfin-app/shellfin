import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { Dialog } from "../../components/Dialog";
import { Icon } from "../../components/Icon";
import { Checkbox } from "../../components/Field";
import { formatBytes, formatDateTime } from "../../i18n/format";
import { getBackend } from "../../ipc";
import type { FileEntry } from "../../ipc/types";
import { DIALOG_KIND } from "../../lib/dialogs";
import { isSymlink } from "../files/fileActions";
import { joinPath } from "../../lib/remotePath";
import { guard } from "../../lib/reportError";
import { displayName, displayPath } from "../../lib/serverText";
import { useStore } from "../../state/store";
import { dismissTransfers, replaceableRows, retryTransfer } from "./transferActions";

/** "Replace?": uploads that hit existing remote files. Replace -> `transfer_retry(overwrite: true)` for each chosen one. */
export function ReplaceDialog({ sessionId }: { sessionId: string }) {
  const { t } = useTranslation();
  const transfers = useStore((s) => s.transfers);
  const closeDialog = useStore((s) => s.closeDialog);
  const rows = replaceableRows(transfers, sessionId);
  const close = () => closeDialog(DIALOG_KIND.replaceTransfers);
  // Files the user took out of the selection (everything is selected by default).
  const [unchecked, setUnchecked] = useState<Set<string>>(() => new Set());
  // What the server has now (size, date) for the comparison; null = unknown.
  const [serverFacts, setServerFacts] = useState<Record<string, FileEntry | null>>({});
  const rowKey = JSON.stringify(rows.map((r) => r.transferId));

  useEffect(() => {
    let cancelled = false;
    for (const r of replaceableRows(useStore.getState().transfers, sessionId)) {
      if (r.remoteDir === null) continue;
      getBackend()
        .sftpStat(sessionId, joinPath(r.remoteDir, r.name))
        .then((entry) => !cancelled && setServerFacts((f) => ({ ...f, [r.transferId]: entry })))
        .catch(() => !cancelled && setServerFacts((f) => ({ ...f, [r.transferId]: null })));
    }
    return () => {
      cancelled = true;
    };
  }, [sessionId, rowKey]);

  const chosen = rows.filter((r) => !unchecked.has(r.transferId));
  // Skip / Escape / close: the conflicting uploads are dismissed for good (never counted again).
  const decline = () => {
    dismissTransfers(rows);
    close();
  };
  const replaceChosen = () => {
    dismissTransfers(rows.filter((r) => unchecked.has(r.transferId)));
    close();
    for (const row of chosen) guard(retryTransfer(row.transferId, true));
  };
  const nothingLeft = rows.length === 0;

  // Everything was handled elsewhere (per-file Replace in the popover, dismissed, retried): nothing left to ask.
  useEffect(() => {
    if (nothingLeft) useStore.getState().closeDialog(DIALOG_KIND.replaceTransfers);
  }, [nothingLeft]);

  if (nothingLeft) return null;
  const hasLink = rows.some((r) => r.remoteDir !== null && isSymlink(sessionId, joinPath(r.remoteDir, r.name)));

  return (
    <Dialog
      title={t("transfers.replaceTitle", { count: rows.length })}
      tone="warn"
      role="alertdialog"
      width={520}
      icon={<Icon name="alert" size={20} strokeWidth={1.8} />}
      onClose={decline}
      footer={
        <>
          <Button onClick={decline} data-autofocus>
            {t("transfers.skipAll")}
          </Button>
          <Button variant="primary" disabled={chosen.length === 0} onClick={replaceChosen}>
            {t("transfers.replaceSelected", { count: chosen.length })}
          </Button>
        </>
      }
    >
      <p className="dialog__text">{t("transfers.replaceBody", { count: rows.length })}</p>
      <ul className="replace-list">
        {rows.map((r) => {
          const server = serverFacts[r.transferId];
          const name = displayName(r.name);
          return (
            <li key={r.transferId} className="replace-item">
              <Checkbox
                label={r.remoteDir !== null ? `${name} — ${displayPath(r.remoteDir)}` : name}
                checked={!unchecked.has(r.transferId)}
                onChange={(on) =>
                  setUnchecked((prev) => {
                    const next = new Set(prev);
                    if (on) next.delete(r.transferId);
                    else next.add(r.transferId);
                    return next;
                  })
                }
              />
              <div className="replace-item__facts">
                <span>{t("transfers.localFacts", { size: r.total !== null ? formatBytes(r.total) : "?" })}</span>
                <span>
                  {server
                    ? t("transfers.serverFacts", {
                        size: server.size !== null ? formatBytes(server.size) : "?",
                        date: server.mtime !== null ? formatDateTime(server.mtime * 1000, { dateStyle: "medium", timeStyle: "short" }) : "?",
                      })
                    : t("transfers.serverFactsUnknown")}
                </span>
              </div>
            </li>
          );
        })}
      </ul>
      {hasLink ? <p className="dialog__warn">{t("transfers.replaceSymlink")}</p> : null}
    </Dialog>
  );
}
