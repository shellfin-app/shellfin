// Settings > Appearance: theme, accent, interface text size, reduce motion. The dialog owns the form
// (`SettingsFormState.look`); every change is previewed live on <html> and undone when the dialog closes unsaved.
import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { Select, SegmentedControl, Setting } from "../../components/Field";
import { InfoTip } from "../../components/InfoTip";
import type { AccentColor, AppearanceSettings, ReduceMotion, ThemeMode } from "../../ipc/types";
import type { AppearanceForm } from "../../lib/settingsFormAppearance";
import { ACCENTS, ACCENT_VALUES, UI_FONT_SIZE, resolveTheme } from "../../lib/theme";
import { endAppearancePreview, previewAppearance, systemPrefersDark } from "./appearance";

export interface AppearanceProps {
  look: AppearanceForm;
  patch: (p: Partial<AppearanceForm>) => void;
}

/**
 * Applies the unsaved appearance while the settings dialog is open. Call it once from the dialog (not from a section,
 * which unmounts when another section is shown). Closing the dialog, saved or not, restores the saved settings.
 */
export function useAppearancePreview(look: AppearanceForm | null): void {
  const appearance = look?.appearance;
  const background = look?.terminalBackground;
  useEffect(() => {
    if (!appearance || background === undefined) return undefined;
    previewAppearance(appearance, background);
    return () => endAppearancePreview();
  }, [appearance, background]);
}

const FONT_SIZES = Array.from({ length: UI_FONT_SIZE.max - UI_FONT_SIZE.min + 1 }, (_, i) => UI_FONT_SIZE.min + i);

export function AppearanceSection({ look, patch }: AppearanceProps) {
  const { t } = useTranslation();
  const a = look.appearance;
  const set = (p: Partial<AppearanceSettings>) => patch({ appearance: { ...a, ...p } });
  const shown = resolveTheme(a.theme, systemPrefersDark());
  const accentLabel: Record<AccentColor, string> = {
    teal: t("settings.appearance.accentTeal"),
    blue: t("settings.appearance.accentBlue"),
    purple: t("settings.appearance.accentPurple"),
    orange: t("settings.appearance.accentOrange"),
    green: t("settings.appearance.accentGreen"),
    pink: t("settings.appearance.accentPink"),
  };
  return (
    <fieldset className="settings-group">
      <legend>{t("settings.appearance.title")}</legend>
      <SegmentedControl<ThemeMode>
        label={t("settings.appearance.theme")}
        value={a.theme}
        onChange={(theme) => set({ theme })}
        options={[
          { value: "dark", label: t("settings.appearance.themeDark") },
          { value: "light", label: t("settings.appearance.themeLight") },
          { value: "system", label: t("settings.appearance.themeSystem") },
        ]}
      />

      <div className="field" role="radiogroup" aria-label={t("settings.appearance.accent")}>
        <span className="setting__label">
          <span className="field__label">{t("settings.appearance.accent")}</span>
          <InfoTip setting={t("settings.appearance.accent")} text={t("settings.appearance.accentInfo")} />
        </span>
        <div className="accent-picker">
          {ACCENTS.map((accent) => (
            <button
              key={accent}
              type="button"
              role="radio"
              aria-checked={a.accent === accent}
              aria-label={accentLabel[accent]}
              title={accentLabel[accent]}
              className={`accent-picker__swatch${a.accent === accent ? " accent-picker__swatch--active" : ""}`}
              style={{ background: ACCENT_VALUES[accent][shown] }}
              onClick={() => set({ accent })}
            />
          ))}
        </div>
      </div>

      <Setting label={t("settings.appearance.uiFontSize")} info={t("settings.appearance.uiFontSizeInfo")}>
        <Select value={String(a.uiFontSize)} onChange={(e) => set({ uiFontSize: Number(e.target.value) })}>
          {FONT_SIZES.map((size) => (
            <option key={size} value={size}>
              {t("settings.appearance.uiFontSizeValue", { size })}
            </option>
          ))}
        </Select>
      </Setting>

      <Setting label={t("settings.appearance.reduceMotion")}>
        <Select value={a.reduceMotion} onChange={(e) => set({ reduceMotion: e.target.value as ReduceMotion })}>
          <option value="system">{t("settings.appearance.reduceMotionSystem")}</option>
          <option value="on">{t("settings.appearance.reduceMotionOn")}</option>
          <option value="off">{t("settings.appearance.reduceMotionOff")}</option>
        </Select>
      </Setting>
    </fieldset>
  );
}
