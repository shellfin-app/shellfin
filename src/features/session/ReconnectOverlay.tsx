import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useShallow } from "zustand/react/shallow";
import { Button } from "../../components/Button";
import { Icon } from "../../components/Icon";
import { Progress } from "../../components/Progress";
import { errorMessage } from "../../i18n/errorMessage";
import { formatDateTime, formatList } from "../../i18n/format";
import { basename } from "../../lib/remotePath";
import { guard } from "../../lib/reportError";
import { displayServerText } from "../../lib/serverText";
import { useStore } from "../../state/store";
import { HostKeyChangedHelp, HostKeyMismatchHelp } from "./SessionErrorPanel";
import { reconnectNow, requestCloseSession } from "./sessionActions";

/** Re-renders every `ms` while `active`; returns the current time. */
function useNow(active: boolean, ms = 250): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return undefined;
    const id = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(id);
  }, [active, ms]);
  return now;
}

/**
 * Connection-lost overlay over the terminal panel (Reconnect.dc, §5.8): stays up through the whole auto-reconnect
 * (`waiting` countdown -> `connecting` -> `authenticating`), and switches to the final state when attempts are
 * exhausted or the failure is fatal (`disconnected`). Editor buffers stay untouched underneath.
 */
export function ReconnectOverlay({ sessionId, fill = false }: { sessionId: string; fill?: boolean }) {
  const { t } = useTranslation();
  const entry = useStore((s) => s.sessionsById[sessionId]);
  const dirtyPaths = useStore(
    useShallow((s) =>
      (s.workspaces[sessionId]?.editorTabs ?? []).flatMap((id) => {
        const b = s.buffers[id];
        return b?.dirty ? [b.path] : [];
      }),
    ),
  );
  const state = entry?.state;
  const waiting = state?.phase === "reconnecting" && state.stage === "waiting";
  const now = useNow(waiting);
  if (!entry || !state || (state.phase !== "reconnecting" && state.phase !== "disconnected")) return null;

  const dirtyNames = dirtyPaths.map((path) => displayServerText(basename(path), 128));
  const host = displayServerText(entry.label, 256);
  const cause = state.phase === "reconnecting" ? state.cause : state.error;
  const lostAt = entry.lostAtMs;
  // "Disconnect" by the user: not a failure, the tab stays and offers Reconnect.
  const byUser = cause?.code === "disconnected" && cause.params.reason === "userRequested";
  // The title already says "connection lost": repeat the cause only when it adds something.
  const showCause = cause !== null && cause !== undefined && !(cause.code === "disconnected" && !cause.params.message);

  let status: string | null = null;
  let progress = 1;
  let attempt: string | null = null;
  if (state.phase === "reconnecting") {
    attempt = t("reconnect.attempt", { attempt: state.attempt, max: state.maxAttempts });
    if (state.stage === "waiting") {
      const next = state.nextAttemptAtMs ?? now;
      const since = entry.stageSinceMs ?? now;
      const total = Math.max(1, next - since);
      status = t("reconnect.countdown", { seconds: Math.max(0, Math.ceil((next - now) / 1000)) });
      progress = Math.min(1, Math.max(0, (now - since) / total));
    } else if (state.stage === "connecting") {
      status = t("reconnect.stage.connecting", { host: displayServerText(state.hop?.host ?? entry.label, 256) });
    } else {
      status = t("reconnect.stage.authenticating");
    }
  }
  // "Reconnect now" skips the backoff wait: while an attempt is already running there is nothing to hurry (the button
  // stays, disabled, so the layout does not jump), and after a fatal failure only a retry the backend allows is offered.
  const canRetry = state.phase === "reconnecting" || state.canRetry;
  const retryEnabled = state.phase === "reconnecting" ? state.stage === "waiting" : state.canRetry;

  return (
    <div className={`reconnect-overlay${fill ? " reconnect-overlay--fill" : ""}`}>
      {/* Not a dialog: focus stays where it is (the terminal underneath), so no dialog role. The title is an alert, which
          is announced once when the overlay appears; the countdown below it is not live. */}
      <div className="reconnect-card" role="group" aria-label={byUser ? t("reconnect.disconnectedTitle", { host }) : t("reconnect.title", { host })}>
        <div className="reconnect-card__head">
          <span className="reconnect-card__icon">
            <Icon name={byUser ? "plug" : "wifiOff"} size={18} />
          </span>
          <div className="reconnect-card__titles">
            <span className="reconnect-card__title" role="alert">{byUser ? t("reconnect.disconnectedTitle", { host }) : t("reconnect.title", { host })}</span>
            <span className="reconnect-card__sub mono">
              {showCause && cause ? <span>{errorMessage(cause)}</span> : null}
              {lostAt !== null && !byUser ? <span>{t("reconnect.lostAt", { time: formatDateTime(lostAt, { timeStyle: "medium" }) })}</span> : null}
            </span>
          </div>
        </div>
        {status !== null ? (
          <div className="reconnect-card__progress">
            <div className="reconnect-card__row">
              <span>{status}</span>
              <span className="reconnect-card__attempt">{attempt}</span>
            </div>
            <Progress value={progress} />
          </div>
        ) : null}
        {cause?.code === "hostKeyChanged" ? <HostKeyChangedHelp error={cause} /> : null}
        {cause?.code === "hostKeyMismatchOnReconnect" ? <HostKeyMismatchHelp error={cause} /> : null}
        {dirtyNames.length > 0 ? (
          <p className="reconnect-card__note">{t("reconnect.unsavedNote", { files: formatList(dirtyNames) })}</p>
        ) : null}
        <div className="reconnect-card__actions">
          <Button onClick={() => requestCloseSession(sessionId)}>{t("reconnect.closeTab")}</Button>
          {canRetry ? (
            <Button variant="primary" disabled={!retryEnabled} onClick={() => guard(reconnectNow(sessionId))}>
              {byUser ? t("reconnect.reconnect") : t("reconnect.now")}
            </Button>
          ) : null}
        </div>
      </div>
    </div>
  );
}
