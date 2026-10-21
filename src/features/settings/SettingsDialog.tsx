import { useCallback, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { Dialog } from "../../components/Dialog";
import { errorMessage } from "../../i18n/errorMessage";
import { getBackend } from "../../ipc";
import type { AppError } from "../../ipc/types";
import { detectInstalledFonts } from "../../lib/fontFamily";
import { reportError } from "../../lib/reportError";
import { formFromSettings, invalidFields, settingsFromForm, type SettingsFormState } from "../../lib/settingsForm";
import type { AppearanceForm } from "../../lib/settingsFormAppearance";
import { LAYOUT_DEFAULTS, resetLayoutPrefs } from "../../state/layoutPrefs";
import { useStore } from "../../state/store";
import { AdvancedSection } from "./AdvancedSection";
import { AppearanceSection, useAppearancePreview } from "./AppearanceSection";
import { ConnectionSection } from "./ConnectionSection";
import { EditorSection } from "./EditorSection";
import { FilesSection } from "./FilesSection";
import { GeneralSection } from "./GeneralSection";
import type { SectionProps } from "./settingsFields";
import { TerminalSection } from "./TerminalSection";
import { TransfersSection } from "./TransfersSection";
import "./settings.css";

const SECTIONS = ["general", "appearance", "files", "terminal", "editor", "transfers", "connection", "advanced"] as const;
type Section = (typeof SECTIONS)[number];
const tabId = (s: Section) => `settings-tab-${s}`;
const PANEL_ID = "settings-panel";

/** The section shown last time the dialog was open (kept for the rest of the run). */
let lastSection: Section = "general";
function rememberSection(s: Section): void {
  lastSection = s;
}

/** Fixed dialog size (the viewport caps it on small windows), so switching sections never resizes the modal. */
export const SETTINGS_WIDTH = 860;
export const SETTINGS_HEIGHT = 600;

/** Is "Custom…" the chosen font, given the saved family and the installed fonts? */
function isCustomFont(family: string | null, installed: string[]): boolean {
  return family !== null && !installed.includes(family);
}

/**
 * Settings (§4.4 `Settings`): the dialog owns the one form model, the live appearance preview, save and cancel; each
 * section renders its part of the form. Ranges are checked here and again by the backend (`invalidArgument{field}`).
 */
export function SettingsDialog() {
  const { t } = useTranslation();
  const settings = useStore((s) => s.settings);
  const setSettings = useStore((s) => s.setSettings);
  const closeStoreDialog = useStore((s) => s.closeDialog);
  const closeDialog = () => closeStoreDialog("settings"); // never closes a quit dialog that replaced this one
  const [form, setForm] = useState<SettingsFormState | null>(() => (settings ? formFromSettings(settings) : null));
  const [invalid, setInvalid] = useState<string[]>([]);
  const [error, setError] = useState<AppError | null>(null);
  const [busy, setBusy] = useState(false);
  const [section, setSectionState] = useState<Section>(lastSection);
  const [recentsCleared, setRecentsCleared] = useState(false);
  const installed = useMemo(() => detectInstalledFonts(), []);
  // "Custom…" in the font pickers lives here, so it survives a switch to another section and back.
  const [terminalFontCustom, setTerminalFontCustom] = useState(() => isCustomFont(settings?.terminal.fontFamily ?? null, installed));
  const [editorFontCustom, setEditorFontCustom] = useState(() => isCustomFont(settings?.editor.fontFamily ?? null, installed));
  const tabRefs = useRef<Partial<Record<Section, HTMLButtonElement | null>>>({});
  useAppearancePreview(form?.look ?? null); // live preview of theme/accent/size/motion/terminal background; undone on close

  // Functional updates: quick successive edits never overwrite each other with a stale copy of the form.
  const patch = useCallback((p: Partial<SettingsFormState>) => setForm((f) => (f ? { ...f, ...p } : f)), []);
  const patchLook = useCallback((p: Partial<AppearanceForm>) => setForm((f) => (f ? { ...f, look: { ...f.look, ...p } } : f)), []);
  const setNumber = useCallback(
    (path: string, value: string) => setForm((f) => (f ? { ...f, numbers: { ...f.numbers, [path]: value } } : f)),
    [],
  );

  if (!settings || !form) return null;

  const setSection = (s: Section) => {
    rememberSection(s);
    setSectionState(s);
  };
  const onNavKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const i = SECTIONS.indexOf(section);
    const to =
      e.key === "ArrowDown" || e.key === "ArrowRight"
        ? (i + 1) % SECTIONS.length
        : e.key === "ArrowUp" || e.key === "ArrowLeft"
          ? (i + SECTIONS.length - 1) % SECTIONS.length
          : e.key === "Home"
            ? 0
            : e.key === "End"
              ? SECTIONS.length - 1
              : -1;
    const next = SECTIONS[to];
    if (!next) return;
    e.preventDefault();
    setSection(next);
    tabRefs.current[next]?.focus();
  };

  const fail = (e: unknown) => {
    const err = (e as { error?: AppError }).error;
    if (err) setError(err);
    else reportError(e);
  };

  const resetAll = async () => {
    setBusy(true);
    setError(null);
    try {
      const defaults = await getBackend().settingsReset();
      // Local layout choices go with it (`settings_reset` resets the backend side only).
      resetLayoutPrefs();
      const ui = useStore.getState();
      ui.setActivity(LAYOUT_DEFAULTS.activity);
      ui.setSidebarOpen(LAYOUT_DEFAULTS.sidebarOpen);
      ui.setStatusBarOpen(LAYOUT_DEFAULTS.statusBarOpen);
      setSettings(defaults);
      setForm(formFromSettings(defaults));
      setTerminalFontCustom(false);
      setEditorFontCustom(false);
      setInvalid([]);
    } catch (e) {
      fail(e);
    } finally {
      setBusy(false);
    }
  };

  const clearRecents = async () => {
    setBusy(true);
    setError(null);
    try {
      await getBackend().recentsClear();
      useStore.getState().setRecents([]);
      setRecentsCleared(true);
    } catch (e) {
      fail(e);
    } finally {
      setBusy(false);
    }
  };

  const save = async () => {
    const bad = invalidFields(form);
    setInvalid(bad);
    if (bad.length > 0) {
      // Field paths start with their section ("terminal.background" -> Terminal): show the first one with a problem
      // unless the current section has one, so Save never fails without a visible reason.
      const sectionOf = (path: string) => SECTIONS.find((s) => path.startsWith(`${s}.`));
      if (!bad.some((p) => sectionOf(p) === section)) {
        const target = bad.map(sectionOf).find((s) => s !== undefined);
        if (target) setSection(target);
      }
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const saved = await getBackend().settingsUpdate(settingsFromForm(settings, form));
      setSettings(saved);
      closeDialog();
    } catch (e) {
      fail(e);
    } finally {
      setBusy(false);
    }
  };

  const props: SectionProps = { form, patch, patchLook, setNumber, invalid };

  return (
    <Dialog
      title={t("settings.title")}
      width={SETTINGS_WIDTH}
      height={SETTINGS_HEIGHT}
      onClose={closeDialog}
      footer={
        <>
          {error ? <span className="dialog__error">{errorMessage(error)}</span> : null}
          <span className="spacer" />
          <Button onClick={closeDialog}>{t("common.cancel")}</Button>
          <Button variant="primary" onClick={() => void save()} disabled={busy}>
            {t("settings.save")}
          </Button>
        </>
      }
    >
      <div className="settings-layout">
        <div className="settings-side">
          <div className="settings-nav" role="tablist" aria-orientation="vertical" aria-label={t("settings.sections")} onKeyDown={onNavKeyDown}>
            {SECTIONS.map((s) => (
              <button
                key={s}
                ref={(el) => {
                  tabRefs.current[s] = el;
                }}
                type="button"
                role="tab"
                id={tabId(s)}
                aria-selected={section === s}
                aria-controls={PANEL_ID}
                tabIndex={section === s ? 0 : -1}
                className={`settings-nav__item${section === s ? " settings-nav__item--active" : ""}`}
                onClick={() => setSection(s)}
              >
                {s === "appearance" ? t("settings.appearance.title") : t(`settings.${s}`)}
              </button>
            ))}
          </div>
        </div>
        <div className="settings-sections" role="tabpanel" id={PANEL_ID} aria-labelledby={tabId(section)}>
          {section === "general" ? <GeneralSection {...props} /> : null}
          {section === "appearance" ? <AppearanceSection look={form.look} patch={patchLook} /> : null}
          {section === "files" ? <FilesSection {...props} /> : null}
          {section === "terminal" ? (
            <TerminalSection {...props} installed={installed} custom={terminalFontCustom} setCustom={setTerminalFontCustom} />
          ) : null}
          {section === "editor" ? (
            <EditorSection {...props} installed={installed} custom={editorFontCustom} setCustom={setEditorFontCustom} />
          ) : null}
          {section === "transfers" ? <TransfersSection {...props} /> : null}
          {section === "connection" ? <ConnectionSection {...props} /> : null}
          {section === "advanced" ? (
            <AdvancedSection
              form={form}
              patch={patch}
              busy={busy}
              recentsCleared={recentsCleared}
              onClearRecents={clearRecents}
              onResetAll={resetAll}
            />
          ) : null}
        </div>
      </div>
    </Dialog>
  );
}
