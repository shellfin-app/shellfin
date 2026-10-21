import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { Dialog } from "../../components/Dialog";
import { Checkbox } from "../../components/Field";
import { Icon } from "../../components/Icon";
import type { Prompt } from "../../ipc/types";
import { hostKeyAnswer } from "../../lib/answers";
import { guard } from "../../lib/reportError";
import { displayServerText } from "../../lib/serverText";
import { serverFingerprintCommand } from "../../lib/shellquote";
import { PromptOrigin } from "./PromptOrigin";
import { promptInputGuardMs } from "./promptGuard";
import { answerPrompt } from "./sessionActions";

type HostKeyPrompt = Extract<Prompt, { kind: "hostKeyUnknown" }>;

/** Unknown host key (HostKey.dc, §5.3): explicit decision, fingerprint to compare, optional "remember". */
export function HostKeyDialog({ sessionId, prompt }: { sessionId: string; prompt: HostKeyPrompt }) {
  const { t } = useTranslation();
  const [remember, setRemember] = useState(prompt.canRemember);
  const [busy, setBusy] = useState(false);
  const file = prompt.knownHostsFile ?? "known_hosts";

  const answer = (accept: boolean) => {
    if (busy) return; // a late Escape must not race the real answer
    setBusy(true);
    void answerPrompt(sessionId, prompt.id, hostKeyAnswer(accept, accept && prompt.canRemember && remember)).then((ok) => {
      if (!ok) setBusy(false);
    });
  };

  return (
    <Dialog
      title={t("hostKey.title", { host: displayServerText(prompt.hop.host, 256) })}
      tone="warn"
      icon={<Icon name="shield" size={20} strokeWidth={1.8} />}
      role="alertdialog"
      inputGuardMs={promptInputGuardMs()}
      onClose={() => answer(false)}
      footer={
        <>
          <Button onClick={() => answer(false)} disabled={busy} data-autofocus>
            {t("common.cancel")}
          </Button>
          <Button variant="primary" onClick={() => answer(true)} disabled={busy}>
            {t("hostKey.trust")}
          </Button>
        </>
      }
    >
      <PromptOrigin sessionId={sessionId} hop={prompt.hop} />
      <p className="dialog__text">
        {t("hostKey.body", { file: displayServerText(file, 256), command: serverFingerprintCommand(prompt.keyType) })}
      </p>
      <div className="facts">
        <div className="facts__row">
          <span className="facts__label">{t("hostKey.host")}</span>
          <span className="facts__value mono">
            <bdi>{displayServerText(prompt.knownHostsName, 256)}</bdi>
          </span>
        </div>
        <div className="facts__row">
          <span className="facts__label">{t("hostKey.type")}</span>
          <span className="facts__value mono">
            <bdi>{displayServerText(prompt.keyType, 64)}</bdi>
          </span>
        </div>
        <div className="facts__row">
          <span className="facts__label">{t("hostKey.fingerprint")}</span>
          <span className="facts__value facts__value--strong mono">
            <bdi>{displayServerText(prompt.fingerprint, 128)}</bdi>
          </span>
          <button
            type="button"
            className="icon-button"
            aria-label={t("hostKey.copyFingerprint")}
            title={t("hostKey.copyFingerprint")}
            onClick={() => guard(navigator.clipboard.writeText(prompt.fingerprint))}
          >
            <Icon name="copy" size={13} />
          </button>
        </div>
      </div>
      {prompt.otherKeyTypesKnown.length > 0 ? (
        <p className="dialog__warn" role="note">
          {t("hostKey.otherTypesKnown", { types: prompt.otherKeyTypesKnown.map((k) => displayServerText(k, 64)).join(", ") })}
        </p>
      ) : null}
      {prompt.canRemember ? (
        <Checkbox label={t("hostKey.remember", { file: displayServerText(file, 256) })} checked={remember} onChange={setRemember} />
      ) : (
        <p className="dialog__hint">{t("hostKey.cannotRemember")}</p>
      )}
    </Dialog>
  );
}
