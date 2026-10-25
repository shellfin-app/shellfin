import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { Dialog } from "../../components/Dialog";
import { Icon } from "../../components/Icon";
import { Progress } from "../../components/Progress";
import { errorMessage } from "../../i18n/errorMessage";
import { formatBytes, formatDateTime } from "../../i18n/format";
import { guard } from "../../lib/reportError";
import { displayName } from "../../lib/serverText";
import { useStore } from "../../state/store";
import { ReleaseNotes, parseNotesForDisplay } from "./ReleaseNotes";
import {
  cancelUpdateDownload,
  installUpdate,
  openReleasePage,
  skipUpdateVersion,
  updateLater,
} from "./updateActions";
import "./updates.css";

/** Release notes are untrusted server text: escaped, bounded, a small Markdown subset as React elements (§9.5). */
export const NOTES_DISPLAY_LIMIT = 10_000;

/** The update offer, the download progress and failures (§9.8). */
export function UpdateDialog() {
  const { t } = useTranslation();
  const update = useStore((s) => s.update);
  const platform = useStore((s) => s.platform);
  const [busy, setBusy] = useState(false);
  const rawNotes = update?.info.notes ?? "";
  const notes = useMemo(() => parseNotesForDisplay(rawNotes, NOTES_DISPLAY_LIMIT), [rawNotes]);
  if (!update) return null;
  const { info, phase } = update;
  const version = displayName(info.version);
  const manual = info.install === "manualDownload";
  const date = info.date ? Date.parse(info.date) : Number.NaN;
  const run = (action: () => Promise<void>) => {
    setBusy(true);
    guard(action().finally(() => setBusy(false)));
  };
  const working = phase === "downloading" || phase === "restarting";
  const fraction = update.total ? update.downloaded / update.total : 0;

  let footer;
  if (phase === "downloading") {
    footer = (
      <>
        <span className="spacer" />
        <Button onClick={cancelUpdateDownload} data-autofocus>
          {t("common.cancel")}
        </Button>
      </>
    );
  } else if (phase === "restarting") {
    footer = null;
  } else if (manual) {
    footer = (
      <>
        <Button variant="ghost" onClick={() => run(skipUpdateVersion)} disabled={busy}>
          {t("update.skip")}
        </Button>
        <span className="spacer" />
        <Button onClick={updateLater} disabled={busy}>
          {t("update.later")}
        </Button>
        <Button variant="primary" onClick={openReleasePage} disabled={busy} data-autofocus>
          {t("update.download")}
        </Button>
      </>
    );
  } else {
    footer = (
      <>
        <Button variant="ghost" onClick={() => run(skipUpdateVersion)} disabled={busy}>
          {t("update.skip")}
        </Button>
        <span className="spacer" />
        <Button onClick={updateLater} disabled={busy}>
          {t("update.later")}
        </Button>
        <Button variant="primary" onClick={() => run(installUpdate)} disabled={busy} data-autofocus>
          {phase === "failed" ? t("update.retry") : t("update.install")}
        </Button>
      </>
    );
  }

  return (
    <Dialog
      title={t("update.title", { version })}
      subtitle={t("update.current", { version: displayName(info.currentVersion) })}
      icon={<Icon name={phase === "failed" ? "alert" : "download"} size={20} strokeWidth={1.8} />}
      tone={phase === "failed" ? "warn" : "default"}
      width={560}
      onClose={working ? undefined : updateLater}
      footer={footer}
    >
      {Number.isFinite(date) ? (
        <p className="dialog__hint">{t("update.published", { date: formatDateTime(date, { dateStyle: "medium" }) })}</p>
      ) : null}
      {phase === "failed" && update.error ? (
        <p className="dialog__text update__error" role="alert">
          {errorMessage(update.error)}
        </p>
      ) : null}
      {phase === "downloading" ? (
        <div className="update__progress" role="status">
          <Progress value={fraction} />
          <span className="dialog__hint">
            {update.total
              ? t("update.downloadingOf", { done: formatBytes(update.downloaded), total: formatBytes(update.total) })
              : t("update.downloading", { done: formatBytes(update.downloaded) })}
          </span>
        </div>
      ) : null}
      {phase === "restarting" ? (
        <p className="dialog__text" role="status">
          {t("update.restarting")}
        </p>
      ) : null}
      {manual && phase !== "failed" ? (
        <p className="dialog__text">{t(platform === "linux" ? "update.manualLinux" : "update.manual")}</p>
      ) : null}
      <h3 className="update__notes-title">{t("update.notes")}</h3>
      {info.notes ? (
        <div className="update__notes" tabIndex={0} aria-label={t("update.notes")}>
          <ReleaseNotes blocks={notes.blocks} />
        </div>
      ) : (
        <p className="dialog__hint">{t("update.noNotes")}</p>
      )}
      {info.notesTruncated || notes.truncated ? <p className="dialog__hint">{t("update.notesTruncated")}</p> : null}
      {manual || phase === "restarting" ? null : (
        // Reachable until the restart: the manifest may carry shortened notes (scripts/updater-manifest.mjs). The
        // manual-download footer already opens the release page.
        <p className="dialog__hint">
          <button type="button" className="update__link" onClick={openReleasePage}>
            {t("update.releaseNotesLink")}
          </button>
        </p>
      )}
    </Dialog>
  );
}
