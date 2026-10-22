import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { Dialog } from "../../components/Dialog";
import { Checkbox, Field, Select } from "../../components/Field";
import { Icon } from "../../components/Icon";
import { errorMessage } from "../../i18n/errorMessage";
import { getBackend } from "../../ipc";
import { toIpcError } from "../../ipc/call";
import { DIALOG_KIND } from "../../lib/dialogs";
import { displayName, displayServerText } from "../../lib/serverText";
import type { BookmarkExportFormat, BookmarksImportPreview, ImportInvalid, ImportStrategy } from "../../ipc/types";
import type { BookmarksIoResult } from "../../state/slices/ui";
import { useStore } from "../../state/store";
import { applyBookmarksImport, cancelBookmarksImport, exportBookmarks } from "./bookmarksIo";

const SSH_CONFIG: BookmarkExportFormat = "sshConfig";
const DIALOG = DIALOG_KIND.bookmarksIo;
const DIALOG_IMPORT = DIALOG_KIND.bookmarksImport;
const RESULT_DIALOG = DIALOG_KIND.bookmarksIoResult;

/**
 * A startup command shown for consent is never cut: escaping happens before any truncation, so a bound could hide
 * the tail behind invisible characters while the whole command is imported. The backend caps commands at 1024
 * characters; the list wraps and scrolls instead.
 */
const COMMAND_DISPLAY_LIMIT = Number.MAX_SAFE_INTEGER;

/**
 * Import review (after the native file dialog): what the file holds, the conflict strategy and, only when the file
 * has startup commands, the opt-in to keep them with the affected bookmarks and commands listed. Nothing is saved
 * before "Import"; every other way out (Cancel, Escape, close button) drops the pending import. Names and commands
 * come from the file: text only, through `displayName` / `displayServerText`.
 */
export function ImportBookmarksDialog({ preview, token }: { preview: BookmarksImportPreview; token: string }) {
  const { t } = useTranslation();
  const closeDialog = useStore((s) => s.closeDialog);
  const [strategy, setStrategy] = useState<ImportStrategy>("skip");
  const [keep, setKeep] = useState(false);
  const [busy, setBusy] = useState(false);
  // An apply attempt (even a failed one) consumes the pending import: nothing is left to retry or cancel.
  const [consumed, setConsumed] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const hints: Record<ImportStrategy, string> = {
    skip: t("bookmarksIo.strategyHint.skip"),
    replace: t("bookmarksIo.strategyHint.replace"),
    duplicate: t("bookmarksIo.strategyHint.duplicate"),
  };
  const commands = preview.startupCommands;

  // `settled`: the pending import was applied (any attempt) or cancelled, so nothing is left to cancel.
  const settled = useRef(false);
  const mounted = useRef(false);
  const backend = useRef(getBackend()); // the deferred cleanup must not reach a later backend
  const cancelOnce = () => {
    if (settled.current) return;
    settled.current = true;
    cancelBookmarksImport(token, backend.current);
  };
  // The dialog can vanish without its close handler (another dialog replaces it): drop the pending import then.
  // The check is deferred so a StrictMode re-mount (dev) does not cancel a live dialog.
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      setTimeout(() => {
        if (!mounted.current) cancelOnce();
      }, 0);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const close = () => {
    cancelOnce();
    closeDialog(DIALOG_IMPORT);
  };

  const run = () => {
    setBusy(true);
    setConsumed(true);
    settled.current = true;
    setError(null);
    applyBookmarksImport(token, strategy, keep && commands.length > 0).then(
      () => setBusy(false),
      (e: unknown) => {
        const err = toIpcError(e).error;
        const stale = err.code === "invalidArgument" && err.params.field === "token";
        setError(stale ? t("bookmarksIo.importStale") : errorMessage(err));
        setBusy(false);
      },
    );
  };

  const mark = (exists: boolean): string | null => {
    if (!exists) return null;
    if (strategy === "skip") return t("bookmarksIo.commandSkipped");
    if (strategy === "replace") return t("bookmarksIo.commandReplaces");
    return null;
  };

  return (
    <Dialog
      title={t("bookmarksIo.importTitle")}
      width={560}
      icon={<Icon name="download" size={20} strokeWidth={1.8} />}
      onClose={busy ? undefined : close}
      showClose
      footer={
        <>
          {error ? <span className="dialog__error">{error}</span> : null}
          <span className="spacer" />
          <Button onClick={close} disabled={busy}>
            {consumed && error ? t("common.close") : t("common.cancel")}
          </Button>
          <Button variant="primary" onClick={run} disabled={busy || consumed} data-autofocus>
            {t("bookmarksIo.importAction")}
          </Button>
        </>
      }
    >
      <p className="dialog__text">{t("bookmarksIo.fileBookmarks", { count: preview.bookmarks })}</p>
      {preview.conflicts > 0 ? <p className="dialog__text">{t("bookmarksIo.fileConflicts", { count: preview.conflicts })}</p> : null}
      {preview.invalid.length > 0 ? (
        <>
          <p className="dialog__warn">{t("bookmarksIo.invalidIntro")}</p>
          <ul className="bookmarks-io__problems">
            {preview.invalid.map((p, i) => (
              <ProblemRow key={`${p.entry}:${p.index}:${p.field}:${i}`} problem={p} />
            ))}
          </ul>
        </>
      ) : null}
      <Field label={t("bookmarksIo.strategy")} hint={hints[strategy]}>
        <Select value={strategy} onChange={(e) => setStrategy(e.target.value as ImportStrategy)} disabled={busy}>
          <option value="skip">{t("bookmarksIo.strategyOption.skip")}</option>
          <option value="replace">{t("bookmarksIo.strategyOption.replace")}</option>
          <option value="duplicate">{t("bookmarksIo.strategyOption.duplicate")}</option>
        </Select>
      </Field>
      {commands.length > 0 ? (
        <>
          <Checkbox
            label={t("bookmarksIo.keepStartup")}
            hint={t("bookmarksIo.keepStartupHint")}
            checked={keep}
            onChange={setKeep}
            disabled={busy}
          />
          {keep ? (
            <>
              <p className="dialog__warn" role="alert">
                {t("bookmarksIo.keepStartupWarning")}
              </p>
              {/* focusable so a keyboard user can scroll a long list */}
              <ul
                className="bookmarks-io__problems bookmarks-io__commands"
                aria-label={t("bookmarksIo.keepStartupList")}
                tabIndex={0}
              >
                {commands.map((c, i) => {
                  const note = mark(c.exists);
                  return (
                    <li key={`${c.name}:${i}`}>
                      <bdi>{displayName(c.name)}</bdi>
                      {note ? <span className="bookmarks-io__mark"> {note}</span> : null}
                      <br />
                      <code>{displayServerText(c.command, COMMAND_DISPLAY_LIMIT)}</code>
                    </li>
                  );
                })}
              </ul>
            </>
          ) : null}
        </>
      ) : null}
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

