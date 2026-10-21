import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { Dialog } from "../../components/Dialog";
import { Checkbox, Field, SecretInput } from "../../components/Field";
import { Icon } from "../../components/Icon";
import { ServerText } from "../../components/ServerText";
import type { Prompt } from "../../ipc/types";
import { formatEndpoint } from "../../lib/endpoint";
import {
  cancelAnswer,
  passphraseAnswer,
  passwordAnswer,
  skipAnswer,
  savedPasswordAnswer,
} from "../../lib/answers";
import { displayLocalPath } from "../../lib/localPath";
import { displayServerText } from "../../lib/serverText";
import { useStore } from "../../state/store";
import { PromptOrigin } from "./PromptOrigin";
import { promptInputGuardMs } from "./promptGuard";
import { answerPrompt } from "./sessionActions";

type SecretKind = Extract<Prompt, { kind: "passphrase" | "password" }>;

/**
 * Passphrase / password prompt (Password.dc, §5.6). The secret lives only in this component's state; the input
 * is cleared as soon as it is submitted and the component is remounted for every new prompt.
 */
export function SecretPrompt({ sessionId, prompt }: { sessionId: string; prompt: SecretKind }) {
  const { t } = useTranslation();
  const isPassphrase = prompt.kind === "passphrase";
  const [secret, setSecret] = useState("");
  const [addToAgent, setAddToAgent] = useState(prompt.kind === "passphrase" ? prompt.addToAgentDefault : false);
  const [remember, setRemember] = useState(false);
  const [busy, setBusy] = useState(false);

  const send = (answer: Parameters<typeof answerPrompt>[2]) => {
    if (busy) return; // ignore a late Escape/duplicate while an answer is in flight
    setBusy(true);
    setSecret("");
    void answerPrompt(sessionId, prompt.id, answer).then((ok) => {
      if (!ok) setBusy(false);
    });
  };

  const submit = (e?: FormEvent) => {
    e?.preventDefault();
    if (secret === "" || busy) return;
    if (prompt.kind === "passphrase") send(passphraseAnswer(secret, addToAgent && prompt.agentAvailable && prompt.addToAgentSupported));
    else send(passwordAnswer(secret, remember && prompt.canRemember));
  };

  const platform = useStore((s) => s.platform);
  const endpoint = formatEndpoint(prompt.hop.user, prompt.hop.host, prompt.hop.port);
  const attemptsLeft = prompt.attemptsLeft;
  const lastError = prompt.lastError;

  return (
    <Dialog
      title={isPassphrase ? t("prompt.passphrase.title") : t("prompt.password.title")}
      subtitle={
        isPassphrase ? (
          <>
            <bdi>{displayServerText(displayLocalPath(prompt.keyPath, platform), 256)}</bdi>
            <span className="arrow" aria-hidden="true" />
            <bdi>{endpoint}</bdi>
          </>
        ) : (
          <bdi>{endpoint}</bdi>
        )
      }
      icon={<Icon name="key" size={20} strokeWidth={1.8} />}
      inputGuardMs={promptInputGuardMs()}
      onClose={() => send(cancelAnswer())}
      footer={
        <>
          {prompt.kind === "password" && prompt.savedPasswordAvailable ? (
            <Button variant="ghost" onClick={() => send(savedPasswordAnswer())} disabled={busy}>
              {t("prompt.password.useSaved")}
            </Button>
          ) : null}
          {isPassphrase ? (
            <Button variant="ghost" onClick={() => send(skipAnswer())} disabled={busy}>
              {t("prompt.passphrase.skip")}
            </Button>
          ) : null}
          <span className="spacer" />
          <Button onClick={() => send(cancelAnswer())} disabled={busy}>
            {t("common.cancel")}
          </Button>
          <Button variant="primary" onClick={() => submit()} disabled={busy || secret === ""}>
            {isPassphrase ? t("prompt.passphrase.unlock") : t("connect.connect")}
          </Button>
        </>
      }
    >
      <PromptOrigin sessionId={sessionId} hop={prompt.hop} />
      <form onSubmit={submit} className="secret-form">
        <Field label={isPassphrase ? t("prompt.passphrase.label") : t("prompt.password.label")}>
          <SecretInput
            label={isPassphrase ? t("prompt.passphrase.label") : t("prompt.password.label")}
            data-autofocus
            autoComplete="off"
            spellCheck={false}
            value={secret}
            onChange={(e) => setSecret(e.target.value)}
            aria-invalid={lastError ? true : undefined}
          />
        </Field>
        {prompt.kind === "password" && prompt.serverLabel ? (
          <span className="dialog__hint">
            <ServerText text={prompt.serverLabel} />
          </span>
        ) : null}
        {lastError ? (
          <p className="dialog__error" role="alert">
            {t(`prompt.errors.${lastError}`, { count: attemptsLeft })}
          </p>
        ) : null}
        {prompt.kind === "passphrase" && prompt.agentAvailable ? (
          <Checkbox
            label={t("prompt.passphrase.addToAgent")}
            checked={addToAgent && prompt.addToAgentSupported}
            disabled={!prompt.addToAgentSupported}
            title={prompt.addToAgentSupported ? undefined : t("prompt.passphrase.addToAgentUnavailable")}
            onChange={setAddToAgent}
          />
        ) : null}
        {prompt.kind === "password" && prompt.canRemember ? (
          <Checkbox label={t("prompt.password.remember")} checked={remember} onChange={setRemember} />
        ) : null}
        <button type="submit" className="sr-only" tabIndex={-1} aria-hidden="true" />
      </form>
    </Dialog>
  );
}
