import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { Icon } from "../../components/Icon";
import { StatePanel } from "../../components/StatePanel";
import { CONNECT_STEPS, currentStep, type ConnectStep } from "../../lib/connectSteps";
import { guard } from "../../lib/reportError";
import { displayServerText } from "../../lib/serverText";
import { basename } from "../../lib/remotePath";
import { describeTarget } from "../../lib/targetInfo";
import { useStore } from "../../state/store";
import { closeSession } from "./sessionActions";
import { sessionTitle } from "./sessionTitle";

/** Re-renders once a second while mounted; returns the current time. */
function useTick(): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, []);
  return now;
}

/**
 * First connection in progress: target, jump host, the steps with the running one marked,
 * elapsed time and Cancel (Esc).
 */
export function ConnectingCard({ sessionId }: { sessionId: string }) {
  const { t } = useTranslation();
  const entry = useStore((s) => s.sessionsById[sessionId]);
  const prompt = useStore((s) => s.prompts[sessionId]);
  const bookmarks = useStore((s) => s.bookmarks?.bookmarks);
  const sshHosts = useStore((s) => s.sshConfig?.hosts);
  const now = useTick();

  // Esc cancels while nothing modal is above the card (prompts and dialogs handle their own Escape first).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      const st = useStore.getState();
      if (st.dialog !== null || st.palette !== null || st.prompts[sessionId] !== undefined) return;
      if (st.activeSessionId !== sessionId) return;
      guard(closeSession(sessionId));
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [sessionId]);

  if (!entry) return null;
  const info = describeTarget(entry.target, bookmarks ?? [], sshHosts ?? []);
  const hop = entry.state.phase === "connecting" || entry.state.phase === "authenticating" || entry.state.phase === "openingChannels" ? entry.state.hop : null;
  const current = currentStep(entry.state.phase, prompt?.kind);
  const currentIndex = CONNECT_STEPS.indexOf(current);
  const title = sessionTitle(entry, t); // the same name as the tab (rename, " (n)")
  const seconds = Math.max(0, Math.floor((now - entry.startedAtMs) / 1000));
  const bm = entry.target.kind === "bookmark" ? bookmarks?.find((b) => b.id === (entry.target as { id: string }).id) : undefined;
  const keyName = bm?.auth.method === "key" ? basename(bm.auth.identityFile.replace(/\\/g, "/")) : null;
  const waiting = prompt !== undefined;

  const stepText = (step: ConnectStep): string => {
    const host = displayServerText(hop?.label ?? info.host ?? entry.label, 256);
    switch (step) {
      case "resolve":
        return t("connecting.step.resolve");
      case "connect":
        return info.via !== null || (hop !== null && hop.index > 0)
          ? t("connecting.step.connectHop", { host, hop: (hop?.index ?? 0) + 1 })
          : t("connecting.step.connect", { host });
      case "hostKey":
        return t("connecting.step.hostKey");
      case "auth":
        return keyName !== null ? t("connecting.step.authKey", { key: displayServerText(keyName, 128) }) : t("connecting.step.auth");
      case "open":
        return t("connecting.step.open");
    }
  };

  return (
    <StatePanel
      icon="loader"
      tone="busy"
      role="status"
      title={title}
      subtitle={
        info.endpoint !== null ? (
          <>
            <bdi>{displayServerText(info.endpoint, 256)}</bdi>
            {info.via !== null ? ` · ${t("hosts.via", { host: displayServerText(info.via, 128) })}` : null}
          </>
        ) : undefined
      }
      actions={
        <Button onClick={() => guard(closeSession(sessionId))} data-testid="connecting-cancel">
          {t("connecting.cancel")}
        </Button>
      }
    >
      <ol className="connecting-steps" aria-label={t("connecting.steps")}>
        {CONNECT_STEPS.map((step, i) => {
          const state = i < currentIndex ? "done" : i === currentIndex ? "current" : "todo";
          return (
            <li key={step} className={`connecting-steps__item connecting-steps__item--${state}`} aria-current={state === "current" ? "step" : undefined}>
              <span className="connecting-steps__mark" aria-hidden="true">
                {state === "done" ? <Icon name="check" size={13} strokeWidth={2.4} /> : state === "current" ? <Icon name="loader" size={13} className="spin" /> : null}
              </span>
              <span>{stepText(step)}</span>
              {state === "current" && waiting ? <span className="connecting-steps__wait">{t("session.needsAnswer")}</span> : null}
            </li>
          );
        })}
      </ol>
      <p className="state-panel__hint">
        {t("connecting.elapsed", { seconds })}
      </p>
    </StatePanel>
  );
}
