import { useEffect, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { Icon } from "../../components/Icon";
import { IconButton } from "../../components/IconButton";
import { Progress } from "../../components/Progress";
import { errorMessage } from "../../i18n/errorMessage";
import { formatBytes, formatRate } from "../../i18n/format";
import { effectiveLimits } from "../../lib/bandwidth";
import { guard } from "../../lib/reportError";
import { displayName, displayPath } from "../../lib/serverText";
import type { TransferRow } from "../../state/slices/transfers";
import { useStore } from "../../state/store";
import { cancelTransfer, isReplaceable, retryTransfer, showInTree } from "./transferActions";
import "./transfers.css";

function TransferItem({ row }: { row: TransferRow }) {
  const { t } = useTranslation();
  const name = row.name === "" ? t("transfers.unknownFile") : displayName(row.name);
  const removeTransfer = useStore((s) => s.removeTransfer);
  const progress = row.total ? row.bytes / row.total : row.status === "done" ? 1 : 0;

  let status: string;
  switch (row.status) {
    case "running":
      status = row.total ? t("transfers.progress", { done: formatBytes(row.bytes), total: formatBytes(row.total) }) : formatBytes(row.bytes);
      break;
    case "done":
      status = row.total ? `${formatBytes(row.total)} · ${t("transfers.done")}` : t("transfers.done");
      break;
    case "cancelled":
      status = t("transfers.cancelled");
      break;
    case "skipped":
      status = t("transfers.skipped");
      break;
    case "failed":
      status = isReplaceable(row) ? t("transfers.waitingDecision") : row.error ? errorMessage(row.error) : t("transfers.failed");
      break;
  }

  return (
    <li className={`transfer transfer--${row.status}${isReplaceable(row) ? " transfer--waiting" : ""}`}>
      <Icon name={row.direction === "upload" ? "upload" : "download"} size={14} className="transfer__icon" />
      <div className="transfer__main">
        <span className="transfer__name">
          <bdi>{name}</bdi>
          {row.direction === "upload" && row.remoteDir !== null ? (
            <bdi className="transfer__dest mono">{t("transfers.to", { dir: displayPath(row.remoteDir) })}</bdi>
          ) : null}
        </span>
        <span className="transfer__status">{status}</span>
        {row.status === "running" ? <Progress value={progress} /> : null}
      </div>
      <div className="transfer__actions">
        {row.status === "running" ? (
          <IconButton icon="close" iconSize={12} size={24} label={t("transfers.cancel", { name })} onClick={() => guard(cancelTransfer(row.transferId))} />
        ) : null}
        {isReplaceable(row) ? (
          <Button onClick={() => guard(retryTransfer(row.transferId, true))}>{t("transfers.replace")}</Button>
        ) : row.status === "failed" && row.retryable ? (
          <Button onClick={() => guard(retryTransfer(row.transferId, false))}>{t("transfers.retry")}</Button>
        ) : null}
        {row.status === "done" && row.direction === "upload" && row.remoteDir !== null ? (
          <IconButton icon="folder" iconSize={12} size={24} label={t("transfers.showInTree", { name })} onClick={() => showInTree(row)} />
        ) : null}
        {row.status !== "running" ? (
          <IconButton icon="close" iconSize={12} size={24} label={t("transfers.dismiss", { name })} onClick={() => removeTransfer(row.transferId)} />
        ) : null}
      </div>
    </li>
  );
}

/** Status-bar button that shows the session's transfers and opens the popover (progress, cancel, retry, Replace). */
export function TransfersButton({ sessionId }: { sessionId: string }) {
  const { t } = useTranslation();
  const all = useStore((s) => s.transfers);
  const open = useStore((s) => s.transfersOpen);
  const setOpen = useStore((s) => s.setTransfersOpen);
  const clearFinished = useStore((s) => s.clearFinishedTransfers);
  const rows = useMemo(() => all.filter((r) => r.sessionId === sessionId), [all, sessionId]);
  const settings = useStore((s) => s.settings);
  // The live overrides: a bookmark edit is applied to the running session at once (ARCHITECTURE §7.6).
  const overrides = useStore((s) => s.sessionsById[sessionId]?.info?.limitOverrides);
  const limits = effectiveLimits(settings, overrides);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const popRef = useRef<HTMLDivElement>(null);
  const running = rows.filter((r) => r.status === "running").length;
  // An upload waiting for the Replace decision is not a failure.
  const waiting = rows.filter((r) => isReplaceable(r)).length;
  const failed = rows.filter((r) => r.status === "failed" && !isReplaceable(r)).length;
  const visible = rows.length > 0;

  useEffect(() => {
    if (!visible && open) setOpen(false);
  }, [visible, open, setOpen]);

  useEffect(() => {
    if (!open) return undefined;
    const onDown = (e: MouseEvent) => {
      if (!(e.target instanceof Node)) return;
      if (popRef.current?.contains(e.target) || buttonRef.current?.contains(e.target)) return;
      setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || !popRef.current?.contains(document.activeElement)) return;
      e.preventDefault();
      e.stopPropagation();
      setOpen(false);
      buttonRef.current?.focus();
    };
    document.addEventListener("mousedown", onDown, true);
    document.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("mousedown", onDown, true);
      document.removeEventListener("keydown", onKey, true);
    };
  }, [open, setOpen]);

  if (!visible) return null;
  const label =
    running > 0
      ? t("transfers.active", { count: running })
      : waiting > 0
        ? t("transfers.needDecision", { count: waiting })
        : failed > 0
          ? t("transfers.needAttention", { count: failed })
          : t("transfers.title");

  return (
    <>
      <button
        ref={buttonRef}
        type="button"
        className={`status-bar__button${failed > 0 || waiting > 0 ? " status-bar__button--warn" : ""}`}
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        <Icon name="transfers" size={12} />
        {label}
      </button>
      {open ? (
        <div ref={popRef} className="transfers-popover" role="dialog" aria-label={t("transfers.title")}>
          <div className="transfers-popover__head">
            <span className="sec">{t("transfers.title")}</span>
            <Button variant="ghost" onClick={clearFinished}>
              {t("transfers.clear")}
            </Button>
          </div>
          {limits.upload > 0 || limits.download > 0 ? (
            <p className="transfers-popover__limits">
              {t("transfers.limits", {
                up: limits.upload > 0 ? formatRate(limits.upload * 1024) : t("transfers.unlimited"),
                down: limits.download > 0 ? formatRate(limits.download * 1024) : t("transfers.unlimited"),
              })}
            </p>
          ) : null}
          <ul className="transfers-popover__list">
            {rows.map((row) => (
              <TransferItem key={row.transferId} row={row} />
            ))}
          </ul>
        </div>
      ) : null}
    </>
  );
}
