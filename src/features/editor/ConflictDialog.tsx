import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { Dialog } from "../../components/Dialog";
import { Icon } from "../../components/Icon";
import { errorMessage } from "../../i18n/errorMessage";
import { formatBytes, formatDateTime, formatList } from "../../i18n/format";
import { getBackend } from "../../ipc";
import { toIpcError } from "../../ipc/call";
import { DIALOG_KIND } from "../../lib/dialogs";
import type { AppError } from "../../ipc/types";
import { countLines, diffLines, type DiffResult } from "../../lib/lineDiff";
import { basename } from "../../lib/remotePath";
import { guard } from "../../lib/reportError";
import { displayName, displayPath, displayServerText } from "../../lib/serverText";
import { useStore } from "../../state/store";
import { isSymlink } from "../files/fileActions";
import { getBufferState } from "./bufferStore";
import { openNextConflict, reloadFromServer, saveBuffer, snoozeConflict } from "./editorActions";

type Server = { status: "loading" } | { status: "ready"; diff: DiffResult; lines: number } | { status: "failed"; error: AppError };

/**
 * `remoteChanged` (§6.4): the server file changed since the buffer was loaded. Overwrite (force), Reload from
 * server (drops local edits) or Cancel. "Compare-lite": line counts and the first differing lines.
 */
export function ConflictDialog({ bufferId }: { bufferId: string }) {
  const { t } = useTranslation();
  const meta = useStore((s) => s.buffers[bufferId]);
  const closeDialog = useStore((s) => s.closeDialog);
  const [server, setServer] = useState<Server>({ status: "loading" });
  const [busy, setBusy] = useState(false);

  // Fetch the server's current text for the comparison (the read goes through the same limits as opening).
  useEffect(() => {
    const info = useStore.getState().buffers[bufferId];
    if (!info || (info.conflict?.size === 0 && info.conflict.mtime === 0)) return undefined; // gone: nothing to compare
    let cancelled = false;
    getBackend()
      .sftpReadText(info.sessionId, info.path, info.encoding)
      .then((doc) => {
        if (cancelled) return;
        const mine = getBufferState(bufferId)?.doc.toString() ?? "";
        setServer({ status: "ready", diff: diffLines(mine, doc.text), lines: countLines(doc.text) });
      })
      .catch((e: unknown) => {
        if (!cancelled) setServer({ status: "failed", error: toIpcError(e).error });
      });
    return () => {
      cancelled = true;
    };
  }, [bufferId]);

  const cancel = () => {
    snoozeConflict(bufferId);
    closeDialog(DIALOG_KIND.conflict);
    openNextConflict();
  };

  if (!meta || meta.conflict === null) {
    // Resolved elsewhere (saved / reloaded / closed): nothing to decide any more.
    return <ClosesItself onClose={() => closeDialog(DIALOG_KIND.conflict)} />;
  }
  const name = displayName(basename(meta.path));
  // The backend reports a file that vanished as remoteChanged{size: 0, mtime: 0} (ARCHITECTURE §6.4); a not-found compare read says the same.
  const gone = (meta.conflict.size === 0 && meta.conflict.mtime === 0) || (server.status === "failed" && server.error.code === "remoteNotFound");
  const symlink = isSymlink(meta.sessionId, meta.path);
  const target = meta.conflict.target;
  const myLines = countLines(getBufferState(bufferId)?.doc.toString() ?? "");

  const overwrite = () => {
    setBusy(true);
    closeDialog(DIALOG_KIND.conflict);
    guard(saveBuffer(bufferId, { force: true }).then(() => openNextConflict()));
  };
  const reload = () => {
    setBusy(true);
    closeDialog(DIALOG_KIND.conflict);
    guard(
      reloadFromServer(bufferId).then((ok) => {
        // A failed reload must not bring the same dialog straight back.
        if (!ok) snoozeConflict(bufferId);
        openNextConflict();
      }),
    );
  };

  return (
    <Dialog
      title={gone ? t("conflict.goneTitle", { name }) : t("conflict.title", { name })}
      subtitle={<bdi>{displayPath(meta.path)}</bdi>}
      tone="warn"
      role="alertdialog"
      width={620}
      icon={<Icon name="alert" size={20} strokeWidth={1.8} />}
      onClose={cancel}
      footer={
        <>
          <Button variant="ghost" onClick={overwrite} disabled={busy}>
            {gone ? t("conflict.saveAgain") : t("conflict.overwrite")}
          </Button>
          <span className="spacer" />
          {gone ? null : (
            <Button onClick={reload} disabled={busy}>
              {t("conflict.reload")}
            </Button>
          )}
          <Button variant="primary" onClick={cancel} disabled={busy} data-autofocus>
            {t("common.cancel")}
          </Button>
        </>
      }
    >
      <p className="dialog__text">{gone ? t("conflict.goneBody") : t("conflict.body")}</p>
      {target !== undefined ? (
        <p className="dialog__warn">
          {t("conflict.symlinkTarget")} <bdi className="mono">{displayPath(target)}</bdi>
        </p>
      ) : symlink ? (
        <p className="dialog__warn">{t("conflict.symlink")}</p>
      ) : null}
      <div className="facts">
        <div className="facts__row">
          <span className="facts__label">{t("conflict.server")}</span>
          <span className="facts__value">
            {gone ? t("conflict.gone") : t("conflict.serverFacts", { size: formatBytes(meta.conflict.size), time: formatDateTime(meta.conflict.mtime * 1000) })}
            {server.status === "ready" ? ` · ${t("conflict.lines", { count: server.lines })}` : null}
          </span>
        </div>
        <div className="facts__row">
          <span className="facts__label">{t("conflict.mine")}</span>
          <span className="facts__value">{t("conflict.lines", { count: myLines })}</span>
        </div>
      </div>
      {gone ? null : server.status === "loading" ? <p className="dialog__hint">{t("conflict.comparing")}</p> : null}
      {!gone && server.status === "failed" ? <p className="dialog__warn">{t("conflict.compareFailed", { reason: errorMessage(server.error) })}</p> : null}
      {!gone && server.status === "ready" ? <DiffPreview diff={server.diff} /> : null}
      <p className="dialog__hint">{gone ? t("conflict.goneHint") : t("conflict.hint")}</p>
    </Dialog>
  );
}

function DiffPreview({ diff }: { diff: DiffResult }) {
  const { t } = useTranslation();
  if (diff.added === 0 && diff.removed === 0) return <p className="dialog__hint">{t("conflict.sameText")}</p>;
  return (
    <div className="diff">
      <p className="dialog__hint">
        {formatList([
          ...(diff.added > 0 ? [t("conflict.onlyServer", { count: diff.added })] : []),
          ...(diff.removed > 0 ? [t("conflict.onlyYours", { count: diff.removed })] : []),
        ])}
      </p>
      {diff.exact ? (
        <pre className="diff__lines mono" aria-label={t("conflict.diffLabel")}>
          {diff.lines.map((l) => (
            <div key={`${l.op}:${l.line}`} className={`diff__line diff__line--${l.op}`}>
              <span className="diff__side" aria-hidden="true">
                {l.op === "add" ? t("conflict.server") : t("conflict.mine")}
              </span>
              <span className="diff__no">{l.line}</span>
              <span className="diff__text">
                <span className="sr-only">{l.op === "add" ? t("conflict.onServer") : t("conflict.inYours")}</span>
                <bdi>{displayServerText(l.text, 300)}</bdi>
              </span>
            </div>
          ))}
          {diff.truncated ? <div className="diff__more">{t("conflict.more")}</div> : null}
        </pre>
      ) : (
        <p className="dialog__hint">{t("conflict.tooLongToCompare")}</p>
      )}
    </div>
  );
}

/** Renders nothing and asks to be closed (the conflict it was opened for is gone). */
function ClosesItself({ onClose }: { onClose: () => void }) {
  useEffect(() => {
    onClose();
  }, [onClose]);
  return null;
}
