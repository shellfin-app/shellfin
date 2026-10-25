import i18next from "i18next";
import { resolveLocale, setLanguage, setPseudoLocale } from "./i18n";
import { syncNativeMenuLabels } from "./features/shell/nativeMenuLabels";
import { syncNotificationTexts } from "./features/shell/notificationTexts";
import { installGlobalErrorHandlers } from "./features/errorReport/errorReport";
import { installAutoRefresh } from "./features/files/autoRefresh";
import { restoreBookmarkSessions } from "./features/session/restoreSessions";
import { guard } from "./lib/reportError";
import { initBackend } from "./ipc";
import { toIpcError } from "./ipc/call";
import { installDropHandlers } from "./features/files/dropFiles";
import { installQuitFlow } from "./features/session/quitFlow";
import { installShellEvents } from "./features/shell/shellEvents";
import { installUpdateEvents } from "./features/updates/updateActions";
import { installResumeProbe } from "./features/session/resumeProbe";
import { openSession } from "./features/session/sessionActions";
import { sshConfigTarget } from "./lib/targets";
import { useStore } from "./state/store";

let listenersInstalled = false;
const installedListeners = new Set<string>();
let localeSubscribed = false;

async function applyLanguage(locale: string): Promise<boolean> {
  if (i18next.language === "en-XA") return false; // dev pseudo-locale wins
  await setLanguage(resolveLocale(locale));
  return true;
}

async function applyLocale(locale: string): Promise<void> {
  if (!(await applyLanguage(locale))) return;
  await syncNativeMenuLabels(); // macOS only: startup and every language change (ARCHITECTURE §8.2.3)
  await syncNotificationTexts(); // texts of the OS notifications, same moments (ARCHITECTURE §8.11)
}

/**
 * Startup (§8.9, §4.2). The quit listener and the global event listeners are registered BEFORE
 * `app_frontend_ready`, so a failing or slow bootstrap can still be quit through the confirm flow and no
 * `ssh-config:changed` / `recents:changed` event is lost between the snapshot and the listener.
 * Throws on failure; use `startApp` to turn that into the retry screen.
 */
export async function bootstrap(): Promise<void> {
  installGlobalErrorHandlers(); // uncaught errors and unhandled rejections open the error details dialog (§8.14)
  const backend = await initBackend();

  if (!listenersInstalled) {
    // Each listener is tracked on its own: when one fails and the user presses Retry, only the missing ones are
    // registered, so none ends up registered twice.
    const steps: [string, () => Promise<unknown>][] = [
      ["recents", () => backend.listen("recents:changed", (recents) => useStore.getState().setRecents(recents))],
      ["sshConfig", () => backend.listen("ssh-config:changed", (snapshot) => useStore.getState().setSshConfig(snapshot))],
      ["quit", () => installQuitFlow()],
      ["drop", () => installDropHandlers()],
      ["shell", () => installShellEvents()],
      ["update", () => installUpdateEvents()], // updater: `update:available`, `update:install-failed`
    ];
    const results = await Promise.allSettled(
      steps
        .filter(([name]) => !installedListeners.has(name))
        .map(async ([name, install]) => {
          await install();
          installedListeners.add(name);
        }),
    );
    for (const result of results) if (result.status === "rejected") throw result.reason;
    installResumeProbe(); // session_probe_all on `online` / after sleep or a long hidden spell (§5.7)
    installAutoRefresh(); // `files.autoRefresh`: expanded folders, follows the setting
    listenersInstalled = true;
  }

  const data = await backend.appFrontendReady();
  // The language is applied BEFORE the shell renders (`setBootstrap` marks it ready), so a Ukrainian user never sees
  // the English UI first. The strings are bundled / a local lazy chunk, so this adds no network wait.
  await applyLanguage(data.settings.locale);
  useStore.getState().setBootstrap(data);
  // Startup warnings (quarantined/read-only storage, unreadable files ...) must not stay invisible.
  for (const warning of data.warnings) useStore.getState().pushNotice(warning, "warning");

  // Settings.locale -> UI language (now and whenever settings change).
  await applyLocale(data.settings.locale);
  if (!localeSubscribed) {
    localeSubscribed = true;
    useStore.subscribe((s, prev) => {
      if (s.settings && s.settings.locale !== prev.settings?.locale) void applyLocale(s.settings.locale);
    });
  }

  // `general.onStartup = "restore"`: the backend sends the bookmarks open at the last quit. Not awaited: the
  // confirmation and credential prompts wait for the user.
  if (data.restoreBookmarkIds.length > 0) guard(restoreBookmarkSessions(data.restoreBookmarkIds));

  // Environment chips on the welcome screen; failures must not block the UI.
  backend.envInfo().then((env) => useStore.getState().setEnv(env)).catch(() => undefined);
  backend.agentStatus().then((agent) => useStore.getState().setAgent(agent)).catch(() => undefined);
}

/** Runs `bootstrap`; on failure the UI shows the retry screen instead of staying blank. */
export async function startApp(): Promise<void> {
  try {
    await bootstrap();
  } catch (e) {
    useStore.getState().setBootError(toIpcError(e).error);
  }
}

/** Dev-only URL switches: `?open=prod-web-01,staging-db` opens mock sessions, `?xa=1` enables en-XA. */
export async function applyDevSwitches(search: string): Promise<void> {
  if (!import.meta.env.DEV) return;
  const params = new URLSearchParams(search);
  if (params.get("xa") === "1") await setPseudoLocale(true);
  const open = params.get("open");
  if (open) {
    const hosts = useStore.getState().sshConfig?.hosts ?? [];
    for (const alias of open.split(",")) {
      const host = hosts.find((h) => h.alias === alias);
      if (host) await openSession(sshConfigTarget(host.alias), host.alias);
    }
  }
}
