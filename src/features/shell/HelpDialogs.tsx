import { useTranslation } from "react-i18next";
import { BrandMark } from "../../components/BrandMark";
import { Button } from "../../components/Button";
import { Dialog } from "../../components/Dialog";
import { t as translate } from "../../i18n/translate";
import { A, CHEAT_SHEET, shortcutLabel } from "../../lib/actionIds";
import { nativeMenuTitle } from "../../lib/menuMnemonic";
import { useStore } from "../../state/store";
import { actionLabel, runAction } from "../actions/registry";
import "./helpDialogs.css";

/** Help > Keyboard shortcuts: every action with a shortcut, grouped like the menus. */
export function ShortcutsDialog() {
  const { t } = useTranslation();
  const close = useStore((s) => s.closeDialog);
  const platform = useStore((s) => s.platform);
  return (
    <Dialog
      title={t("shortcuts.title")}
      width={640}
      onClose={() => close("shortcuts")}
      footer={
        <Button variant="primary" onClick={() => close("shortcuts")} data-autofocus>
          {t("common.close")}
        </Button>
      }
    >
      <p className="dialog__text">{t("shortcuts.intro")}</p>
      <div className="shortcuts">
        {CHEAT_SHEET.map((section) => {
          const rows = section.actions.flatMap((id) => {
            const keys = shortcutLabel(id, platform);
            return keys ? [{ id, keys }] : [];
          });
          if (rows.length === 0) return null;
          // A heading, not a menu: no mnemonic marker and no CJK "(F)" group (lib/menuMnemonic.ts).
          const title = section.id.startsWith("menu:") ? nativeMenuTitle(translate(`menubar.${section.id.slice(5)}`)) : t("shortcuts.focus");
          return (
            <section key={section.id} className="shortcuts__section" aria-label={title}>
              <h3 className="sec">{title}</h3>
              <dl className="shortcuts__list">
                {rows.map((row) => (
                  <div key={row.id} className="shortcuts__row">
                    <dt>{actionLabel(row.id)}</dt>
                    <dd>
                      <kbd className="kbd mono">{row.keys}</kbd>
                    </dd>
                  </div>
                ))}
              </dl>
            </section>
          );
        })}
      </div>
    </Dialog>
  );
}

/** Help > About Shellfin. */
export function AboutDialog() {
  const { t } = useTranslation();
  const close = useStore((s) => s.closeDialog);
  const version = useStore((s) => s.appVersion);
  return (
    <Dialog
      title={t("about.title")}
      width={440}
      onClose={() => close("about")}
      footer={
        <>
          <Button onClick={() => runAction(A.openLogs)}>{t("about.logs")}</Button>
          <Button onClick={() => runAction(A.documentation)}>{t("about.website")}</Button>
          <Button variant="primary" onClick={() => close("about")} data-autofocus>
            {t("common.close")}
          </Button>
        </>
      }
    >
      <div className="about">
        <BrandMark size={56} />
        <div className="about__text">
          <p className="about__name">{t("app.name")}</p>
          <p className="about__version mono">{t("about.version", { version })}</p>
          <p className="dialog__text">{t("about.tagline")}</p>
          <p className="dialog__text about__license">{t("about.license")}</p>
        </div>
      </div>
    </Dialog>
  );
}
