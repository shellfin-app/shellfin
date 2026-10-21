import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { Dialog } from "../../components/Dialog";
import { errorMessage } from "../../i18n/errorMessage";
import { getBackend } from "../../ipc";
import type { AgentKey, AppError, LocalKeyInfo } from "../../ipc/types";
import { COPIED } from "../../lib/notices";
import { guard, reportError } from "../../lib/reportError";
import { REVEAL_KNOWN_HOSTS, REVEAL_SSH_CONFIG } from "../../lib/reveal";
import { useStore } from "../../state/store";
import "./keys.css";

export interface KeyRow {
  id: string;
  title: string;
  path: string | null;
  keyType: string | null;
  fingerprint: string | null;
  encrypted: boolean;
  inAgent: boolean;
  error?: AppError;
}

/** One row per key: a file key that is also loaded in the agent is merged by fingerprint. */
export function mergeKeys(local: readonly LocalKeyInfo[], agentKeys: readonly AgentKey[]): KeyRow[] {
  const rows: KeyRow[] = local.map((k) => ({
    id: k.path,
    title: k.path,
    path: k.path,
    keyType: k.keyType,
    fingerprint: k.fingerprint,
    encrypted: k.encrypted,
    inAgent: k.inAgent,
    ...(k.error ? { error: k.error } : {}),
  }));
  const known = new Set(rows.map((r) => r.fingerprint).filter((f): f is string => f !== null));
  for (const a of agentKeys) {
    if (known.has(a.fingerprint)) continue;
    rows.push({ id: `agent:${a.fingerprint}`, title: a.comment || a.fingerprint, path: null, keyType: a.keyType, fingerprint: a.fingerprint, encrypted: false, inAgent: true });
  }
  return rows;
}

/** Keys view: local private keys (`keys_list`) and the ssh-agent identities (`agent_status`). */
export function KeysDialog() {
  const { t } = useTranslation();
  const closeStoreDialog = useStore((s) => s.closeDialog);
  const closeDialog = () => closeStoreDialog("keys");
  const agent = useStore((s) => s.agent);
  const setAgent = useStore((s) => s.setAgent);
  const pushInfo = useStore((s) => s.pushInfo);
  const [keys, setKeys] = useState<LocalKeyInfo[] | null>(null);
  const [reloadTick, setReloadTick] = useState(0);

  useEffect(() => {
    let cancelled = false;
    const backend = getBackend();
    Promise.all([backend.keysList(), backend.agentStatus()])
      .then(([list, status]) => {
        if (cancelled) return;
        setKeys(list);
        setAgent(status);
      })
      .catch(reportError);
    return () => {
      cancelled = true;
    };
  }, [setAgent, reloadTick]);

  const rows = mergeKeys(keys ?? [], agent?.available ? agent.identities : []);
  const copy = (text: string) => guard(navigator.clipboard.writeText(text).then(() => pushInfo(COPIED)));
  // The one-line public key comes from the backend (`keys_read_public`, never decrypts); the path is exactly a listed one.
  const copyPublicKey = (path: string) => guard(getBackend().keysReadPublic(path).then(copy));

  return (
    <Dialog
      title={t("keys.title")}
      width={680}
      onClose={closeDialog}
      footer={
        <>
          <Button variant="ghost" icon="refresh" onClick={() => setReloadTick((n) => n + 1)}>
            {t("keys.refresh")}
          </Button>
          <Button variant="ghost" onClick={() => guard(getBackend().appRevealPath(REVEAL_SSH_CONFIG))}>
            {t("keys.revealConfig")}
          </Button>
          <Button variant="ghost" onClick={() => guard(getBackend().appRevealPath(REVEAL_KNOWN_HOSTS))}>
            {t("keys.revealKnownHosts")}
          </Button>
          <span className="spacer" />
          <Button onClick={closeDialog}>{t("common.close")}</Button>
        </>
      }
    >
      <section className="keys-section" aria-labelledby="keys-local">
        <h3 className="sec" id="keys-local">
          {t("keys.title")}
        </h3>
        {keys === null ? <p className="dialog__hint">{t("common.loading")}</p> : null}
        {keys !== null && rows.length === 0 ? <p className="dialog__hint">{t("keys.noLocal")}</p> : null}
        <ul className="keys-list">
          {rows.map((k) => (
            <li key={k.id} className="keys-item">
              <bdi className="mono keys-item__path">{k.title}</bdi>
              <span className="keys-item__meta mono">
                {k.keyType ? <bdi>{k.keyType}</bdi> : null}
                {k.fingerprint ? <bdi>{k.fingerprint}</bdi> : null}
              </span>
              <span className="chips">
                {k.path !== null ? <span>{t("keys.badge.file")}</span> : null}
                {k.inAgent ? <span>{t("keys.badge.agent")}</span> : null}
                {k.encrypted ? <span>{t("keys.badge.passphrase")}</span> : null}
              </span>
              {k.error ? <span className="dialog__error">{errorMessage(k.error)}</span> : null}
              <span className="keys-item__actions">
                {k.fingerprint ? (
                  <Button variant="ghost" icon="copy" onClick={() => copy(k.fingerprint ?? "")}>
                    {t("keys.copyFingerprint")}
                  </Button>
                ) : null}
                {k.path !== null ? (
                  <Button variant="ghost" icon="copy" onClick={() => copyPublicKey(k.path ?? "")}>
                    {t("keys.copyPublicKey")}
                  </Button>
                ) : null}
              </span>
            </li>
          ))}
        </ul>
        {agent?.warning ? <p className="dialog__warn">{errorMessage(agent.warning)}</p> : null}
        {agent && !agent.available ? <p className="dialog__hint">{t("welcome.agentUnavailable")}</p> : null}
      </section>
    </Dialog>
  );
}
