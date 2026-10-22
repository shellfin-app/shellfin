import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { Dialog } from "../../components/Dialog";
import { Icon } from "../../components/Icon";
import type { ConfigWarning, SshConfigSnapshot } from "../../ipc/types";
import { DIALOG_KIND } from "../../lib/dialogs";
import { displayPath, displayServerText } from "../../lib/serverText";
import { useStore } from "../../state/store";
import "./welcome.css";

/** Rows rendered at most; the rest is folded into "and N more" (a hostile config can produce thousands). */
export const MAX_SHOWN_CONFIG_WARNINGS = 500;

/** Every load warning of the snapshot, including the `badLine` warnings the per-file cap left out (§7.4). */
export function configWarningCount(config: SshConfigSnapshot | null): number {
  return config ? config.warnings.length + config.droppedWarnings : 0;
}

/**
 * `configWarnings.<code>` with its params. Every param comes from the config (paths, keywords, patterns, aliases):
 * paths (`file`, Include `pattern`) through `displayPath`, the rest through `displayServerText` (§9.5). A `badLine` at
 * line 0 is a whole file skipped as key material or binary data (§7.4), not a line.
 */
export function configWarningMessage(t: TFunction, warning: ConfigWarning): string {
  if (warning.code === "badLine" && warning.location.line === 0) return t("configWarningsDialog.fileSkipped");
  // The key is built from the code, so the typed `t` cannot check its params (like `errorMessage`).
  const tr = t as unknown as (key: string, options: Record<string, string>) => string;
  const params: Record<string, string> = {};
  for (const [k, v] of Object.entries(warning.params as Record<string, string>)) {
    params[k] = k === "file" || k === "pattern" ? displayPath(v) : displayServerText(v, 256);
  }
  return tr(`configWarnings.${warning.code}`, params);
}

/** Lists the ssh config load warnings (Welcome > Config > "N warnings"). Reads the live snapshot. */
export function ConfigWarningsDialog() {
  const { t } = useTranslation();
  const closeDialog = useStore((s) => s.closeDialog);
  const config = useStore((s) => s.sshConfig);
  const close = () => closeDialog(DIALOG_KIND.configWarnings);
  const warnings = config?.warnings ?? [];
  const shown = warnings.slice(0, MAX_SHOWN_CONFIG_WARNINGS);
  const more = warnings.length - shown.length + (config?.droppedWarnings ?? 0);
  return (
    <Dialog
      title={t("configWarningsDialog.title")}
      tone="warn"
      width={620}
      icon={<Icon name="alert" size={20} strokeWidth={1.8} />}
      onClose={close}
      footer={
        <>
          <span className="spacer" />
          <Button variant="primary" onClick={close} data-autofocus>
            {t("common.close")}
          </Button>
        </>
      }
    >
      {shown.length === 0 && more === 0 ? <p className="dialog__text">{t("configWarningsDialog.none")}</p> : null}
      {shown.length > 0 ? (
        // focusable so a keyboard user can scroll a long list
        <ul className="config-warnings" aria-label={t("configWarningsDialog.listLabel")} tabIndex={0}>
          {shown.map((w, i) => (
            <li key={`${w.location.file}:${w.location.line}:${i}`}>
              {/* A file-level warning (line 0) that names its file already says where it is. */}
              {w.location.line === 0 && "file" in w.params && w.params.file === w.location.file ? null : (
                <span className="config-warnings__loc mono">
                  {w.location.line > 0
                    ? t("configWarningsDialog.location", { file: displayPath(w.location.file), line: w.location.line })
                    : displayPath(w.location.file)}
                </span>
              )}
              <span className="config-warnings__text">{configWarningMessage(t, w)}</span>
            </li>
          ))}
        </ul>
      ) : null}
      {more > 0 ? <p className="dialog__hint">{t("configWarningsDialog.more", { count: more })}</p> : null}
    </Dialog>
  );
}
