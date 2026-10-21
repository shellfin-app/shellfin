// Settings > Terminal: the terminal background (`terminal.background`): follow the theme, a swatch, or a custom #RRGGBB.
// The text colours switch between the dark and the light palette by the background's luminance (terminalTheme.ts).
import { useTranslation } from "react-i18next";
import { FieldGroup, Input, Setting } from "../../components/Field";
import { parseHexColor, type AppearanceForm } from "../../lib/settingsFormAppearance";

const SWATCHES = ["#0F1115", "#000000", "#14171D", "#FFFFFF", "#FBF9F4"] as const;
type Swatch = (typeof SWATCHES)[number];

const HEX_PLACEHOLDER = "#RRGGBB";

export interface TerminalBackgroundProps {
  look: AppearanceForm;
  patch: (p: Partial<AppearanceForm>) => void;
}

export function TerminalBackgroundField({ look, patch }: TerminalBackgroundProps) {
  const { t } = useTranslation();
  const name: Record<Swatch, string> = {
    "#0F1115": t("settings.terminalBackgroundSwatchDark"),
    "#000000": t("settings.terminalBackgroundSwatchBlack"),
    "#14171D": t("settings.terminalBackgroundSwatchGraphite"),
    "#FFFFFF": t("settings.terminalBackgroundSwatchWhite"),
    "#FBF9F4": t("settings.terminalBackgroundSwatchPaper"),
  };
  const bg = look.terminalBackground;
  const typed = look.terminalBackgroundText;
  const invalid = typed.trim() !== "" && parseHexColor(typed) === null;

  const choose = (value: string | null) => patch({ terminalBackground: value, terminalBackgroundText: value ?? "" });
  const onType = (text: string) => {
    const parsed = parseHexColor(text);
    // A valid colour applies at once; an invalid one keeps the last good value and shows the error.
    patch(parsed !== null ? { terminalBackground: parsed, terminalBackgroundText: text } : { terminalBackgroundText: text });
  };

  return (
    <FieldGroup label={t("settings.terminalBackground")} info={t("settings.terminalBackgroundInfo")}>
      <div className="bg-picker">
        <button
          type="button"
          role="radio"
          aria-checked={bg === null}
          className={`bg-picker__theme${bg === null ? " bg-picker__theme--active" : ""}`}
          onClick={() => choose(null)}
        >
          {t("settings.terminalBackgroundTheme")}
        </button>
        {SWATCHES.map((value) => (
          <button
            key={value}
            type="button"
            role="radio"
            aria-checked={bg === value}
            aria-label={`${name[value]} ${value}`}
            title={`${name[value]} ${value}`}
            className={`bg-picker__swatch${bg === value ? " bg-picker__swatch--active" : ""}`}
            style={{ background: value }}
            onClick={() => choose(value)}
          />
        ))}
      </div>
      <Setting label={t("settings.terminalBackgroundCustom")} error={invalid ? t("settings.terminalBackgroundInvalid") : null}>
        <Input
          mono
          value={typed}
          maxLength={7}
          spellCheck={false}
          autoCapitalize="off"
          autoComplete="off"
          placeholder={HEX_PLACEHOLDER}
          aria-invalid={invalid ? true : undefined}
          onChange={(e) => onType(e.target.value)}
        />
      </Setting>
    </FieldGroup>
  );
}
