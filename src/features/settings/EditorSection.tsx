import { useTranslation } from "react-i18next";
import { Checkbox, Select, Setting } from "../../components/Field";
import type { NewFileEol } from "../../ipc/types";
import { EDITOR_FONT_FAMILY_FIELD, FIELD, MAX_FILE_WARN_FROM_MIB } from "../../lib/settingsForm";
import { SHOW_WHITESPACE } from "../../lib/identifiers";
import { showWhitespaceMode } from "../../lib/showWhitespace";
import { FontFamilyField, FontPreview, NumberField, usableFontFamily, type FontPickerProps, type SectionProps } from "./settingsFields";

export function EditorSection(props: SectionProps & FontPickerProps) {
  const { t } = useTranslation();
  const { form, patch, invalid, installed, custom, setCustom } = props;
  const maxFileMiB = Number(form.numbers[FIELD.EDITOR_MAX_FILE]);
  return (
    <fieldset className="settings-group">
      <legend>{t("settings.editor")}</legend>
      <FontFamilyField
        value={form.editorFontFamily}
        onChange={(editorFontFamily) => patch({ editorFontFamily })}
        installed={installed}
        custom={custom}
        setCustom={setCustom}
        invalid={invalid.includes(EDITOR_FONT_FAMILY_FIELD)}
      />
      <NumberField {...props} path={FIELD.EDITOR_FONT_SIZE} label={t("settings.fontSize")} />
      <NumberField {...props} path={FIELD.EDITOR_TAB_SIZE} label={t("settings.tabSize")} />
      <NumberField {...props} path={FIELD.EDITOR_MAX_FILE} label={t("settings.maxFileSize")} />
      <FontPreview
        size={form.numbers[FIELD.EDITOR_FONT_SIZE]}
        label={t("settings.previewEditor")}
        family={usableFontFamily(form.editorFontFamily)}
        ligatures={form.fontLigatures}
      />
      <Checkbox label={t("settings.wordWrap")} checked={form.editorWordWrap} onChange={(editorWordWrap) => patch({ editorWordWrap })} />
      <Checkbox label={t("settings.lineNumbers")} checked={form.editorLineNumbers} onChange={(editorLineNumbers) => patch({ editorLineNumbers })} />
      <Checkbox
        label={t("settings.highlightActiveLine")}
        checked={form.editorHighlightActiveLine}
        onChange={(editorHighlightActiveLine) => patch({ editorHighlightActiveLine })}
      />
      <Checkbox
        label={t("settings.indentWithTabs")}
        checked={form.editorIndentWithTabs}
        onChange={(editorIndentWithTabs) => patch({ editorIndentWithTabs })}
      />
      <Checkbox
        label={t("settings.trimTrailingWhitespace")}
        info={t("settings.onSaveInfo")}
        checked={form.editorTrimTrailingWhitespace}
        onChange={(editorTrimTrailingWhitespace) => patch({ editorTrimTrailingWhitespace })}
      />
      <Checkbox
        label={t("settings.insertFinalNewline")}
        info={t("settings.onSaveInfo")}
        checked={form.editorInsertFinalNewline}
        onChange={(editorInsertFinalNewline) => patch({ editorInsertFinalNewline })}
      />
      {Number.isFinite(maxFileMiB) && maxFileMiB > MAX_FILE_WARN_FROM_MIB ? (
        <p className="dialog__warn" role="status">
          {t("settings.maxFileSizeWarning", { limit: MAX_FILE_WARN_FROM_MIB })}
        </p>
      ) : null}
      <Checkbox
        label={t("settings.fontLigatures")}
        info={t("settings.fontLigaturesInfo")}
        checked={form.fontLigatures}
        onChange={(fontLigatures) => patch({ fontLigatures })}
      />
      <Checkbox
        label={t("settings.detectIndentation")}
        info={t("settings.detectIndentationInfo")}
        checked={form.detectIndentation}
        onChange={(detectIndentation) => patch({ detectIndentation })}
      />
      <Checkbox
        label={t("settings.autoCloseBrackets")}
        checked={form.autoCloseBrackets}
        onChange={(autoCloseBrackets) => patch({ autoCloseBrackets })}
      />
      <Checkbox label={t("settings.bracketMatching")} checked={form.bracketMatching} onChange={(bracketMatching) => patch({ bracketMatching })} />
      <Checkbox label={t("settings.codeFolding")} checked={form.codeFolding} onChange={(codeFolding) => patch({ codeFolding })} />
      <Setting label={t("settings.newFileEol")} info={t("settings.newFileEolInfo")}>
        <Select value={form.newFileEol} onChange={(e) => patch({ newFileEol: e.target.value as NewFileEol })}>
          <option value="lf">{t("settings.newFileEolLf")}</option>
          <option value="crlf">{t("settings.newFileEolCrlf")}</option>
        </Select>
      </Setting>
      <Setting label={t("settings.showWhitespace")}>
        <Select value={form.showWhitespace} onChange={(e) => patch({ showWhitespace: showWhitespaceMode(e.target.value) })}>
          <option value={SHOW_WHITESPACE.all}>{t("settings.showWhitespaceAll")}</option>
          <option value={SHOW_WHITESPACE.activeLine}>{t("settings.showWhitespaceActiveLine")}</option>
          <option value={SHOW_WHITESPACE.none}>{t("settings.showWhitespaceNone")}</option>
        </Select>
      </Setting>
    </fieldset>
  );
}
