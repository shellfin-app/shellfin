import { useTranslation } from "react-i18next";
import { Checkbox, Input, Select, Setting } from "../../components/Field";
import type { CursorStyle, NewTerminalCwd, RightClickAction, TermType, TerminalBell, TerminalExitAction } from "../../ipc/types";
import {
  CONTRAST_RANGE,
  FIELD,
  FONT_FAMILY_FIELD,
  LINE_HEIGHT_FIELD,
  WORD_SEPARATORS_MAX,
  parseLineHeight,
} from "../../lib/settingsForm";
import { NEW_TERMINAL_CWD, TERMINAL_EXIT } from "../../lib/identifiers";
import { useStore } from "../../state/store";
import { FontFamilyField, NumberField, ScrollbackHint, usableFontFamily, type FontPickerProps, type SectionProps } from "./settingsFields";
import { TerminalBackgroundField } from "./TerminalBackgroundField";
import { TerminalPreview } from "./TerminalPreview";

/** Terminal: the look group (font, cursor, rendering) and the behaviour group (mouse, paste, exit, TERM, ...). */
export function TerminalSection(props: SectionProps & FontPickerProps) {
  const { t } = useTranslation();
  const { form, patch, patchLook, invalid, installed, custom, setCustom } = props;
  const mac = useStore((s) => s.platform === "macos");
  return (
    <>
      <fieldset className="settings-group">
        <legend>{t("settings.terminal")}</legend>
        <TerminalPreview
          look={form.look}
          fontFamily={usableFontFamily(form.terminalFontFamily)}
          fontSize={form.numbers[FIELD.TERMINAL_FONT_SIZE]}
          lineHeight={parseLineHeight(form.terminalLineHeight) ?? undefined}
          letterSpacing={form.numbers[FIELD.LETTER_SPACING]}
          cursorStyle={form.terminalCursorStyle}
          cursorBlink={form.terminalCursorBlink}
        />
        <FontFamilyField
          value={form.terminalFontFamily}
          onChange={(terminalFontFamily) => patch({ terminalFontFamily })}
          installed={installed}
          custom={custom}
          setCustom={setCustom}
          invalid={invalid.includes(FONT_FAMILY_FIELD)}
        />
        <NumberField {...props} path={FIELD.TERMINAL_FONT_SIZE} label={t("settings.fontSize")} />
        <Setting label={t("settings.lineHeight")} error={invalid.includes(LINE_HEIGHT_FIELD) ? t("settings.lineHeightInvalid") : null}>
          <Input
            mono
            inputMode="decimal"
            value={form.terminalLineHeight}
            aria-invalid={invalid.includes(LINE_HEIGHT_FIELD) ? true : undefined}
            onChange={(e) => patch({ terminalLineHeight: e.target.value })}
          />
        </Setting>
        <NumberField {...props} path={FIELD.TERMINAL_SCROLLBACK} label={t("settings.scrollback")} />
        <ScrollbackHint lines={Number(form.numbers[FIELD.TERMINAL_SCROLLBACK])} />
        <Setting label={t("settings.cursorStyle")}>
          <Select value={form.terminalCursorStyle} onChange={(e) => patch({ terminalCursorStyle: e.target.value as CursorStyle })}>
            <option value="block">{t("settings.cursorStyleBlock")}</option>
            <option value="bar">{t("settings.cursorStyleBar")}</option>
            <option value="underline">{t("settings.cursorStyleUnderline")}</option>
          </Select>
        </Setting>
        <Setting label={t("settings.bell")}>
          <Select value={form.terminalBell} onChange={(e) => patch({ terminalBell: e.target.value as TerminalBell })}>
            <option value="off">{t("settings.bellOff")}</option>
            <option value="visual">{t("settings.bellVisual")}</option>
            <option value="sound">{t("settings.bellSound")}</option>
          </Select>
        </Setting>
        <Setting label={t("settings.rightClick")} info={t("settings.rightClickInfo")}>
          <Select value={form.terminalRightClick} onChange={(e) => patch({ terminalRightClick: e.target.value as RightClickAction })}>
            <option value="menu">{t("settings.rightClickMenu")}</option>
            <option value="paste">{t("settings.rightClickPaste")}</option>
          </Select>
        </Setting>
        <Checkbox label={t("settings.cursorBlink")} checked={form.terminalCursorBlink} onChange={(terminalCursorBlink) => patch({ terminalCursorBlink })} />
        <Checkbox label={t("settings.copyOnSelect")} checked={form.terminalCopyOnSelect} onChange={(terminalCopyOnSelect) => patch({ terminalCopyOnSelect })} />
        <NumberField {...props} path={FIELD.LETTER_SPACING} label={t("settings.letterSpacing")} />
        <NumberField {...props} path={FIELD.SCROLL_SENSITIVITY} label={t("settings.scrollSensitivity")} />
        <Setting
          label={t("settings.minimumContrast")}
          info={t("settings.minimumContrastInfo", { min: CONTRAST_RANGE.min })}
          error={invalid.includes(FIELD.MIN_CONTRAST) ? t("settings.minimumContrastInvalid") : null}
        >
          <Input
            mono
            inputMode="decimal"
            value={form.minimumContrastRatio}
            aria-invalid={invalid.includes(FIELD.MIN_CONTRAST) ? true : undefined}
            onChange={(e) => patch({ minimumContrastRatio: e.target.value })}
          />
        </Setting>
        <Setting
          label={t("settings.wordSeparators")}
          info={t("settings.wordSeparatorsInfo", { max: WORD_SEPARATORS_MAX })}
          error={invalid.includes(FIELD.WORD_SEPARATORS) ? t("settings.wordSeparatorsInvalid", { max: WORD_SEPARATORS_MAX }) : null}
        >
          <Input
            mono
            value={form.wordSeparators}
            spellCheck={false}
            autoComplete="off"
            aria-invalid={invalid.includes(FIELD.WORD_SEPARATORS) ? true : undefined}
            onChange={(e) => patch({ wordSeparators: e.target.value })}
          />
        </Setting>
        <TerminalBackgroundField look={form.look} patch={patchLook} />
      </fieldset>
      <fieldset className="settings-group">
        <legend>{t("settings.terminalBehavior")}</legend>
        <Setting label={t("settings.onExit")}>
          <Select value={form.onExit} onChange={(e) => patch({ onExit: e.target.value as TerminalExitAction })}>
            <option value="keep">{t("settings.onExitKeep")}</option>
            <option value={TERMINAL_EXIT.closeOnSuccess}>{t("settings.onExitCloseOnSuccess")}</option>
            <option value="close">{t("settings.onExitClose")}</option>
          </Select>
        </Setting>
        <Setting label={t("settings.newTerminalCwd")} info={t("settings.newTerminalCwdInfo")}>
          <Select value={form.newTerminalCwd} onChange={(e) => patch({ newTerminalCwd: e.target.value as NewTerminalCwd })}>
            <option value="home">{t("settings.newTerminalCwdHome")}</option>
            <option value={NEW_TERMINAL_CWD.currentFolder}>{t("settings.newTerminalCwdCurrent")}</option>
          </Select>
        </Setting>
        <Setting label={t("settings.termType")} info={t("settings.termTypeInfo")}>
          <Select value={form.termType} onChange={(e) => patch({ termType: e.target.value as TermType })}>
            <option value="xterm-256color">xterm-256color</option>
            <option value="xterm">xterm</option>
            <option value="vt100">vt100</option>
          </Select>
        </Setting>
        <Checkbox
          label={t("settings.openTerminalOnConnect")}
          info={t("settings.openTerminalOnConnectInfo")}
          checked={form.openTerminalOnConnect}
          onChange={(openTerminalOnConnect) => patch({ openTerminalOnConnect })}
        />
        <Checkbox
          label={t("settings.mouseReporting")}
          info={t("settings.mouseReportingInfo")}
          checked={form.mouseReporting}
          onChange={(mouseReporting) => patch({ mouseReporting })}
        />
        <Checkbox
          label={t("settings.hideMousePointer")}
          checked={form.hideMousePointerWhileTyping}
          onChange={(hideMousePointerWhileTyping) => patch({ hideMousePointerWhileTyping })}
        />
        <Checkbox
          label={t("settings.multilinePasteWarning")}
          info={t("settings.multilinePasteWarningInfo")}
          checked={form.multilinePasteWarning}
          onChange={(multilinePasteWarning) => patch({ multilinePasteWarning })}
        />
        {mac ? (
          <Checkbox
            label={t("settings.altIsMeta")}
            info={t("settings.altIsMetaInfo")}
            checked={form.altIsMeta}
            onChange={(altIsMeta) => patch({ altIsMeta })}
          />
        ) : null}
        <Checkbox
          label={t("settings.screenReaderMode")}
          info={t("settings.screenReaderModeInfo")}
          checked={form.screenReaderMode}
          onChange={(screenReaderMode) => patch({ screenReaderMode })}
        />
      </fieldset>
    </>
  );
}
