// Texts of the OS notifications (ARCHITECTURE §8.11). Rust decides and shows the notifications; it never authors prose, so
// the frontend hands over translated texts after `app_frontend_ready` and on every language change.
// `{{name}}` / `{{count}}` are the backend's placeholders: they must reach it unchanged, so they are passed as
// themselves instead of being interpolated here.
import i18next from "i18next";
import { getBackend } from "../../ipc";
import type { NotificationTexts } from "../../ipc/types";
import { NOTIFICATION_PLACEHOLDERS } from "../../lib/identifiers";

const TEXT_MAX = 200;

function clip(s: string): string {
  return Array.from(s).slice(0, TEXT_MAX).join("");
}

export function notificationTexts(): NotificationTexts {
  const t = i18next.t.bind(i18next);
  const keep = NOTIFICATION_PLACEHOLDERS;
  return {
    connectionLostTitle: clip(t("notifications.connectionLostTitle")),
    connectionLostBody: clip(t("notifications.connectionLostBody", keep)),
    transfersDoneTitle: clip(t("notifications.transfersDoneTitle")),
    transferDoneBody: clip(t("notifications.transferDoneBody", keep)),
    transfersDoneBody: clip(t("notifications.transfersDoneBody", keep)),
  };
}

/** A failure only leaves the English fallback texts in place, so it is not surfaced. */
export async function syncNotificationTexts(): Promise<void> {
  try {
    await getBackend().notificationsSetTexts(notificationTexts());
  } catch {
    // cosmetic: the backend keeps its previous (English) texts
  }
}
