import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { Dialog } from "../../components/Dialog";
import { Field, Select } from "../../components/Field";
import { Icon } from "../../components/Icon";
import { errorMessage } from "../../i18n/errorMessage";
import { toIpcError } from "../../ipc/call";
import { DIALOG_KIND } from "../../lib/dialogs";
import { displayName } from "../../lib/serverText";
import type { BookmarkExportFormat, ImportInvalid, ImportStrategy } from "../../ipc/types";
import type { BookmarksIoResult } from "../../state/slices/ui";
import { useStore } from "../../state/store";
import { exportBookmarks, importBookmarks } from "./bookmarksIo";

const SSH_CONFIG: BookmarkExportFormat = "sshConfig";
const DIALOG = DIALOG_KIND.bookmarksIo;
const RESULT_DIALOG = DIALOG_KIND.bookmarksIoResult;

/** Import: asks for the conflict strategy BEFORE the native file dialog opens. */
export function ImportBookmarksDialog() {
  const { t } = useTranslation();
  const closeDialog = useStore((s) => s.closeDialog);
  const [strategy, setStrategy] = useState<ImportStrategy>("skip");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const close = () => closeDialog(DIALOG);
  const hints: Record<ImportStrategy, string> = {
    skip: t("bookmarksIo.strategyHint.skip"),
    replace: t("bookmarksIo.strategyHint.replace"),
    duplicate: t("bookmarksIo.strategyHint.duplicate"),
  };

  const run = () => {
    setBusy(true);
    setError(null);
    importBookmarks(strategy).then(
      () => setBusy(false),
      (e: unknown) => {
        setError(errorMessage(toIpcError(e).error));
        setBusy(false);
      },
    );
  };

  return (
    <Dialog
      title={t("bookmarksIo.importTitle")}
      width={520}
      icon={<Icon name="download" size={20} strokeWidth={1.8} />}
      onClose={close}
      footer={
        <>
          {error ? <span className="dialog__error">{error}</span> : null}
          <span className="spacer" />
          <Button onClick={close} disabled={busy}>
            {t("common.cancel")}
          </Button>
          <Button variant="primary" onClick={run} disabled={busy} data-autofocus>
            {t("bookmarksIo.chooseFile")}
          </Button>
        </>
      }
    >
      <p className="dialog__text">{t("bookmarksIo.importIntro")}</p>
      <Field label={t("bookmarksIo.strategy")} hint={hints[strategy]}>
        <Select value={strategy} onChange={(e) => setStrategy(e.target.value as ImportStrategy)}>
          <option value="skip">{t("bookmarksIo.strategyOption.skip")}</option>
          <option value="replace">{t("bookmarksIo.strategyOption.replace")}</option>
          <option value="duplicate">{t("bookmarksIo.strategyOption.duplicate")}</option>
        </Select>
      </Field>
      <p className="dialog__hint">{t("bookmarksIo.noPasswords")}</p>
    </Dialog>
  );
}

/** Export: format choice and the note that passwords are never exported, then the native Save dialog. */
export function ExportBookmarksDialog() {
  const { t } = useTranslation();
  const closeDialog = useStore((s) => s.closeDialog);
  const [format, setFormat] = useState<BookmarkExportFormat>("json");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const close = () => closeDialog(DIALOG);
  const hints: Record<BookmarkExportFormat, string> = {
    json: t("bookmarksIo.formatHint.json"),
    sshConfig: t("bookmarksIo.formatHint.sshConfig"),
  };

  const run = () => {
    setBusy(true);
    setError(null);
    exportBookmarks(format).then(
      () => setBusy(false),
      (e: unknown) => {
        setError(errorMessage(toIpcError(e).error));
        setBusy(false);
      },
    );
  };

  return (
    <Dialog
      title={t("bookmarksIo.exportTitle")}
      width={520}
      icon={<Icon name="upload" size={20} strokeWidth={1.8} />}
      onClose={close}
      footer={
        <>
          {error ? <span className="dialog__error">{error}</span> : null}
          <span className="spacer" />
          <Button onClick={close} disabled={busy}>
            {t("common.cancel")}
          </Button>
          <Button variant="primary" onClick={run} disabled={busy} data-autofocus>
            {t("bookmarksIo.exportAction")}
          </Button>
        </>
      }
    >
      <p className="dialog__text">{t("bookmarksIo.exportIntro")}</p>
      <Field label={t("bookmarksIo.format")} hint={hints[format]}>
        <Select value={format} onChange={(e) => setFormat(e.target.value as BookmarkExportFormat)}>
          <option value="json">{t("bookmarksIo.formatOption.json")}</option>
          <option value={SSH_CONFIG}>{t("bookmarksIo.formatOption.sshConfig")}</option>
        </Select>
      </Field>
      <p className="dialog__warn">{t("bookmarksIo.passwordsNeverExported")}</p>
    </Dialog>
  );
}

/** Readable text for an import problem (`entry` + `code`; the field path is technical and shown as code). */
function ProblemRow({ problem }: { problem: ImportInvalid }) {
  const { t } = useTranslation();
  const n = problem.index + 1; // the file's arrays are shown 1-based
  return (
    <li>
      {t(`importEntry.${problem.entry}`, { n })}: <code>{problem.field}</code> {t(`importInvalidCode.${problem.code}`)}
    </li>
  );
}

/** Summary after an import (added / replaced / skipped / every rejected entry) or an export. */
export function BookmarksIoResultDialog({ result }: { result: BookmarksIoResult }) {
  const { t } = useTranslation();
  const closeDialog = useStore((s) => s.closeDialog);
  const close = () => closeDialog(RESULT_DIALOG);

  if (result.mode === "import") {
    const { added, replaced, skipped, invalid, startupCommandsStripped } = result.data;
    return (
      <Dialog
        title={t("bookmarksIo.importDoneTitle")}
        width={520}
        icon={<Icon name="check" size={20} strokeWidth={1.8} />}
        onClose={close}
        footer={
          <>
            <span className="spacer" />
            <Button variant="primary" onClick={close} data-autofocus>
              {t("common.close")}
            </Button>
          </>
        }
      >
        <ul className="bookmarks-io__counts" aria-label={t("bookmarksIo.summary")}>
          <li>{t("bookmarksIo.added", { count: added })}</li>
          <li>{t("bookmarksIo.replaced", { count: replaced })}</li>
          <li>{t("bookmarksIo.skipped", { count: skipped })}</li>
          <li>{t("bookmarksIo.invalid", { count: invalid.length })}</li>
        </ul>
        {startupCommandsStripped.length > 0 ? (
          <>
            <p className="dialog__warn">{t("bookmarksIo.startupStripped", { count: startupCommandsStripped.length })}</p>
            <ul className="bookmarks-io__problems" aria-label={t("bookmarksIo.startupStrippedList")}>
              {startupCommandsStripped.map((name, i) => (
                <li key={`${name}:${i}`}>
                  <bdi>{displayName(name)}</bdi>
                </li>
              ))}
            </ul>
            <p className="dialog__hint">{t("bookmarksIo.startupStrippedHint")}</p>
          </>
        ) : null}
        {invalid.length > 0 ? (
          <>
            <p className="dialog__warn">{t("bookmarksIo.invalidIntro")}</p>
            <ul className="bookmarks-io__problems">
              {invalid.map((p, i) => (
                <ProblemRow key={`${p.entry}:${p.index}:${p.field}:${i}`} problem={p} />
              ))}
            </ul>
          </>
        ) : null}
      </Dialog>
    );
  }

  const { count, fileName, skipped } = result.data;
  return (
    <Dialog
      title={t("bookmarksIo.exportDoneTitle")}
      width={520}
      icon={<Icon name="check" size={20} strokeWidth={1.8} />}
      onClose={close}
      footer={
        <>
          <span className="spacer" />
          <Button variant="primary" onClick={close} data-autofocus>
            {t("common.close")}
          </Button>
        </>
      }
    >
      <p className="dialog__text">{t("bookmarksIo.exported", { count, file: displayName(fileName ?? "") })}</p>
      <p className="dialog__hint">{t("bookmarksIo.passwordsNeverExported")}</p>
      {skipped.length > 0 ? (
        <>
          <p className="dialog__warn">{t("bookmarksIo.exportSkippedIntro")}</p>
          <ul className="bookmarks-io__problems">
            {skipped.map((s, i) => (
              <li key={`${s.name}:${s.field}:${i}`}>
                <bdi>{displayName(s.name)}</bdi>: <code>{s.field}</code> {t(`exportSkipCode.${s.code}`)}
              </li>
            ))}
          </ul>
        </>
      ) : null}
    </Dialog>
  );
}

