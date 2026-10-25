import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import i18next from "i18next";
import { Button } from "../../components/Button";
import { Dialog } from "../../components/Dialog";
import { Checkbox } from "../../components/Field";
import { Icon } from "../../components/Icon";
import { formatDateTime } from "../../i18n/format";
import { getBackend } from "../../ipc";
import type { ErrorReport } from "../../ipc/types";
import { COPIED } from "../../lib/notices";
import { toIpcError } from "../../ipc/call";
import { guard, reportError } from "../../lib/reportError";
import { isUnexpectedError } from "./errorReport";
import { displayServerText } from "../../lib/serverText";
import { DIALOG_KIND } from "../../lib/dialogs";
import { useStore } from "../../state/store";
import type { ErrorReportSource } from "../../state/slices/ui";
import "./errorReport.css";

const KIND = DIALOG_KIND.errorReport;
/** Keystrokes right after an automatic opening must not hit a button (the user was typing elsewhere). */
const AUTO_INPUT_GUARD_MS = 600;

/**
 * Error details + "Report on GitHub…" (§8.14). The report is built in Rust (redaction, escaping, size limits and the
 * URL); this dialog shows `markdown` as plain text, copies it and opens `issueUrl` through `app_open_external`
 * (native confirm). Nothing is sent by the app.
 */
export function ErrorReportDialog({ source, more, auto }: { source: ErrorReportSource | null; more: number; auto: boolean }) {
  const { t } = useTranslation();
  const closeDialog = useStore((s) => s.closeDialog);
  const pushInfo = useStore((s) => s.pushInfo);
  const [includeLog, setIncludeLog] = useState(false);
  const [report, setReport] = useState<ErrorReport | null>(null);
  // The `includeLog` value the current `report` / `failed` answer belongs to; a different one means a request is out.
  const [doneFor, setDoneFor] = useState<boolean | null>(null);
  const loading = doneFor !== includeLog;
  const [failed, setFailed] = useState(false);
  const [reported, setReported] = useState(false);
  const [copyFailed, setCopyFailed] = useState(false);
  const [busy, setBusy] = useState(false);
  const close = () => closeDialog(KIND);

  useEffect(() => {
    let current = true;
    getBackend()
      .appErrorReport({
        includeLog,
        code: source?.code ?? null,
        params: source?.params ?? null,
        detail: source?.detail ?? null,
        message: source?.message ?? null,
        context: source?.context ?? null,
        stack: source?.stack ?? null,
        occurredAtMs: source?.occurredAtMs ?? null,
        uiLanguage: i18next.language,
      })
      .then((r) => {
        if (!current) return;
        setReport(r);
        setFailed(false);
        setDoneFor(includeLog);
      })
      .catch((e: unknown) => {
        if (!current) return;
        setFailed(true);
        setDoneFor(includeLog);
        // The request's own failure is shown in the dialog; it is not "another unexpected error" for the counter.
        const error = toIpcError(e);
        if (!error.wrapped && isUnexpectedError(error.error)) console.error(error.error.code, error.error.detail ?? "");
        else reportError(e);
      });
    return () => {
      current = false;
    };
  }, [source, includeLog]);

  const ready = report !== null && !loading && !failed;
  const reportOnGithub = () => {
    if (!report) return;
    const { markdown, issueUrl } = report;
    setBusy(true);
    setCopyFailed(false);
    // Copy AND open: a failed copy does not keep the form from opening; the dialog says so (Copy details retries).
    guard(
      Promise.resolve()
        .then(() => navigator.clipboard.writeText(markdown))
        .then(
          () => true,
          (e: unknown) => {
            console.error("clipboard write failed", e instanceof Error ? e.message : "");
            return false;
          },
        )
        .then((copied) => {
          setCopyFailed(!copied);
          return getBackend().appOpenExternal(issueUrl, t("errorReport.openTitle"), t("errorReport.openBody"));
        })
        .then((opened) => setReported(opened))
        .finally(() => setBusy(false)),
    );
  };
  const copyDetails = () => {
    if (!report) return;
    guard(navigator.clipboard.writeText(report.markdown).then(() => pushInfo(COPIED)));
  };

  const partialLog = report !== null && report.logIncluded && report.logLinesInUrl < report.logLines;
  return (
    <Dialog
      title={source ? t("errorReport.title") : t("errorReport.titleNoError")}
      icon={<Icon name="alert" size={20} strokeWidth={1.8} />}
      tone={source ? "warn" : "default"}
      width={680}
      onClose={close}
      inputGuardMs={auto ? AUTO_INPUT_GUARD_MS : 0}
      footer={
        <>
          <Button onClick={copyDetails} disabled={!ready || busy}>
            {t("errorReport.copy")}
          </Button>
          <span className="spacer" />
          <Button onClick={close} data-autofocus>
            {t("common.close")}
          </Button>
          <Button variant="primary" onClick={reportOnGithub} disabled={!ready || busy}>
            {t("errorReport.report")}
          </Button>
        </>
      }
    >
      <div className="error-report">
        {source ? (
          <>
            <p className="dialog__text error-report__facts">{displayServerText(source.message, 512)}</p>
            <p className="dialog__hint mono error-report__meta">
              <span>{t("errorReport.code", { code: displayServerText(source.code, 64) })}</span>
              {source.context !== undefined ? <span>{t("errorReport.where", { place: displayServerText(source.context, 80) })}</span> : null}
              <span>{t("errorReport.when", { time: formatDateTime(source.occurredAtMs, { dateStyle: "medium", timeStyle: "medium" }) })}</span>
            </p>
          </>
        ) : (
          <p className="dialog__text">{t("errorReport.noErrorIntro")}</p>
        )}
        {more > 0 ? (
          <p className="dialog__hint" role="status">
            {t("errorReport.more", { count: more })}
          </p>
        ) : null}
        <Checkbox label={t("errorReport.includeLog")} hint={t("errorReport.includeLogHint")} checked={includeLog}
          onChange={(checked) => {
            setReported(false);
            setCopyFailed(false);
            setIncludeLog(checked);
          }}
        />
        {report !== null && includeLog && !report.logIncluded && !loading ? (
          <p className="dialog__hint">{t("errorReport.logEmpty")}</p>
        ) : null}
        <p className="dialog__hint">{t("errorReport.previewLabel")}</p>
        {failed ? (
          <p className="dialog__error" role="alert">
            {t("errorReport.previewFailed")}
          </p>
        ) : (
          <pre
            className={`error-report__preview mono${loading ? " error-report__preview--busy" : ""}`}
            tabIndex={0}
            aria-label={t("errorReport.previewLabel")}
            aria-busy={loading}
          >
            {report?.markdown ?? ""}
          </pre>
        )}
        <p className="dialog__hint">{t("errorReport.clipboardHint")}</p>
        {partialLog && report ? (
          <p className="dialog__hint">
            {t("errorReport.logPartial", { count: report.logLinesInUrl, total: report.logLines })}
          </p>
        ) : null}
        {reported && !copyFailed ? (
          <p className="dialog__text" role="status">
            {t("errorReport.reported")}
          </p>
        ) : null}
        {copyFailed ? (
          <p className="dialog__error" role="alert">
            {t("errorReport.copyFailed")}
          </p>
        ) : null}
      </div>
    </Dialog>
  );
}

/** Mounts the dialog while the store holds it (DialogHost, and the app-wide ErrorBoundary's crash panel). */
export function ErrorReportHost() {
  const dialog = useStore((s) => s.dialog);
  if (dialog?.kind !== "errorReport") return null;
  return (
    <ErrorReportDialog
      key={`${dialog.source?.code ?? ""}:${dialog.source?.occurredAtMs ?? 0}`}
      source={dialog.source}
      more={dialog.more}
      auto={dialog.auto}
    />
  );
}
