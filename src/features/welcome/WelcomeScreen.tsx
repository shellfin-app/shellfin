import { useEffect, useRef, useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { Icon } from "../../components/Icon";
import { formatList, formatRelativeTime } from "../../i18n/format";
import { getBackend } from "../../ipc";
import type { RecentEntry, RecentTarget } from "../../ipc/types";
import { DIALOG_KIND, configWarningsDialog } from "../../lib/dialogs";
import { USER_HOST_SEPARATOR } from "../../lib/identifiers";
import { guard } from "../../lib/reportError";
import { displayName, displayPath } from "../../lib/serverText";
import { bookmarkTarget, quickTarget, sshConfigTarget } from "../../lib/targets";
import { useStore } from "../../state/store";
import { requestOpenSession } from "../session/sessionActions";
import { configWarningCount } from "./ConfigWarningsDialog";
import "./welcome.css";

/** Resolves what the user typed into quick connect: a bookmark / ssh-config alias by name, else a quick spec. */
export function resolveQuickConnect(
  input: string,
  aliases: readonly string[],
  bookmarks: readonly { id: string; name: string }[],
  /** `connection.defaultUser`: added to a typed target that names no user. */
  defaultUser = "",
): { target: RecentTarget; label: string } | null {
  const text = input.trim();
  if (!text) return null;
  const lower = text.toLowerCase();
  const alias = aliases.find((a) => a.toLowerCase() === lower);
  if (alias) return { target: sshConfigTarget(alias), label: alias };
  const bookmark = bookmarks.find((b) => b.name.toLowerCase() === lower);
  if (bookmark) return { target: bookmarkTarget(bookmark.id), label: bookmark.name };
  return { target: quickTarget(defaultUser !== "" && !text.includes(USER_HOST_SEPARATOR) ? `${defaultUser}@${text}` : text), label: text };
}

function RecentCard({ entry }: { entry: RecentEntry }) {
  const { t } = useTranslation();
  const isBookmark = entry.target.kind === "bookmark";
  return (
    <button type="button" className="recent-card" onClick={() => guard(requestOpenSession(entry.target, entry.label))}>
      <span className="recent-card__title">
        <Icon name={isBookmark ? "star" : "servers"} size={16} strokeWidth={isBookmark ? 1.5 : 1.8} filled={isBookmark} />
        <bdi>{displayName(entry.label)}</bdi>
      </span>
      <span className="recent-card__user mono">{displayName(entry.userAtHost)}</span>
      <span className="chips">
        <span>{formatRelativeTime(entry.lastConnectedAtMs)}</span>
        {entry.via ? <span>{t("hosts.via", { host: displayName(entry.via) })}</span> : null}
        {entry.lastDir ? <span className="mono">{displayPath(entry.lastDir)}</span> : null}
        {entry.terminalCount > 0 ? <span>{t("welcome.recentTerminals", { count: entry.terminalCount })}</span> : null}
      </span>
    </button>
  );
}

/** Welcome screen (Welcome.dc): quick connect, recents, environment chips. */
export function WelcomeScreen() {
  const { t } = useTranslation();
  const inputRef = useRef<HTMLInputElement>(null);
  // Home opens with the cursor in quick connect. Only when nothing else (a dialog, a menu) holds the focus.
  useEffect(() => {
    const active = document.activeElement;
    if (active === null || active === document.body || active.closest(".conn-tabs-wrap") !== null) inputRef.current?.focus();
  }, []);
  // The warnings dialog gives the focus back to its chip; when a reload removed the chip meanwhile, the focus would
  // fall to <body>, so it goes to quick connect instead. Runs after the dialog's own cleanup (same commit).
  const dialogKind = useStore((s) => s.dialog?.kind);
  const previousDialogKind = useRef(dialogKind);
  useEffect(() => {
    const was = previousDialogKind.current;
    previousDialogKind.current = dialogKind;
    if (was !== DIALOG_KIND.configWarnings || dialogKind === was) return;
    const active = document.activeElement;
    if (active === null || active === document.body) inputRef.current?.focus();
  }, [dialogKind]);
  const [text, setText] = useState("");
  const recents = useStore((s) => s.recents);
  const sshConfig = useStore((s) => s.sshConfig);
  const bookmarks = useStore((s) => s.bookmarks);
  const env = useStore((s) => s.env);
  const agent = useStore((s) => s.agent);
  const setSshConfig = useStore((s) => s.setSshConfig);
  const setEnv = useStore((s) => s.setEnv);
  const openDialog = useStore((s) => s.openDialog);

  const configPath = env?.sshConfig.path ?? sshConfig?.mainFile ?? "";
  const platform = useStore((s) => s.platform);
  const configMissing = env !== null && !env.sshConfig.exists;
  const configWarnings = configWarningCount(sshConfig);
  const firstRun = (sshConfig?.hosts.length ?? 0) === 0 && (bookmarks?.bookmarks.length ?? 0) === 0;

  const submit = (e: FormEvent) => {
    e.preventDefault();
    const hit = resolveQuickConnect(
      text,
      sshConfig?.hosts.map((h) => h.alias) ?? [],
      bookmarks?.bookmarks ?? [],
      useStore.getState().settings?.connection.defaultUser ?? "",
    );
    if (!hit) return;
    setText("");
    guard(requestOpenSession(hit.target, hit.label));
  };

  const reload = async () => {
    const backend = getBackend();
    setSshConfig(await backend.sshConfigReload());
    setEnv(await backend.envInfo());
  };

  return (
    <div className="welcome">
      <div className="welcome__inner">
        <div className="welcome__hero">
          <h1 className="welcome__title">{t("welcome.title")}</h1>
          <form className="quick-connect" onSubmit={submit}>
            <Icon name="search" size={18} />
            <input
              ref={inputRef}
              className="quick-connect__input mono"
              type="text"
              value={text}
              onChange={(e) => setText(e.target.value)}
              placeholder={t("welcome.quickConnect.placeholder")}
              aria-label={t("welcome.quickConnect.label")}
              autoComplete="off"
              spellCheck={false}
            />
            <kbd className="kbd mono">{t("welcome.quickConnect.enter")}</kbd>
            <button type="submit" className="sr-only" tabIndex={-1} aria-hidden="true" />
          </form>
          <div className="welcome__actions">
            <Button variant="primary" icon="plus" onClick={() => openDialog({ kind: "bookmark", bookmarkId: null })}>
              {t("shell.newConnection")}
            </Button>
            <Button
              icon="refresh"
              className="welcome__reload"
              title={t("welcome.reloadConfig", { file: configPath })}
              onClick={() => guard(reload())}
            >
              <span className="welcome__reload-label">{t("welcome.reloadConfig", { file: configPath })}</span>
            </Button>
            <Button icon="key" onClick={() => openDialog({ kind: "keys" })}>
              {t("welcome.manageKeys")}
            </Button>
          </div>
        </div>

        <section className="welcome__section" aria-labelledby="welcome-recent">
          <h2 className="sec" id="welcome-recent">
            {t("welcome.recent")}
          </h2>
          {recents.length === 0 ? (
            firstRun ? (
              <div className="welcome__first-run">
                <p className="welcome__empty">{t("welcome.firstRun")}</p>
                <Button variant="primary" icon="plus" onClick={() => openDialog({ kind: "bookmark", bookmarkId: null })}>
                  {t("welcome.firstRunAction")}
                </Button>
              </div>
            ) : (
              <p className="welcome__empty">{t("welcome.recentEmpty")}</p>
            )
          ) : (
            <div className="card-grid">
              {recents.map((r) => (
                <RecentCard key={JSON.stringify(r.target)} entry={r} />
              ))}
            </div>
          )}
        </section>

        <div className="card-grid">
          <section className="info-card" aria-labelledby="welcome-config">
            <h2 className="sec" id="welcome-config">
              {t("welcome.config")}
            </h2>
            <span className="info-card__value mono" title={configPath}>
              {configPath}
            </span>
            {configMissing ? (
              <span className="info-card__hint">{t("welcome.configMissing")}</span>
            ) : env ? (
              <span className="chips">
                <span>{t("welcome.configHosts", { count: env.sshConfig.hostCount })}</span>
                <span>{t("welcome.configIncludes", { count: env.sshConfig.includeCount })}</span>
                <span>{t("welcome.configUpdated", { time: formatRelativeTime(env.sshConfig.loadedAtMs) })}</span>
              </span>
            ) : null}
            {configWarnings > 0 ? (
              <button
                type="button"
                className="info-card__warnings"
                aria-haspopup="dialog"
                onClick={() => openDialog(configWarningsDialog())}
              >
                <Icon name="alert" size={14} strokeWidth={1.8} />
                <span>{t("welcome.configWarnings", { count: configWarnings })}</span>
              </button>
            ) : null}
          </section>
          <section className="info-card" aria-labelledby="welcome-agent">
            <h2 className="sec" id="welcome-agent">
              {t("welcome.agent")}
            </h2>
            <span className="info-card__value mono">{t("authMethod.agent")}</span>
            <span className="chips">
              <span>
                {agent?.available
                  ? t("welcome.agentKeys", {
                      count: agent.identities.length,
                      names: formatList(agent.identities.map((k) => k.comment)),
                    })
                  : t("welcome.agentUnavailable")}
              </span>
            </span>
            {agent?.available ? null : (
              <span className="info-card__hint">{platform === "windows" ? t("welcome.agentHintWindows") : t("welcome.agentHintUnix")}</span>
            )}
          </section>
          <section className="info-card" aria-labelledby="welcome-known-hosts">
            <h2 className="sec" id="welcome-known-hosts">
              {t("welcome.knownHosts")}
            </h2>
            <span className="info-card__value mono" title={env?.knownHosts.path ?? ""}>
              {env?.knownHosts.path ?? ""}
            </span>
            {env ? (
              <span className="chips">
                <span>{t("welcome.knownHostsEntries", { count: env.knownHosts.entryCount })}</span>
              </span>
            ) : null}
          </section>
        </div>
      </div>
    </div>
  );
}
