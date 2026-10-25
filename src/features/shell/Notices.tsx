import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { errorMessage } from "../../i18n/errorMessage";
import { Icon } from "../../components/Icon";
import { IconButton } from "../../components/IconButton";
import { displayLocalPath } from "../../lib/localPath";
import { displayName } from "../../lib/serverText";
import type { AppError } from "../../ipc/types";
import { useStore } from "../../state/store";
import { openErrorDetails, sourceFromAppError } from "../errorReport/errorReport";
import type { Notice } from "../../state/slices/ui";

/** How long an informational toast stays (ms). They never stick: the log and the failing view keep the details. */
export const NOTICE_TIMEOUT_MS = 8000;
/** Errors stay longer: they carry what the user has to act on (WCAG 2.2.1). */
export const ERROR_NOTICE_TIMEOUT_MS = 20000;

function NoticeItem({ notice }: { notice: Notice }) {
  const { t } = useTranslation();
  const dismiss = useStore((s) => s.dismissNotice);
  const platform = useStore((s) => s.platform);
  const { id } = notice;
  // The timer is paused while the pointer is over the toast or focus is inside it, and resumes with what was left.
  const [paused, setPaused] = useState(false);
  const remaining = useRef(notice.level === "error" ? ERROR_NOTICE_TIMEOUT_MS : NOTICE_TIMEOUT_MS);
  useEffect(() => {
    if (paused) return undefined;
    const startedAt = Date.now();
    const timer = setTimeout(() => dismiss(id), remaining.current);
    return () => {
      clearTimeout(timer);
      remaining.current = Math.max(1000, remaining.current - (Date.now() - startedAt));
    };
  }, [id, dismiss, paused]);

  let text = "";
  if (notice.error) {
    text = errorMessage(notice.error);
  } else if (notice.info) {
    switch (notice.info.kind) {
      case "foldersUnsupported":
        text = t("notices.info.foldersUnsupported");
        break;
      case "dropNoTarget":
        text = t("notices.info.dropNoTarget");
        break;
      case "mixedEol":
        text = t("notices.info.mixedEol", { name: displayName(notice.info.name) });
        break;
      case "copied":
        text = t("notices.info.copied");
        break;
      case "outputSaved":
        text = t("notices.info.outputSaved", { path: displayLocalPath(notice.info.path, platform) });
        break;
      case "outputCancelled":
        text = t("notices.info.outputCancelled");
        break;
      case "shortcutSwallowed":
        text = t("notices.info.shortcutSwallowed", { combo: notice.info.combo });
        break;
      case "outputTooLarge":
        text = t("notices.info.outputTooLarge");
        break;
      case "upToDate":
        text = t("notices.info.upToDate");
        break;
      case "updateAvailable":
        text = t("notices.info.updateAvailable", { version: displayName(notice.info.version) });
        break;
    }
  }
  return (
    <div
      className={`notice notice--${notice.level}`}
      data-level={notice.level}
      role={notice.level === "error" ? "alert" : "status"}
      onMouseEnter={() => setPaused(true)}
      onMouseLeave={() => setPaused(false)}
      onFocus={() => setPaused(true)}
      onBlur={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget)) setPaused(false);
      }}
    >
      {notice.level === "success" ? <Icon name="check" size={14} /> : notice.level === "info" ? null : <Icon name="alert" size={14} />}
      <span className="sr-only">{t(`notices.level.${notice.level}`)}: </span>
      <span className="notice__text">{text}</span>
      {notice.level === "error" && notice.error ? (
        <button
          type="button"
          className="notice__action"
          onClick={() => {
            // Compute before dismissing; opening replaces any other dialog (an explicit user action).
            const source = notice.source ?? sourceFromAppError(notice.error as AppError, { occurredAtMs: notice.occurredAtMs });
            dismiss(id);
            openErrorDetails(source);
          }}
        >
          {t("errorReport.details")}
        </button>
      ) : null}
      <IconButton icon="close" iconSize={12} size={22} label={t("notices.dismiss")} onClick={() => dismiss(id)} />
    </div>
  );
}

/** Dismissible notices (live region): errors and short informational messages. */
export function Notices() {
  const { t } = useTranslation();
  const notices = useStore((s) => s.notices);
  // The live region stays mounted (empty) at all times: a region inserted together with its content is often not
  // announced by screen readers.
  return (
    <div className="notices" role="region" aria-label={t("notices.label")} aria-live="polite">
      {notices.map((n) => (
        <NoticeItem key={n.id} notice={n} />
      ))}
    </div>
  );
}
