import { useTranslation } from "react-i18next";
import { Icon } from "../../components/Icon";
import { StatusDot, dotForPhase } from "../../components/StatusDot";
import { formatRate } from "../../i18n/format";
import { formatEndpoint } from "../../lib/endpoint";
import { encodingLabel, eolLabel, languageForPath } from "../../lib/languages";
import { useStore } from "../../state/store";
import { toggleEol } from "../editor/editorActions";
import { sessionTitle } from "../session/sessionTitle";
import { TransfersButton } from "../transfers/TransfersPopover";
import { PanelToggles } from "./PanelToggles";

function WelcomeStatus() {
  const { t } = useTranslation();
  const version = useStore((s) => s.appVersion);
  return (
    <footer className="status-bar mono">
      <PanelToggles withTerminal={false} />
      <span className="status-bar__item">
        <StatusDot state="idle" label={t("status.notConnected")} />
        {t("status.notConnected")}
      </span>
      <span className="status-bar__right">{t("status.version", { version })}</span>
    </footer>
  );
}

function SessionStatus({ sessionId }: { sessionId: string }) {
  const { t } = useTranslation();
  const entry = useStore((s) => s.sessionsById[sessionId]);
  const stats = useStore((s) => s.sessionStats[sessionId]);
  const cursor = useStore((s) => {
    const id = s.workspaces[sessionId]?.activeBuffer;
    return id ? s.cursors[id] : undefined;
  });
  const buffer = useStore((s) => {
    const id = s.workspaces[sessionId]?.activeBuffer;
    return id ? s.buffers[id] : undefined;
  });
  if (!entry) return null;

  const info = entry.info;
  const hop = info?.hops[info.hops.length - 1];
  const unverified = info?.hops.some((h) => h.hostKey.verification === "autoAccepted") ?? false;
  const connected = entry.state.phase === "connected";

  return (
    <footer className="status-bar mono">
      <span className="status-bar__item status-bar__item--strong">
        <StatusDot
          state={dotForPhase(entry.state.phase)}
          label={t(`session.state.${entry.state.phase}`)}
        />
        {hop ? <bdi>{formatEndpoint(hop.user, hop.host, hop.port)}</bdi> : <bdi>{sessionTitle(entry, t)}</bdi>}
      </span>
      {unverified ? <span className="status-bar__chip">{t("status.hostKeyUnverified")}</span> : null}
      {info?.auth.keyType ? (
        <span className="status-bar__pair">
          <span>
            <bdi>{info.auth.keyType}</bdi>
          </span>
          {info.auth.keySource ? (
            <span>
              <bdi>{info.auth.keySource}</bdi>
            </span>
          ) : null}
        </span>
      ) : null}
      {info?.sftpAvailable ? <span>{t("status.sftp")}</span> : null}
      {connected ? (
        <span>{t("status.rates", { tx: formatRate(stats?.txBytesPerSec ?? 0), rx: formatRate(stats?.rxBytesPerSec ?? 0) })}</span>
      ) : null}
      <span className="status-bar__group">
        <PanelToggles withTerminal />
        <TransfersButton sessionId={sessionId} />
        {buffer && buffer.load === "ready" ? (
          <>
            <span>{t("status.cursor", { line: cursor?.line ?? 1, col: cursor?.col ?? 1 })}</span>
            <span>{languageForPath(buffer.path).label ?? t("editor.language.plain")}</span>
            <button
              type="button"
              className="status-bar__button"
              title={t("status.encodingAction")}
              aria-label={t("status.encodingLabel", { encoding: encodingLabel(buffer.encoding, buffer.hasBom) })}
              onClick={() => useStore.getState().openDialog({ kind: "encoding", bufferId: buffer.id })}
            >
              {encodingLabel(buffer.encoding, buffer.hasBom)}
            </button>
            <button
              type="button"
              className="status-bar__button"
              title={buffer.mixedEol ? t("status.eolMixed") : t("status.eolAction")}
              aria-label={t("status.eolLabel", { eol: eolLabel(buffer.eol) })}
              onClick={() => toggleEol(buffer.id)}
            >
              {eolLabel(buffer.eol)}
              {buffer.mixedEol ? <Icon name="alert" size={11} /> : null}
            </button>
          </>
        ) : null}
      </span>
    </footer>
  );
}

/** Status bar (Main.dc bottom strip; Welcome.dc variant "Not connected · v0.1.0"). */
export function StatusBar() {
  const activeId = useStore((s) => s.activeSessionId);
  return activeId ? <SessionStatus sessionId={activeId} /> : <WelcomeStatus />;
}
