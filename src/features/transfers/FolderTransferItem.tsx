// One row of the transfers popover for a folder job (§6.6 "Folders"): scan, decision, copy, conflicts, issues.
import { useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { Icon } from "../../components/Icon";
import { IconButton } from "../../components/IconButton";
import { Progress } from "../../components/Progress";
import { errorMessage } from "../../i18n/errorMessage";
import { formatBytes, formatList, formatNumber } from "../../i18n/format";
import type { ItemIssue } from "../../ipc/types";
import { SEL_TRANSFER_DISMISS } from "../../lib/domIds";
import { guard } from "../../lib/reportError";
import { displayName, displayPath } from "../../lib/serverText";
import { type FolderState, type TransferRow, skippedTotal } from "../../state/slices/transfers";
import { useStore } from "../../state/store";
import { openErrorDetails, sourceFromAppError } from "../errorReport/errorReport";
import { cancelTransfer, needsFolderDecision, openFolderDecision, retryTransfer } from "./transferActions";

/** All issues of the job (listed or only counted). */
function issueTotal(folder: FolderState): number {
  return Math.max(folder.issues.length, skippedTotal(folder.skipped) + folder.failed);
}

function useIssueText(): (issue: ItemIssue) => string {
  const { t } = useTranslation();
  return (issue) => (issue.kind === "skipped" ? t(`transfers.skipReason.${issue.reason}`) : errorMessage(issue.error));
}

function FolderIssues({ folder }: { folder: FolderState }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const issueText = useIssueText();
  const total = issueTotal(folder);
  if (total === 0) return null;
  const notListed = total - folder.issues.length;
  return (
    <div className="transfer__issues">
      <button type="button" className="transfer__issues-toggle" aria-expanded={open} onClick={() => setOpen(!open)}>
        <Icon name={open ? "chevronDown" : "chevronRight"} size={12} />
        {t("transfers.issues", { count: total, n: formatNumber(total) })}
      </button>
      {open ? (
        <ul className="transfer__issue-list">
          {folder.issues.map((item, i) => (
            <li key={`${i}:${item.relPath}`} className={`transfer__issue transfer__issue--${item.issue.kind}`}>
              <bdi className="transfer__issue-path mono">{displayPath(item.relPath)}</bdi>
              <span className="transfer__issue-reason">{issueText(item.issue)}</span>
            </li>
          ))}
          {notListed > 0 ? <li className="transfer__issue transfer__issue--more">{t("transfers.issuesNotListed", { count: notListed, n: formatNumber(notListed) })}</li> : null}
        </ul>
      ) : null}
    </div>
  );
}

export function FolderTransferItem({ row, folder }: { row: TransferRow; folder: FolderState }) {
  const { t } = useTranslation();
  const name = displayName(row.name);
  const removeTransfer = useStore((s) => s.removeTransfer);
  const waiting = needsFolderDecision(row);
  const running = row.status === "running";
  const filesTotal = folder.filesTotal ?? folder.scan.files;
  const actionsRef = useRef<HTMLDivElement>(null);
  // Cancel had the focus: when the row finishes, its Dismiss button (which replaces Cancel) takes it over.
  const cancelFocused = useRef(false);
  useLayoutEffect(() => {
    if (running || !cancelFocused.current) return;
    cancelFocused.current = false;
    actionsRef.current?.querySelector<HTMLButtonElement>(SEL_TRANSFER_DISMISS)?.focus();
  }, [running]);

  let status: string;
  let detail: string | null = null;
  switch (row.status) {
    case "running":
      if (folder.phase === "scanning") status = t("transfers.scanning", { count: folder.scan.files, n: formatNumber(folder.scan.files) });
      else if (folder.pending) status = t("transfers.waitingFolderDecision");
      else
        status = t("transfers.folderProgress", {
          count: filesTotal,
          n: formatNumber(filesTotal),
          done: formatNumber(folder.filesDone),
          bytes: formatBytes(row.bytes),
          total: formatBytes(row.total ?? folder.scan.bytes),
        });
      break;
    case "done": {
      const parts = [t("transfers.folderCopied", { count: folder.filesDone, n: formatNumber(folder.filesDone) })];
      if (folder.failed > 0) parts.push(t("transfers.folderFailedCount", { count: folder.failed, n: formatNumber(folder.failed) }));
      const skipped = skippedTotal(folder.skipped);
      if (skipped > 0) parts.push(t("transfers.folderSkippedCount", { count: skipped, n: formatNumber(skipped) }));
      status = formatList(parts, "unit");
      // Where the copy went (`name (2)` after Keep both) and how many names had to change.
      const where = folder.summary ? [t("transfers.savedAs", { name: displayName(folder.summary.localName) })] : [];
      if (folder.renamed > 0) where.push(t("transfers.renamedCount", { count: folder.renamed, n: formatNumber(folder.renamed) }));
      detail = where.length > 0 ? formatList(where, "unit") : null;
      break;
    }
    case "cancelled":
      status = t("transfers.cancelled");
      break;
    case "skipped":
      status = t("transfers.folderSkipped");
      break;
    case "failed":
      status = row.error ? errorMessage(row.error) : t("transfers.failed");
      break;
  }
  // A stopped job leaves the files it finished (no rollback): say how many.
  if (row.status === "cancelled" || row.status === "failed") {
    detail = t("transfers.filesWritten", { count: folder.filesDone, n: formatNumber(folder.filesDone) });
  }
  const progress = row.total ? row.bytes / row.total : 0;
  const withFailures = row.status === "done" && folder.failed > 0;

  return (
    <li className={`transfer transfer--folder transfer--${row.status}${waiting ? " transfer--waiting" : ""}${withFailures ? " transfer--partial" : ""}`}>
      <Icon name="folder" size={14} className="transfer__icon" />
      <div className="transfer__main">
        <span className="transfer__name" title={name}>
          <bdi>{name}</bdi>
        </span>
        <span className="transfer__status">{status}</span>
        {detail !== null ? <span className="transfer__detail">{detail}</span> : null}
        {running && folder.conflicts ? (
          <span className="transfer__conflicts">
            {t("transfers.conflictsTitle", { count: folder.conflicts.count, n: formatNumber(folder.conflicts.count) })}
          </span>
        ) : null}
        {running && folder.phase === "copying" && folder.current !== null ? (
          <bdi className="transfer__current mono">{displayName(folder.current)}</bdi>
        ) : null}
        {running ? <Progress value={progress} /> : null}
        <FolderIssues folder={folder} />
      </div>
      <div className="transfer__actions" ref={actionsRef}>
        {waiting ? (
          <Button onClick={() => openFolderDecision(row)}>{folder.pending ? t("transfers.decide") : t("transfers.resolveConflicts")}</Button>
        ) : null}
        {running ? (
          <IconButton
            icon="close"
            iconSize={12}
            size={24}
            label={t("transfers.cancel", { name })}
            onClick={() => guard(cancelTransfer(row.transferId))}
            onFocus={() => {
              cancelFocused.current = true;
            }}
            // Removing the focused button gives no `relatedTarget`: only a real move elsewhere forgets the focus.
            onBlur={(e) => {
              if (e.relatedTarget !== null) cancelFocused.current = false;
            }}
          />
        ) : null}
        {row.retryable && (row.status === "failed" || row.status === "done") ? (
          <Button onClick={() => guard(retryTransfer(row.transferId, false))}>{t("transfers.retry")}</Button>
        ) : null}
        {row.status === "failed" && row.error ? (
          <Button onClick={() => row.error && openErrorDetails(sourceFromAppError(row.error))}>{t("errorReport.details")}</Button>
        ) : null}
        {!running ? (
          <IconButton
            icon="close"
            iconSize={12}
            size={24}
            label={t("transfers.dismiss", { name })}
            data-dismiss=""
            onClick={() => removeTransfer(row.transferId)}
          />
        ) : null}
      </div>
    </li>
  );
}
