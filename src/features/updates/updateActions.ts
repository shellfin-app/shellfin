// Updater (docs/ARCHITECTURE.md §9.8): the frontend half of the update flow. The backend checks, downloads,
// verifies and installs; this module only shows the offer, forwards the user's choice and follows the progress.
//   update:available (automatic check)  -> the update dialog, or an info notice when another modal is open;
//   "Check for updates…" (manual)       -> the dialog, or "You're up to date";
//   "Update and restart"                -> update_install (progress) -> the backend starts the normal quit flow,
//                                          which asks about unsaved changes; cancelling it keeps the app running.
import { getBackend } from "../../ipc";
import { toIpcError } from "../../ipc/call";
import type { Unlisten } from "../../ipc";
import type { AppError, UpdateInfo } from "../../ipc/types";
import { t } from "../../i18n/translate";
import { UP_TO_DATE, updateAvailableNotice } from "../../lib/notices";
import { guard } from "../../lib/reportError";
import { useStore } from "../../state/store";
import { isModalOpen } from "../session/promptVisibility";

/** Show `info` in the update dialog. */
function offer(info: UpdateInfo): void {
  const s = useStore.getState();
  s.setUpdateInfo(info);
  s.openDialog({ kind: "update" });
}

/**
 * An automatic check found `info` (never a skipped version: the backend filters those). Never replaces a dialog,
 * the palette or a credential prompt the user is working in: then a short notice points to Help instead.
 */
export function announceUpdate(info: UpdateInfo): void {
  const s = useStore.getState();
  if (s.update && s.update.phase !== "available") return; // a download or install is already under way
  if (isModalOpen(s)) {
    s.setUpdateInfo(info);
    s.pushInfo(updateAvailableNotice(info.version));
    return;
  }
  offer(info);
}

/** Help > Check for updates… (also the palette and the macOS app menu). */
export async function checkForUpdates(): Promise<void> {
  const s = useStore.getState();
  if (s.update && (s.update.phase === "downloading" || s.update.phase === "restarting")) {
    s.openDialog({ kind: "update" });
    return;
  }
  const outcome = await getBackend().updateCheck();
  if (outcome.status === "available") offer(outcome.update);
  else {
    useStore.getState().setUpdateInfo(null);
    useStore.getState().pushInfo(UP_TO_DATE);
  }
}

/** "Update and restart". Errors stay in the dialog (rendered through `errors.<code>`); `cancelled` goes back to the offer. */
export async function installUpdate(): Promise<void> {
  const current = useStore.getState().update;
  if (!current || current.phase === "downloading" || current.phase === "restarting") return;
  const { version } = current.info;
  useStore.getState().setUpdatePhase("downloading");
  try {
    await getBackend().updateInstall(version, (event) => {
      const s = useStore.getState();
      if (event.phase === "downloading") s.setUpdateProgress(event.downloaded, event.total);
      else s.setUpdatePhase("restarting");
    });
    // The quit flow is running now (the backend emitted `app:quit-requested`).
    if (useStore.getState().update?.phase === "downloading") useStore.getState().setUpdatePhase("restarting");
  } catch (e) {
    const error = toIpcError(e).error;
    if (error.code === "cancelled") useStore.getState().setUpdatePhase("available");
    else useStore.getState().setUpdatePhase("failed", error);
  }
}

/** Cancel during the download: the backend stops it and `installUpdate` returns to the offer. */
export function cancelUpdateDownload(): void {
  guard(getBackend().updateCancel());
}

/** "Later": closes the dialog; the next automatic check (or a manual one) offers it again. */
export function updateLater(): void {
  const s = useStore.getState();
  s.closeDialog("update");
  if (s.update?.phase === "failed") s.setUpdatePhase("available");
}

/** "Skip this version": automatic checks stop announcing it (a manual check still shows it). */
export async function skipUpdateVersion(): Promise<void> {
  const current = useStore.getState().update;
  if (!current) return;
  await getBackend().updateSkipVersion(current.info.version);
  const s = useStore.getState();
  s.closeDialog("update");
  s.setUpdateInfo(null);
}

/** Open the release page through the backend's confirmed opener (§9.4): the Linux .deb/.rpm download and the
 * "Release notes on GitHub" link of the update dialog. */
export function openReleasePage(): void {
  const current = useStore.getState().update;
  if (!current) return;
  const url = current.info.releaseUrl;
  guard(getBackend().appOpenExternal(url, t("help.openTitle"), t("help.openBody", { url })));
}

/**
 * A release-notes link: `url` is the `href` of the URL the allowlist accepted (`allowedReleaseLink`), so the confirm
 * body shows exactly what is opened. The backend re-checks the basics and confirms (§9.4).
 */
export function openNotesLink(url: string): void {
  guard(getBackend().appOpenExternal(url, t("help.openTitle"), t("help.openBody", { url })));
}

/** The user cancelled the quit that "Update and restart" started: the staged update is dropped by the backend. */
export function onUpdateQuitCancelled(): void {
  const s = useStore.getState();
  if (s.update?.phase === "restarting") s.setUpdatePhase("available");
}

/** The verified update could not be installed at the end of the quit flow; the app keeps running. */
function onInstallFailed(error: AppError): void {
  const s = useStore.getState();
  if (!s.update) {
    s.pushNotice(error);
    return;
  }
  s.setUpdatePhase("failed", error);
  s.openDialog({ kind: "update" });
}

/** Registers the updater's global events (bootstrap). */
export async function installUpdateEvents(): Promise<Unlisten[]> {
  const backend = getBackend();
  return Promise.all([
    backend.listen("update:available", (info) => announceUpdate(info)),
    backend.listen("update:install-failed", (error) => onInstallFailed(error)),
  ]);
}
