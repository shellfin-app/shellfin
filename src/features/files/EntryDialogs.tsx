import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { Dialog } from "../../components/Dialog";
import { Field, Input } from "../../components/Field";
import { Icon } from "../../components/Icon";
import { errorMessage } from "../../i18n/errorMessage";
import { toIpcError } from "../../ipc/call";
import { DIALOG_KIND } from "../../lib/dialogs";
import { basename, dirname } from "../../lib/remotePath";
import { displayName, displayPath } from "../../lib/serverText";
import { useStore } from "../../state/store";
import { reportError } from "../../lib/reportError";
import { deleteEntry, goToFolder, moveEntry, removeHint } from "./fileActions";

/** Delete with confirmation (files, and folders that are empty: there is no recursive delete in v1). */
export function DeleteEntryDialog({ sessionId, path, entry }: { sessionId: string; path: string; entry: "file" | "dir" }) {
  const { t } = useTranslation();
  const closeDialog = useStore((s) => s.closeDialog);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const name = displayName(basename(path));
  const close = () => closeDialog(DIALOG_KIND.deleteEntry);
  const probablyDenied = removeHint(sessionId, path) === "no";

  const run = () => {
    setBusy(true);
    setError(null);
    deleteEntry(sessionId, path).then(
      () => close(),
      (e: unknown) => {
        // `directoryNotEmpty` and friends are explained inside the dialog, which stays open.
        setError(errorMessage(toIpcError(e).error));
        setBusy(false);
      },
    );
  };

  return (
    <Dialog
      title={entry === "dir" ? t("files.deleteFolderTitle", { name }) : t("files.deleteFileTitle", { name })}
      subtitle={<bdi>{displayPath(path)}</bdi>}
      tone="danger"
      role="alertdialog"
      width={460}
      icon={<Icon name="trash" size={20} strokeWidth={1.8} />}
      onClose={close}
      footer={
        <>
          <Button onClick={close} disabled={busy} data-autofocus>
            {t("common.cancel")}
          </Button>
          <Button variant="primary" onClick={run} disabled={busy}>
            {t("common.delete")}
          </Button>
        </>
      }
    >
      <p className="dialog__text">{entry === "dir" ? t("files.deleteFolderBody") : t("files.deleteFileBody")}</p>
      {probablyDenied ? <p className="dialog__warn">{t("files.deleteMayFail")}</p> : null}
      {error ? (
        <p className="dialog__error" role="alert">
          {error}
        </p>
      ) : null}
    </Dialog>
  );
}

/** Move into another folder: the context-menu replacement for drag-to-move (HTML5 DnD is unavailable on Windows). */
export function MoveEntryDialog({ sessionId, path }: { sessionId: string; path: string }) {
  const { t } = useTranslation();
  const closeDialog = useStore((s) => s.closeDialog);
  const [dest, setDest] = useState(dirname(path));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const name = displayName(basename(path));
  const close = () => closeDialog(DIALOG_KIND.moveEntry);
  const probablyDenied = removeHint(sessionId, path) === "no";

  const run = () => {
    setBusy(true);
    setError(null);
    moveEntry(sessionId, path, dest.trim()).then(
      () => close(),
      (e: unknown) => {
        setError(errorMessage(toIpcError(e).error));
        setBusy(false);
      },
    );
  };

  return (
    <Dialog
      title={t("files.moveTitle", { name })}
      subtitle={<bdi>{displayPath(path)}</bdi>}
      width={500}
      icon={<Icon name="folder" size={20} strokeWidth={1.8} />}
      onClose={close}
      footer={
        <>
          <Button onClick={close} disabled={busy}>
            {t("common.cancel")}
          </Button>
          <Button variant="primary" onClick={run} disabled={busy || dest.trim() === ""}>
            {t("files.moveAction")}
          </Button>
        </>
      }
    >
      <form
        onSubmit={(e) => {
          e.preventDefault();
          if (!busy && dest.trim() !== "") run();
        }}
      >
        <Field label={t("files.moveDestination")} hint={probablyDenied ? `${t("files.moveHint")} ${t("files.moveMayFail")}` : t("files.moveHint")} error={error}>
          <Input mono value={dest} onChange={(e) => setDest(e.target.value)} data-autofocus spellCheck={false} autoComplete="off" />
        </Field>
      </form>
    </Dialog>
  );
}

/** "Go to folder…": re-roots the file tree at a typed path. Bad input is explained inline, server errors become notices. */
export function GoToFolderDialog({ sessionId }: { sessionId: string }) {
  const { t } = useTranslation();
  const closeDialog = useStore((s) => s.closeDialog);
  const root = useStore((s) => s.workspaces[sessionId]?.tree.root ?? "");
  const [value, setValue] = useState(root);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const close = () => closeDialog(DIALOG_KIND.goToFolder);

  const run = () => {
    if (busy || value.trim() === "") return;
    setBusy(true);
    setError(null);
    goToFolder(sessionId, value).then(
      () => close(),
      (e: unknown) => {
        const { error: appError } = toIpcError(e);
        if (appError.code === "invalidArgument") setError(errorMessage(appError));
        else reportError(e);
        setBusy(false);
      },
    );
  };

  return (
    <Dialog
      title={t("files.goToFolderTitle")}
      width={500}
      icon={<Icon name="folder" size={20} strokeWidth={1.8} />}
      onClose={close}
      footer={
        <>
          <Button onClick={close} disabled={busy}>
            {t("common.cancel")}
          </Button>
          <Button variant="primary" onClick={run} disabled={busy || value.trim() === ""}>
            {t("files.goToFolderAction")}
          </Button>
        </>
      }
    >
      <form
        onSubmit={(e) => {
          e.preventDefault();
          run();
        }}
      >
        <Field label={t("files.goToFolderLabel")} hint={t("files.goToFolderHint")} error={error}>
          <Input
            mono
            value={value}
            onChange={(e) => setValue(e.target.value)}
            onFocus={(e) => e.currentTarget.select()}
            data-autofocus
            spellCheck={false}
            autoComplete="off"
          />
        </Field>
      </form>
    </Dialog>
  );
}
