import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { BandwidthInput } from "../../components/BandwidthInput";
import { Dialog } from "../../components/Dialog";
import { ColorPicker } from "./ColorPicker";
import { Checkbox, Field, FieldGroup, Input, SegmentedControl, Select } from "../../components/Field";
import { Icon } from "../../components/Icon";
import { errorMessage } from "../../i18n/errorMessage";
import { getBackend } from "../../ipc";
import type { AppError } from "../../ipc/types";
import {
  NEW_GROUP,
  NOTES_MAX,
  SUBMIT_ACTION,
  bindingChanged,
  emptyForm,
  formFromBookmark,
  formFromSpec,
  toBookmarkInput,
  validateForm,
  type AuthMode,
  type BookmarkFormState,
  type FormErrorKey,
  type FormErrors,
  type LimitMode,
  type SubmitAction,
} from "../../lib/bookmarkForm";
import type { BandwidthField } from "../../lib/bandwidth";
import { parseHostSpec } from "../../lib/hostSpec";
import { displayServerText } from "../../lib/serverText";
import { reportError } from "../../lib/reportError";
import { useStore } from "../../state/store";
import { canReplaceSession } from "../session/editConnection";
import { closeSession, startProbe, type Probe } from "../session/sessionActions";
import { submitBookmarkForm, type SubmitIntent } from "./hostActions";
import "./bookmarkDialog.css";

/** Per-bookmark transfer speed: global / no limit / own limit. */
function LimitControl({
  label,
  mode,
  field,
  error,
  onChange,
}: {
  label: string;
  mode: LimitMode;
  field: BandwidthField;
  error: string | null;
  onChange: (mode: LimitMode, field: BandwidthField) => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="bookmark-form__limit">
      <Field label={label}>
        <Select value={mode} onChange={(e) => onChange(e.target.value as LimitMode, field)}>
          <option value="global">{t("connect.bandwidthGlobal")}</option>
          <option value="unlimited">{t("connect.bandwidthUnlimited")}</option>
          <option value="limit">{t("connect.bandwidthLimit")}</option>
        </Select>
      </Field>
      {mode === "limit" ? (
        <BandwidthInput label={label} field={field} invalid={error !== null} onChange={(f) => onChange(mode, f)} />
      ) : null}
    </div>
  );
}

type TestState =
  | { status: "idle" }
  | { status: "running" }
  | { status: "ok" }
  | { status: "failed"; error: AppError | null };

/** New connection / edit bookmark (Connect.dc): all auth modes, ProxyJump, groups, start dir, test connection. */
export function BookmarkDialog({
  bookmarkId,
  prefill,
  replaceSession,
}: {
  bookmarkId: string | null;
  prefill?: { spec: string; name?: string; sshConfig?: boolean };
  replaceSession?: string;
}) {
  const { t } = useTranslation();
  const bookmarks = useStore((s) => s.bookmarks);
  const sshHosts = useStore((s) => s.sshConfig?.hosts);
  const sshFile = useStore((s) => s.sshConfig?.mainFile ?? "");
  const agent = useStore((s) => s.agent);
  // `connection.rememberPasswords = false`: no "save password" option (a password saved earlier keeps working).
  const canSavePassword = useStore((s) => s.settings?.connection.rememberPasswords !== false);
  const closeStoreDialog = useStore((s) => s.closeDialog);
  // Close only THIS dialog: the quit confirmation may have replaced it while a submit was pending.
  const closeDialog = () => closeStoreDialog("bookmark");

  // After the first successful save the dialog switches to edit mode with the saved id.
  const [editingId, setEditingId] = useState<string | null>(bookmarkId);
  const readOnly = bookmarks?.readOnly ?? false;
  const editing = useMemo(
    () => (editingId ? (bookmarks?.bookmarks.find((b) => b.id === editingId) ?? null) : null),
    [editingId, bookmarks],
  );
  const [probeSession, setProbeSession] = useState<string | null>(null);
  const probeWaiting = useStore((s) => (probeSession !== null && s.prompts[probeSession] !== undefined ? probeSession : null));
  const showPromptOf = useStore((s) => s.showPromptOf);
  const [form, setForm] = useState<BookmarkFormState>(() => {
    const defaultUser = useStore.getState().settings?.connection.defaultUser ?? "";
    const base = editing
      ? formFromBookmark(editing)
      : prefill
        ? formFromSpec(prefill.spec, prefill.name, prefill.sshConfig === true ? "" : defaultUser) // the config's User wins
        : emptyForm(defaultUser);
    return readOnly ? { ...base, saveAsBookmark: false } : base;
  });
  const [errors, setErrors] = useState<FormErrors>({});
  const [submitError, setSubmitError] = useState<AppError | null>(null);
  const [busy, setBusy] = useState(false);
  const [test, setTest] = useState<TestState>({ status: "idle" });
  const probeRef = useRef<Probe | null>(null);

  const patch = (p: Partial<BookmarkFormState>) => {
    setForm((f) => ({ ...f, ...p }));
    setTest((s) => (s.status === "running" ? s : { status: "idle" }));
  };

  // Leaving the dialog aborts a running test connection.
  useEffect(() => () => probeRef.current?.cancel(), []);

  const warnForget = editing !== null && bindingChanged(editing, form);
  const groups = bookmarks?.groups ?? [];
  const jumpHosts = (sshHosts ?? []).filter((h) => h.alias !== editing?.name);
  // The host field names an SSH config alias: its `User` applies when this field stays empty (placeholder only).
  // Trimmed and case-sensitive, like the backend's `Host` matching (OpenSSH: `Host Lab` does not apply to `lab`).
  const hostAlias = form.host.trim();
  const configHost = (sshHosts ?? []).find((h) => h.alias === hostAlias);
  const configUserRaw = configHost?.user;
  const configUser = configUserRaw ? displayServerText(configUserRaw, 64) : null;
  // A config alias that differs only in case does not apply (OpenSSH `Host` is case-sensitive): say so,
  // or a bookmark `DB` silently loses the settings of `Host db`.
  const lowerHost = hostAlias.toLowerCase();
  const caseOnlyAlias =
    hostAlias === "" || configHost !== undefined ? undefined : (sshHosts ?? []).find((h) => h.alias.toLowerCase() === lowerHost)?.alias;
  const jumpBookmarks = (bookmarks?.bookmarks ?? []).filter((b) => b.id !== editing?.id);

  const validate = (action: SubmitAction = SUBMIT_ACTION.connect): boolean => {
    // A group only matters when the bookmark is kept.
    const e = validateForm({ ...form, saveAsBookmark: action !== SUBMIT_ACTION.connect });
    setErrors(e);
    return Object.keys(e).length === 0;
  };

  /** Host field: a pasted `user@host:port` fills Host, User and Port. */
  const splitHost = (text: string): boolean => {
    const parts = parseHostSpec(text);
    if (!parts) return false;
    patch({
      host: parts.host,
      ...(parts.user !== null ? { user: parts.user } : {}),
      ...(parts.port !== null ? { port: String(parts.port) } : {}),
    });
    return true;
  };

  const submit = async (action: SubmitAction) => {
    if (!validate(action)) return;
    setBusy(true);
    setSubmitError(null);
    const intent: SubmitIntent = action === SUBMIT_ACTION.save ? "save" : "connect";
    try {
      await submitBookmarkForm(
        { ...form, saveAsBookmark: action !== SUBMIT_ACTION.connect },
        {
        editing,
        intent,
        beforeConnect: async () => {
          // Only a tab that failed before ever connecting is replaced; a live workspace is never closed here.
          if (replaceSession && canReplaceSession(replaceSession)) await closeSession(replaceSession);
        },
        onSaved: (saved, groupId) => {
          setEditingId(saved.id);
          setForm((f) => ({ ...f, groupId: groupId ?? "", newGroupName: "" }));
        },
        },
      );
      closeDialog();
    } catch (e) {
      const err = (e as { error?: AppError }).error;
      if (err) setSubmitError(err);
      else reportError(e);
    } finally {
      setBusy(false);
    }
  };

  const runTest = () => {
    if (!validate()) return;
    probeRef.current?.cancel();
    setProbeSession(null);
    const draft = {
      bookmark: toBookmarkInput(form, editing?.id ?? null, form.groupId === "" || form.groupId === NEW_GROUP ? null : form.groupId),
      password: form.auth === "password" && form.password !== "" ? form.password : null,
    };
    setTest({ status: "running" });
    const probe = startProbe(draft, setProbeSession);
    probeRef.current = probe;
    void probe.result.then((r) => {
      if (probeRef.current !== probe) return;
      setTest(r.ok ? { status: "ok" } : { status: "failed", error: r.error });
    });
  };

  const browse = async () => {
    try {
      const path = await getBackend().dialogPickIdentityFile(t("connect.browseTitle"), t("connect.keyFilter"));
      if (path) patch({ identityFile: path });
    } catch (e) {
      reportError(e);
    }
  };

  const authOptions = [
    { value: "auto" as const, label: t("connect.authAuto") },
    { value: "key" as const, label: t("connect.authKey") },
    { value: "password" as const, label: t("connect.authPassword") },
    { value: "agent" as const, label: t("connect.authAgent") },
  ];

  const err = (key: FormErrorKey | undefined) => (key ? t(`connect.errors.${key}`) : null);

  return (
    <Dialog
      title={editing ? t("connect.title.edit") : t("connect.title.new")}
      width={720}
      onClose={closeDialog}
      showClose
      footer={
        <>
          <Button variant="ghost" icon="plug" onClick={runTest} disabled={busy || test.status === "running"}>
            {t("connect.test")}
          </Button>
          <span className="spacer" />
          <Button onClick={closeDialog}>{t("common.cancel")}</Button>
          {readOnly ? (
            <Button variant="primary" onClick={() => void submit(SUBMIT_ACTION.connect)} disabled={busy}>
              {t("connect.connect")}
            </Button>
          ) : (
            <>
              {editing ? null : (
                <Button onClick={() => void submit(SUBMIT_ACTION.connect)} disabled={busy}>
                  {t("connect.connect")}
                </Button>
              )}
              <Button onClick={() => void submit(SUBMIT_ACTION.save)} disabled={busy}>
                {t("connect.save")}
              </Button>
              <Button variant="primary" onClick={() => void submit(SUBMIT_ACTION.saveConnect)} disabled={busy}>
                {t("connect.saveAndConnect")}
              </Button>
            </>
          )}
        </>
      }
    >
      <form
        className="bookmark-form"
        onSubmit={(e) => {
          e.preventDefault();
          void submit(readOnly ? SUBMIT_ACTION.connect : SUBMIT_ACTION.saveConnect);
        }}
      >
        <Field label={t("connect.host")} error={err(errors.host)} hint={t("connect.hostHint")}>
          <Input
            data-autofocus
            mono
            value={form.host}
            placeholder={t("connect.hostPlaceholder")}
            onChange={(e) => patch({ host: e.target.value })}
            onPaste={(e) => {
              if (splitHost(e.clipboardData.getData("text"))) e.preventDefault();
            }}
            onBlur={(e) => {
              splitHost(e.target.value);
            }}
            autoComplete="off"
            spellCheck={false}
            aria-invalid={errors.host ? true : undefined}
          />
        </Field>
        {caseOnlyAlias === undefined ? null : (
          <p className="dialog__warn" role="status">
            {t("connect.hostCaseMismatch", { alias: displayServerText(caseOnlyAlias, 64) })}
          </p>
        )}
        <div className="bookmark-form__row">
          <Field label={t("connect.user")}>
            <Input
              mono
              value={form.user}
              placeholder={configUser === null ? t("connect.userPlaceholder") : t("connect.userPlaceholderSshConfig", { user: configUser })}
              onChange={(e) => patch({ user: e.target.value })}
              autoComplete="off"
              spellCheck={false}
            />
          </Field>
          <Field label={t("connect.port")} error={err(errors.port)}>
            <Input
              mono
              inputMode="numeric"
              value={form.port}
              placeholder="22"
              onChange={(e) => patch({ port: e.target.value })}
              aria-invalid={errors.port ? true : undefined}
            />
          </Field>
        </div>
        <Field label={t("connect.name")} hint={t("connect.nameHint")}>
          <Input
            value={form.name}
            placeholder={form.host.trim() || t("connect.namePlaceholder")}
            onChange={(e) => patch({ name: e.target.value })}
          />
        </Field>

        <SegmentedControl<AuthMode>
          label={t("connect.authLabel")}
          options={authOptions}
          value={form.auth}
          onChange={(auth) => patch({ auth })}
        />

        {form.auth === "auto" ? <p className="dialog__hint">{t("connect.authAutoHelp")}</p> : null}

        {form.auth === "key" ? (
          <div className="bookmark-form__section">
            <FieldGroup label={t("connect.privateKey")} error={err(errors.identityFile)}>
              <div className="bookmark-form__inline">
                <Input
                  mono
                  aria-label={t("connect.privateKey")}
                  value={form.identityFile}
                  onChange={(e) => patch({ identityFile: e.target.value })}
                  spellCheck={false}
                  aria-invalid={errors.identityFile ? true : undefined}
                />
                <Button onClick={() => void browse()}>{t("connect.browse")}</Button>
              </div>
            </FieldGroup>
            <span className="dialog__hint">{t("connect.passphraseHint")}</span>
          </div>
        ) : null}

        {form.auth === "password" ? (
          <div className="bookmark-form__section">
            <Field label={t("connect.authPassword")}>
              <Input
                type="password"
                autoComplete="off"
                value={form.password}
                onChange={(e) => patch({ password: e.target.value })}
              />
            </Field>
            {canSavePassword ? (
              <Checkbox
                label={t("connect.savePassword")}
                checked={form.savePassword}
                onChange={(savePassword) => patch({ savePassword })}
              />
            ) : (
              <p className="dialog__hint">{t("connect.savePasswordOff")}</p>
            )}
            {warnForget ? <p className="dialog__warn" role="status">{t("connect.passwordWillBeForgotten")}</p> : null}
          </div>
        ) : null}

        {form.auth === "agent" ? (
          <div className="bookmark-form__agent">
            <span className="dialog__hint">{t("connect.agentKeys")}</span>
            {agent?.available && agent.identities.length > 0 ? (
              agent.identities.map((k) => (
                <span key={k.fingerprint} className="mono bookmark-form__key">
                  <bdi>{`${k.keyType} ${k.fingerprint} ${k.comment}`}</bdi>
                </span>
              ))
            ) : (
              <span className="dialog__hint">{t("welcome.agentUnavailable")}</span>
            )}
          </div>
        ) : null}

        <div className="bookmark-form__divider" />

        <div className="bookmark-form__row bookmark-form__row--even">
          <Field label={t("connect.proxyJump")}>
            <Select value={form.proxyJump} onChange={(e) => patch({ proxyJump: e.target.value })}>
              <option value="">{t("connect.proxyNone")}</option>
              {jumpHosts.map((h) => (
                <option key={`ssh:${h.alias}`} value={`ssh:${h.alias}`}>
                  {t("connect.proxyFromSshConfig", { name: h.alias, file: sshFile })}
                </option>
              ))}
              {jumpBookmarks.map((b) => (
                <option key={`bm:${b.id}`} value={`bm:${b.id}`}>
                  {b.name}
                </option>
              ))}
            </Select>
          </Field>
          <Field label={t("connect.group")}>
            <Select
              value={form.groupId}
              disabled={readOnly}
              onChange={(e) => patch({ groupId: e.target.value })}
            >
              <option value="">{t("connect.noGroup")}</option>
              {groups.map((g) => (
                <option key={g.id} value={g.id}>
                  {g.name}
                </option>
              ))}
              <option value={NEW_GROUP}>{t("connect.newGroup")}</option>
            </Select>
          </Field>
        </div>
        {form.groupId === NEW_GROUP && !readOnly ? (
          <Field label={t("connect.newGroupName")} error={err(errors.newGroupName)}>
            <Input
              value={form.newGroupName}
              onChange={(e) => patch({ newGroupName: e.target.value })}
              aria-invalid={errors.newGroupName ? true : undefined}
            />
          </Field>
        ) : null}
        <ColorPicker value={form.color} disabled={readOnly} onChange={(color) => patch({ color })} />
        <Field label={t("connect.startDir")}>
          <Input mono value={form.startDir} onChange={(e) => patch({ startDir: e.target.value })} spellCheck={false} />
        </Field>
        <Field label={t("connect.startupCommand")} hint={t("connect.startupCommandHint")} error={err(errors.startupCommand)}>
          <Input
            mono
            value={form.startupCommand}
            onChange={(e) => patch({ startupCommand: e.target.value })}
            spellCheck={false}
            autoComplete="off"
            aria-invalid={errors.startupCommand ? true : undefined}
          />
        </Field>
        <Checkbox
          label={t("connect.confirmConnect")}
          hint={t("connect.confirmConnectHint")}
          checked={form.confirmConnect}
          onChange={(confirmConnect) => patch({ confirmConnect })}
        />
        <Field label={t("connect.notes")} hint={t("connect.notesHint", { max: NOTES_MAX })} error={err(errors.notes)}>
          <textarea
            className="input bookmark-form__notes"
            rows={3}
            value={form.notes}
            onChange={(e) => patch({ notes: e.target.value })}
            aria-invalid={errors.notes ? true : undefined}
          />
        </Field>

        <fieldset className="bookmark-form__speed">
          <legend>{t("connect.bandwidthTitle")}</legend>
          <div className="bookmark-form__row bookmark-form__row--even">
            <LimitControl
              label={t("connect.uploadLimit")}
              mode={form.uploadMode}
              field={form.uploadLimit}
              error={err(errors.uploadLimit)}
              onChange={(uploadMode, uploadLimit) => patch({ uploadMode, uploadLimit })}
            />
            <LimitControl
              label={t("connect.downloadLimit")}
              mode={form.downloadMode}
              field={form.downloadLimit}
              error={err(errors.downloadLimit)}
              onChange={(downloadMode, downloadLimit) => patch({ downloadMode, downloadLimit })}
            />
          </div>
        </fieldset>

        {readOnly ? <p className="dialog__hint">{t("hosts.readOnlyHint")}</p> : null}
        <div className="bookmark-form__status" role="status" aria-live="polite">
          {test.status === "running" ? <span className="dialog__hint">{t("connect.testRunning")}</span> : null}
          {probeWaiting ? (
            <span className="bookmark-form__waiting">
              {t("connect.testNeedsAnswer")}
              <Button onClick={() => showPromptOf(probeWaiting)}>{t("connect.testAnswer")}</Button>
            </span>
          ) : null}
          {test.status === "ok" ? (
            <span className="bookmark-form__ok">
              <Icon name="check" size={14} /> {t("connect.testOk")}
            </span>
          ) : null}
          {test.status === "failed" ? (
            <span className="dialog__error">
              {t("connect.testFailed")}
              {test.error ? <bdi className="bookmark-form__reason">{errorMessage(test.error)}</bdi> : null}
            </span>
          ) : null}
          {submitError ? <span className="dialog__error">{errorMessage(submitError)}</span> : null}
        </div>
      </form>
    </Dialog>
  );
}
