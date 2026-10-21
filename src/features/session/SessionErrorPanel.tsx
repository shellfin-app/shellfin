import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { Icon } from "../../components/Icon";
import { errorIcon } from "../../lib/errorIcon";
import { ServerText } from "../../components/ServerText";
import { StatePanel } from "../../components/StatePanel";
import { errorMessage } from "../../i18n/errorMessage";
import { formatList } from "../../i18n/format";
import type { AppError, AuthMethodKind } from "../../ipc/types";
import { COPIED } from "../../lib/notices";
import { guard } from "../../lib/reportError";
import { displayServerText } from "../../lib/serverText";
import { describeTarget } from "../../lib/targetInfo";
import { useStore } from "../../state/store";
import { editConnection, editConnectionNeedsCopy } from "./editConnection";
import { closeSession, reconnectNow } from "./sessionActions";

const MAX_HINT_LENGTH = 2048;

type HostKeyChanged = Extract<AppError, { code: "hostKeyChanged" }>;
type HostKeyMismatch = Extract<AppError, { code: "hostKeyMismatchOnReconnect" }>;

function CopyButton({ text }: { text: string }) {
  const { t } = useTranslation();
  return (
    <button
      type="button"
      className="icon-button"
      aria-label={t("common.copy")}
      title={t("common.copy")}
      onClick={() => guard(navigator.clipboard.writeText(text) /* the backend string, verbatim */)}
    >
      <Icon name="copy" size={13} />
    </button>
  );
}

/** File:line, fingerprint and the backend-built `ssh-keygen -R` hint (shared with the reconnect overlay). */
export function HostKeyChangedHelp({ error }: { error: HostKeyChanged }) {
  const { t } = useTranslation();
  const { keyType, fingerprint, knownHostsFile, line, removeHint, removeHintShell } = error.params;
  // The command is built by the backend (real lookup name, quoting for the local shell): shown
  // verbatim, never edited. `null` = it could not be built safely -> instruction only.
  // Anything this long is not a sane command: show the instruction only, offer no (truncated) copy.
  const hint = removeHint !== null && removeHint.length <= MAX_HINT_LENGTH ? removeHint : null;
  return (
    <div className="facts">
      <div className="facts__row">
        <span className="facts__label">{t("hostKey.expectedAt")}</span>
        <span className="facts__value mono">
          <bdi>{displayServerText(knownHostsFile, 256)}</bdi>
          {`:${line}`}
        </span>
      </div>
      <div className="facts__row">
        <span className="facts__label">{t("hostKey.type")}</span>
        <span className="facts__value mono">
          <bdi>{displayServerText(keyType, 64)}</bdi>
        </span>
      </div>
      <div className="facts__row">
        <span className="facts__label">{t("hostKey.presentedFingerprint")}</span>
        <span className="facts__value facts__value--strong mono">
          <bdi>{displayServerText(fingerprint, 128)}</bdi>
        </span>
      </div>
      <p className="dialog__hint">{t("hostKey.removeInstruction")}</p>
      {hint ? (
        <div className="facts__row">
          <span className="facts__label">{removeHintShell === "powershell" ? t("hostKey.runInPowershell") : t("hostKey.runInTerminal")}</span>
          <span className="facts__value mono">
            <code>{displayServerText(hint, MAX_HINT_LENGTH)}</code>
          </span>
          <CopyButton text={hint} />
        </div>
      ) : null}
    </div>
  );
}

/** Fix instructions for `hostKeyMismatchOnReconnect`: the key the session was opened with vs the one offered now. */
export function HostKeyMismatchHelp({ error }: { error: HostKeyMismatch }) {
  const { t } = useTranslation();
  const { expected, actual } = error.params;
  return (
    <div className="facts">
      <div className="facts__row">
        <span className="facts__label">{t("hostKey.expectedFingerprint")}</span>
        <span className="facts__value mono">
          <bdi>{displayServerText(expected, 128)}</bdi>
        </span>
      </div>
      <div className="facts__row">
        <span className="facts__label">{t("hostKey.presentedFingerprint")}</span>
        <span className="facts__value facts__value--strong mono">
          <bdi>{displayServerText(actual, 128)}</bdi>
        </span>
      </div>
      <p className="dialog__hint">{t("hostKey.mismatchInstruction")}</p>
    </div>
  );
}

/** One-line remedy for the errors users hit most. `null` = nothing useful to add. */
export function errorFix(error: AppError, t: TFunction): string | null {
  switch (error.code) {
    case "dnsFailed":
      return t("fix.dnsFailed");
    case "connectRefused":
      return t("fix.connectRefused", { port: error.params.port });
    case "connectTimeout":
      return t("fix.connectTimeout");
    case "networkUnreachable":
      return t("fix.networkUnreachable");
    case "tooManyAuthFailures":
      return t("fix.tooManyAuthFailures");
    case "authFailed":
      return t("fix.authFailed");
    case "agentNoIdentities":
      return t("fix.agentNoIdentities");
    case "agentUnavailable":
      return t("fix.agentUnavailable");
    case "keyLoadFailed":
    case "passphraseIncorrect":
      return t("fix.keyProblem");
    case "hostKeyRejected":
      return t("fix.hostKeyRejected");
    case "jumpChannelFailed":
      return t("fix.jumpChannelFailed");
    case "kexFailed":
      return t("fix.kexFailed");
    default:
      return null;
  }
}

/**
 * Display list of auth methods. The backend sends `password` AND `keyboardInteractive` in `tried` when the user's
 * password went through a keyboard-interactive "Password:" prompt: then only "Password" is shown.
 */
export function methodList(methods: readonly AuthMethodKind[], t: TFunction, tried = false): string {
  const hideKbd = tried && methods.includes("password");
  const named = methods
    .filter((m) => m !== "none" && !(hideKbd && m === "keyboardInteractive"))
    .map((m) => t(`authMethod.${m}`));
  return formatList(named);
}

/** Failure before the first successful connection (§5.8 `disconnected`): why, what to do, retry/edit/close. */
export function SessionErrorPanel({ sessionId }: { sessionId: string }) {
  const { t } = useTranslation();
  const entry = useStore((s) => s.sessionsById[sessionId]);
  const bookmarks = useStore((s) => s.bookmarks?.bookmarks);
  const sshHosts = useStore((s) => s.sshConfig?.hosts);
  const pushInfo = useStore((s) => s.pushInfo);
  if (!entry || entry.state.phase !== "disconnected") return null;
  const { error, canRetry } = entry.state;
  const host = displayServerText(entry.label, 256);
  const info = describeTarget(entry.target, bookmarks ?? [], sshHosts ?? []);
  const fix = error ? errorFix(error, t) : null;
  const needsCopy = editConnectionNeedsCopy(sessionId);

  const copyDetails = () => {
    const lines = [t("session.connectFailed", { host: entry.label })];
    if (info.endpoint) lines.push(info.endpoint);
    if (error) {
      lines.push(errorMessage(error), `code: ${error.code}`);
      if (error.code === "authFailed") {
        lines.push(`tried: ${formatList(error.params.tried)}`, `server methods: ${formatList(error.params.serverMethods)}`);
      }
      if (error.detail) lines.push(error.detail);
    }
    guard(navigator.clipboard.writeText(lines.join("\n")).then(() => pushInfo(COPIED)));
  };

  return (
    <StatePanel
      icon={errorIcon(error)}
      tone="danger"
      role="alert"
      title={t("session.connectFailed", { host })}
      subtitle={info.endpoint !== null ? <bdi>{displayServerText(info.endpoint, 256)}</bdi> : undefined}
      actions={
        <>
          <Button onClick={() => guard(closeSession(sessionId))}>{t("reconnect.closeTab")}</Button>
          <Button icon="copy" onClick={copyDetails}>
            {t("session.copyDetails")}
          </Button>
          <Button icon="edit" onClick={() => editConnection(sessionId)}>
            {needsCopy ? t("session.editCopy") : t("session.edit")}
          </Button>
          {canRetry || error?.code === "hostKeyChanged" ? (
            <Button variant="primary" icon="refresh" onClick={() => guard(reconnectNow(sessionId))}>
              {t("session.retry")}
            </Button>
          ) : null}
        </>
      }
    >
      {error ? <p className="state-panel__body">{errorMessage(error)}</p> : null}
      {error?.code === "authFailed" ? (
        <div className="facts">
          <div className="facts__row">
            <span className="facts__label">{t("session.tried")}</span>
            <span className="facts__value">{methodList(error.params.tried, t, true) || t("session.triedNothing")}</span>
          </div>
          <div className="facts__row">
            <span className="facts__label">{t("session.serverAccepts")}</span>
            <span className="facts__value">{methodList(error.params.serverMethods, t) || t("session.serverAcceptsUnknown")}</span>
          </div>
        </div>
      ) : null}
      {fix ? <p className="state-panel__hint">{fix}</p> : null}
      {error?.code === "hostKeyChanged" ? <HostKeyChangedHelp error={error} /> : null}
      {error?.code === "hostKeyMismatchOnReconnect" ? <HostKeyMismatchHelp error={error} /> : null}
      {error?.detail ? (
        <details className="session-error__details">
          <summary>{t("session.details")}</summary>
          <pre className="mono">
            <ServerText text={error.detail} limit={2000} keepNewlines />
          </pre>
        </details>
      ) : null}
    </StatePanel>
  );
}
