// Folder job dialogs (§6.6 "Folders"): what a job waits for before the copy (root exists, large tree, low space) and
// the deferred "Files already exist" conflicts. Closing a dialog without a choice leaves the job waiting; the
// transfers popover offers it again.
import { type ReactNode, useEffect } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { Dialog } from "../../components/Dialog";
import { Icon } from "../../components/Icon";
import { formatBytes, formatFileDate, formatNumber } from "../../i18n/format";
import type { ConflictSide, TransferDecision } from "../../ipc/types";
import { DIALOG_KIND } from "../../lib/dialogs";
import { guard } from "../../lib/reportError";
import { displayName, displayPath } from "../../lib/serverText";
import { DECISION } from "../../lib/transferDecisions";
import { useStore } from "../../state/store";
import { decideTransfer } from "./transferActions";

function useFolderRow(transferId: string) {
  return useStore((s) => {
    const row = s.transfers.find((r) => r.transferId === transferId);
    return row?.status === "running" ? row.folder : undefined;
  });
}

/** Closes the dialog once nothing waits any more (decided elsewhere, cancelled, finished). */
function useCloseWhenIdle(kind: (typeof DIALOG_KIND)[keyof typeof DIALOG_KIND], idle: boolean): void {
  useEffect(() => {
    if (idle) useStore.getState().closeDialog(kind);
  }, [idle, kind]);
}

export function TransferDecisionDialog({ transferId }: { transferId: string }) {
  const { t } = useTranslation();
  const request = useFolderRow(transferId)?.pending ?? null;
  const close = () => useStore.getState().closeDialog(DIALOG_KIND.transferDecision);
  useCloseWhenIdle(DIALOG_KIND.transferDecision, request === null);
  if (request === null) return null;

  const decide = (decision: TransferDecision) => {
    close();
    guard(decideTransfer(transferId, decision));
  };
  const cancel = (
    <Button onClick={() => decide(DECISION.cancel)} data-autofocus>
      {t("common.cancel")}
    </Button>
  );

  let title: string;
  let body: string;
  let actions: ReactNode;
  switch (request.kind) {
    case "rootExists": {
      const name = displayName(request.name);
      title = request.isDir ? t("transfers.rootExistsTitle") : t("transfers.rootExistsFileTitle");
      body = request.isDir ? t("transfers.rootExistsBody", { name }) : t("transfers.rootExistsFileBody", { name });
      actions = (
        <>
          {cancel}
          <Button variant={request.isDir ? "outline" : "primary"} onClick={() => decide(DECISION.keepBoth)}>
            {t("transfers.keepBoth")}
          </Button>
          {request.isDir ? (
            <Button variant="primary" onClick={() => decide(DECISION.merge)}>
              {t("transfers.merge")}
            </Button>
          ) : null}
        </>
      );
      break;
    }
    case "largeTree":
      title = t("transfers.largeTreeTitle", { count: request.files, n: formatNumber(request.files) });
      body = t("transfers.largeTreeBody", { count: request.files, n: formatNumber(request.files), size: formatBytes(request.bytes) });
      actions = (
        <>
          {cancel}
          <Button variant="primary" onClick={() => decide(DECISION.proceed)}>
            {t("transfers.proceed")}
          </Button>
        </>
      );
      break;
    case "lowSpace":
      title = t("transfers.lowSpaceTitle");
      body = t("transfers.lowSpaceBody", { needed: formatBytes(request.needed), available: formatBytes(request.available) });
      actions = (
        <>
          {cancel}
          <Button variant="primary" onClick={() => decide(DECISION.proceed)}>
            {t("transfers.proceedAnyway")}
          </Button>
        </>
      );
      break;
  }

  return (
    <Dialog title={title} tone="warn" role="alertdialog" width={480} icon={<Icon name="alert" size={20} strokeWidth={1.8} />} onClose={close} footer={actions}>
      <p className="dialog__text">{body}</p>
    </Dialog>
  );
}

/** "On the server: 12 KiB, date" (the source) or "On this computer: …" (the existing target). */
function useSideText(): (side: ConflictSide, local: boolean) => string {
  const { t } = useTranslation();
  return (side, local) => {
    // Server mtimes can be anything: an out-of-range one gives "" (never a RangeError).
    const date = side.mtime !== null ? formatFileDate(side.mtime, Date.now(), "absolute") : "";
    const unknown = t("transfers.unknownFact");
    const params = { size: side.size !== null ? formatBytes(side.size) : unknown, date: date === "" ? unknown : date };
    return local ? t("transfers.localSideFacts", params) : t("transfers.serverFacts", params);
  };
}

export function TransferConflictsDialog({ transferId }: { transferId: string }) {
  const { t } = useTranslation();
  const conflicts = useFolderRow(transferId)?.conflicts ?? null;
  const sideText = useSideText();
  const close = () => useStore.getState().closeDialog(DIALOG_KIND.transferConflicts);
  useCloseWhenIdle(DIALOG_KIND.transferConflicts, conflicts === null);
  if (conflicts === null) return null;

  const decide = (decision: TransferDecision) => {
    close();
    guard(decideTransfer(transferId, decision));
  };
  const more = conflicts.count - conflicts.sample.length;

  return (
    <Dialog
      title={t("transfers.conflictsTitle", { count: conflicts.count, n: formatNumber(conflicts.count) })}
      tone="warn"
      role="alertdialog"
      width={560}
      icon={<Icon name="alert" size={20} strokeWidth={1.8} />}
      onClose={close}
      footer={
        <>
          <Button onClick={() => decide(DECISION.skipAll)} data-autofocus>
            {t("transfers.skipAll")}
          </Button>
          <Button onClick={() => decide(DECISION.replaceOlder)}>{t("transfers.replaceOlder")}</Button>
          <Button variant="primary" onClick={() => decide(DECISION.replaceAll)}>
            {t("transfers.replaceAll")}
          </Button>
        </>
      }
    >
      <p className="dialog__text">{t("transfers.conflictsBody")}</p>
      {/* A scroll container: reachable with Tab so the keyboard can scroll it. */}
      <ul className="replace-list" tabIndex={0} aria-label={t("transfers.conflictsListLabel")}>
        {conflicts.sample.map((c) => (
          <li key={c.itemId} className="replace-item">
            <bdi className="mono">{displayPath(c.relPath)}</bdi>
            <div className="conflict-sample__facts">
              <span>{sideText(c.source, false)}</span>
              <span>{sideText(c.target, true)}</span>
            </div>
          </li>
        ))}
      </ul>
      {more > 0 ? <p className="dialog__text">{t("transfers.conflictsMore", { count: more, n: formatNumber(more) })}</p> : null}
    </Dialog>
  );
}
