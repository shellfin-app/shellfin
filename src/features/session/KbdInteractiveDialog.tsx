import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { Dialog } from "../../components/Dialog";
import { SecretInput } from "../../components/Field";
import { Icon } from "../../components/Icon";
import type { Prompt } from "../../ipc/types";
import { formatEndpoint } from "../../lib/endpoint";
import { cancelAnswer, kbdAnswer } from "../../lib/answers";
import { displayServerText } from "../../lib/serverText";
import { PromptOrigin } from "./PromptOrigin";
import { promptInputGuardMs } from "./promptGuard";
import { answerPrompt } from "./sessionActions";

type KbdPrompt = Extract<Prompt, { kind: "keyboardInteractive" }>;

/** Keyboard-interactive round (2FA etc.): every label/instruction is SERVER text, shown sanitised (§9.5). */
export function KbdInteractiveDialog({ sessionId, prompt }: { sessionId: string; prompt: KbdPrompt }) {
  const { t } = useTranslation();
  const [values, setValues] = useState<string[]>(() => prompt.fields.map(() => ""));
  const [busy, setBusy] = useState(false);

  const send = (answer: Parameters<typeof answerPrompt>[2]) => {
    if (busy) return;
    setBusy(true);
    setValues(prompt.fields.map(() => ""));
    void answerPrompt(sessionId, prompt.id, answer).then((ok) => {
      if (!ok) setBusy(false);
    });
  };
  const submit = (e?: FormEvent) => {
    e?.preventDefault();
    if (!busy) send(kbdAnswer(values));
  };

  const name = displayServerText(prompt.name, 256);
  const instructions = displayServerText(prompt.instructions, 256, true);

  return (
    <Dialog
      title={name === "" ? t("prompt.kbd.title") : t("prompt.kbd.titleFrom", { host: displayServerText(prompt.hop.host, 256), name })}
      subtitle={<bdi>{formatEndpoint(prompt.hop.user, prompt.hop.host, prompt.hop.port)}</bdi>}
      icon={<Icon name="key" size={20} strokeWidth={1.8} />}
      inputGuardMs={promptInputGuardMs()}
      onClose={() => send(cancelAnswer())}
      footer={
        <>
          <Button onClick={() => send(cancelAnswer())} disabled={busy}>
            {t("common.cancel")}
          </Button>
          <Button variant="primary" onClick={() => submit()} disabled={busy}>
            {t("prompt.kbd.submit")}
          </Button>
        </>
      }
    >
      <PromptOrigin sessionId={sessionId} hop={prompt.hop} />
      <form onSubmit={submit} className="secret-form">
        {instructions !== "" ? <p className="dialog__text kbd-instructions">{instructions}</p> : null}
        {prompt.fields.map((field, i) => {
          const label = displayServerText(field.label, 256);
          return (
            <label key={i} className="field">
              <span className="field__label">{label}</span>
              {field.echo ? (
                <input
                  className="input"
                  data-autofocus={i === 0 ? true : undefined}
                  type="text"
                  autoComplete="off"
                  spellCheck={false}
                  value={values[i] ?? ""}
                  onChange={(e) => setValues((v) => v.map((x, j) => (j === i ? e.target.value : x)))}
                />
              ) : (
                <SecretInput
                  label={label}
                  data-autofocus={i === 0 ? true : undefined}
                  autoComplete="off"
                  spellCheck={false}
                  value={values[i] ?? ""}
                  onChange={(e) => setValues((v) => v.map((x, j) => (j === i ? e.target.value : x)))}
                />
              )}
            </label>
          );
        })}
        <button type="submit" className="sr-only" tabIndex={-1} aria-hidden="true" />
      </form>
    </Dialog>
  );
}
