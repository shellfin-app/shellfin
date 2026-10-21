import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { Dialog } from "../../components/Dialog";
import { Icon } from "../../components/Icon";
import { FieldGroup } from "../../components/Field";
import { DIALOG_KIND } from "../../lib/dialogs";
import { ENCODINGS, encodingLabel } from "../../lib/languages";
import { basename } from "../../lib/remotePath";
import { guard } from "../../lib/reportError";
import { displayName, displayPath } from "../../lib/serverText";
import { useStore } from "../../state/store";
import { reopenWithEncoding, setSaveEncoding } from "./editorActions";

/**
 * Encoding choice for a buffer (§6.3/§6.5): "Reopen with encoding" re-reads the file and decodes it differently
 * (also the way out of a `decodeFailed` tab); "Save with encoding" keeps the text and re-encodes it on the next save.
 */
export function EncodingPicker({ bufferId }: { bufferId: string }) {
  const { t } = useTranslation();
  const meta = useStore((s) => s.buffers[bufferId]);
  const closeDialog = useStore((s) => s.closeDialog);
  const [choice, setChoice] = useState<string>(() => (meta && meta.load === "ready" ? meta.encoding : "windows-1251"));
  if (!meta) return null;
  const ready = meta.load === "ready";
  const close = () => closeDialog(DIALOG_KIND.encoding);

  return (
    <Dialog
      title={t("encoding.title", { name: displayName(basename(meta.path)) })}
      subtitle={<bdi>{displayPath(meta.path)}</bdi>}
      width={460}
      icon={<Icon name="file" size={20} strokeWidth={1.8} />}
      onClose={close}
      footer={
        <>
          <Button onClick={close}>{t("common.cancel")}</Button>
          {ready ? (
            <Button
              onClick={() => {
                setSaveEncoding(bufferId, choice);
                close();
              }}
              disabled={choice === meta.encoding}
            >
              {t("encoding.saveWith")}
            </Button>
          ) : null}
          <Button
            variant="primary"
            data-autofocus
            onClick={() => {
              close();
              guard(reopenWithEncoding(bufferId, choice));
            }}
          >
            {t("encoding.reopenWith")}
          </Button>
        </>
      }
    >
      <FieldGroup label={t("encoding.label")}>
        <div className="radio-list" role="radiogroup" aria-label={t("encoding.label")}>
          {ENCODINGS.map((enc) => (
            <label key={enc} className="check">
              <input type="radio" name="encoding" value={enc} checked={choice === enc} onChange={() => setChoice(enc)} />
              <span className="mono">{encodingLabel(enc)}</span>
            </label>
          ))}
        </div>
      </FieldGroup>
      {ready && meta.dirty ? <p className="dialog__warn">{t("encoding.discardWarning")}</p> : null}
      <p className="dialog__hint">{t("encoding.hint")}</p>
    </Dialog>
  );
}
