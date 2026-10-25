import { Suspense, lazy, useMemo, useRef, type KeyboardEvent, type MouseEvent, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { Icon } from "../../components/Icon";
import { IconButton } from "../../components/IconButton";
import { errorMessage } from "../../i18n/errorMessage";
import { formatBytes, formatRelativeTime } from "../../i18n/format";
import { encodingDialog } from "../../lib/dialogs";
import { SEL_SELECTED_TAB } from "../../lib/domIds";
import { failureKind } from "../../lib/fileFailure";
import { basename, segments } from "../../lib/remotePath";
import { guard } from "../../lib/reportError";
import { displayName } from "../../lib/serverText";
import type { BufferMeta } from "../../state/slices/buffers";
import { useStore } from "../../state/store";
import { identityOf, writability } from "../../lib/permissions";
import { lookupEntry } from "../files/fileActions";
import { openErrorDetails, sourceFromAppError } from "../errorReport/errorReport";
import { downloadFile } from "../transfers/transferActions";
import { closeBuffer, openFile, requestCloseBuffer } from "./editorActions";
import { editorStatus } from "../../lib/editorStatus";
import "./editor.css";

// CodeMirror is its own chunk; `loadBufferRegistry` fetches it together with the file, so this is already
// resolved by the time a buffer is ready.
const CodeEditor = lazy(() => import("./CodeEditor").then((m) => ({ default: m.CodeEditor })));

/** A file that could not be opened as text, with what the user can do about it (§6.3). */
function OpenFailure({ meta }: { meta: BufferMeta }) {
  const { t } = useTranslation();
  const openDialog = useStore((s) => s.openDialog);
  const connected = useStore((s) => s.sessionsById[meta.sessionId]?.state.phase === "connected");
  const name = displayName(basename(meta.path));
  const failure = meta.failure;
  const kind = failureKind(failure);
  const download = () => guard(downloadFile(meta.sessionId, meta.path));
  const closeTab = (
    <Button onClick={() => closeBuffer(meta.sessionId, meta.id)}>{t("editor.closeThisTab")}</Button>
  );

  let text: string;
  let actions: ReactNode;
  switch (kind) {
    case "binary":
      text = t("editor.binary", { name });
      actions = (
        <>
          <Button variant="primary" icon="download" onClick={download} disabled={!connected}>
            {t("files.download")}
          </Button>
          {closeTab}
        </>
      );
      break;
    case "tooLarge": {
      const params = failure?.code === "fileTooLarge" ? failure.params : null;
      text = t("editor.tooLarge", { name, size: formatBytes(params?.size ?? 0), limit: formatBytes(params?.limit ?? 0) });
      actions = (
        <>
          <Button variant="primary" icon="download" onClick={download} disabled={!connected}>
            {t("files.download")}
          </Button>
          <Button onClick={() => openDialog({ kind: "settings" })}>{t("editor.openSettings")}</Button>
          {closeTab}
        </>
      );
      break;
    }
    case "decode": {
      const encoding = failure && "params" in failure && "encoding" in failure.params ? failure.params.encoding : "";
      text = failure?.code === "unsupportedEncoding" ? t("editor.unsupportedEncoding", { name, encoding }) : t("editor.notText", { name, encoding });
      actions = (
        <>
          <Button variant="primary" onClick={() => openDialog(encodingDialog(meta.id))} disabled={!connected}>
            {t("editor.reopenWithEncoding")}
          </Button>
          <Button icon="download" onClick={download} disabled={!connected}>
            {t("files.download")}
          </Button>
          {closeTab}
        </>
      );
      break;
    }
    default:
      text = t("editor.loadFailed", { name });
      actions = (
        <>
          <Button variant="primary" icon="refresh" onClick={() => guard(openFile(meta.sessionId, meta.path))} disabled={!connected}>
            {t("session.retry")}
          </Button>
          {failure ? <Button onClick={() => openErrorDetails(sourceFromAppError(failure))}>{t("errorReport.details")}</Button> : null}
          {closeTab}
        </>
      );
  }
  return (
    <div className="editor-failure" role="status">
      <Icon name="alert" size={22} strokeWidth={1.8} className="editor-failure__icon" />
      <p className="editor-failure__text">{text}</p>
      {kind === "other" && failure ? <p className="editor-failure__detail">{errorMessage(failure)}</p> : null}
      <div className="editor-failure__actions">{actions}</div>
    </div>
  );
}

/** Editor tabs + breadcrumbs + CodeMirror (Main.dc "main" column, top part). */
export function EditorArea({ sessionId }: { sessionId: string }) {
  const { t } = useTranslation();
  const tabs = useStore((s) => s.workspaces[sessionId]?.editorTabs);
  const activeId = useStore((s) => s.workspaces[sessionId]?.activeBuffer ?? null);
  const buffers = useStore((s) => s.buffers);
  const label = useStore((s) => s.sessionsById[sessionId]?.label ?? "");
  const setActiveBuffer = useStore((s) => s.setActiveBuffer);
  const listRef = useRef<HTMLDivElement>(null);
  const active = activeId ? buffers[activeId] : undefined;
  const connected = useStore((s) => s.sessionsById[sessionId]?.state.phase === "connected");
  const tree = useStore((s) => s.workspaces[sessionId]?.tree);
  const sessionInfo = useStore((s) => s.sessionsById[sessionId]?.info);
  const identity = useMemo(() => identityOf(sessionInfo), [sessionInfo]);
  // Permission hint: only a hint, saving is still allowed. Unknown permissions or identity = no hint.
  const readOnlyHint =
    active !== undefined && active.load === "ready" && tree !== undefined && writability(lookupEntry(tree, active.path), identity) === "no";
  const status = active ? editorStatus(active, connected) : ({ kind: "none" } as const);

  const onTabKey = (e: KeyboardEvent<HTMLButtonElement>, index: number) => {
    const ids = tabs ?? [];
    const targets: Record<string, number> = {
      ArrowRight: (index + 1) % ids.length,
      ArrowLeft: (index - 1 + ids.length) % ids.length,
      Home: 0,
      End: ids.length - 1,
    };
    const target = targets[e.key];
    if (target === undefined) return;
    e.preventDefault();
    const id = ids[target];
    if (!id) return;
    setActiveBuffer(sessionId, id);
    // The newly selected tab is the only one in the tab order after the re-render.
    setTimeout(() => listRef.current?.querySelector<HTMLElement>(SEL_SELECTED_TAB)?.focus(), 0);
  };
  const onTabAux = (e: MouseEvent, id: string) => {
    if (e.button === 1) {
      e.preventDefault();
      requestCloseBuffer(id);
    }
  };

  return (
    <div className="editor-area">
      <div className="editor-tabs">
        <div ref={listRef} className="editor-tabs__list" role="tablist" aria-label={t("editor.tabs")}>
          {(tabs ?? []).map((id, index) => {
            const b = buffers[id];
            if (!b) return null;
            const name = displayName(basename(b.path));
            const selected = id === activeId;
            return (
              <div
                key={id}
                className={`editor-tab${selected ? " editor-tab--active" : ""}`}
                data-editor-tab={id}
                data-session-id={sessionId}
              >
                <button
                  type="button"
                  role="tab"
                  aria-selected={selected}
                  tabIndex={selected ? 0 : -1}
                  className="editor-tab__main"
                  onClick={() => setActiveBuffer(sessionId, id)}
                  onAuxClick={(e) => onTabAux(e, id)}
                  onKeyDown={(e) => onTabKey(e, index)}
                >
                  <bdi>{name}</bdi>
                  {b.conflict ? (
                    <span className="editor-tab__flag editor-tab__flag--danger" role="img" aria-label={t("editor.conflict")} title={t("editor.conflict")} />
                  ) : b.pendingSave ? (
                    <span className="editor-tab__flag" role="img" aria-label={t("editor.pendingSave")} title={t("editor.pendingSave")} />
                  ) : null}
                  {b.dirty ? <span className="editor-tab__dirty" role="img" aria-label={t("editor.unsaved")} title={t("editor.unsaved")} /> : null}
                </button>
                <IconButton
                  icon="close"
                  iconSize={11}
                  size={20}
                  label={t("editor.closeTab", { name })}
                  tabIndex={-1}
                  onClick={() => requestCloseBuffer(id)}
                />
              </div>
            );
          })}
        </div>
        {/* Split editor: not in v1 (§1.1), kept to match the mockup. */}
        <div className="editor-tabs__actions">
          <IconButton icon="split" iconSize={16} size={30} label={t("editor.split")} disabled />
        </div>
      </div>

      {active ? (
        <div className="breadcrumbs mono">
          <bdi>{displayName(label)}</bdi>
          {segments(active.path).map((seg, i, all) => (
            <span key={`${i}:${seg}`} className="breadcrumbs__item">
              <span className="breadcrumbs__sep" aria-hidden="true" />
              <bdi className={i === all.length - 1 ? "breadcrumbs__current" : undefined}>{displayName(seg)}</bdi>
            </span>
          ))}
          {status.kind === "saving" ? (
            <span className="breadcrumbs__saved">{t("editor.saving")}</span>
          ) : status.kind === "conflict" ? (
            <span className="breadcrumbs__saved breadcrumbs__saved--danger">{t("editor.conflict")}</span>
          ) : status.kind === "pendingSave" ? (
            <span className="breadcrumbs__saved breadcrumbs__saved--warn">{t("editor.pendingSave")}</span>
          ) : status.kind === "offlineDirty" ? (
            <span className="breadcrumbs__saved breadcrumbs__saved--warn">
              <span className="breadcrumbs__dot" aria-hidden="true" />
              {t("editor.offlineDirty")}
            </span>
          ) : status.kind === "unsaved" ? (
            <span className="breadcrumbs__saved breadcrumbs__saved--warn">
              <span className="breadcrumbs__dot" aria-hidden="true" />
              {status.savedAtMs !== null
                ? t("editor.unsavedSince", { time: formatRelativeTime(status.savedAtMs) })
                : t("editor.unsaved")}
            </span>
          ) : status.kind === "saved" ? (
            <span className="breadcrumbs__saved">{t("editor.savedAgo", { time: formatRelativeTime(status.savedAtMs) })}</span>
          ) : null}
        </div>
      ) : null}

      {readOnlyHint ? (
        <p className="editor-banner" role="status">
          <Icon name="lock" size={13} />
          {t("editor.readOnlyBanner")}
        </p>
      ) : null}

      <div className="editor-area__body">
        {active && active.load === "ready" ? (
          <Suspense fallback={null}>
            <CodeEditor bufferId={active.id} />
          </Suspense>
        ) : null}
        {active && active.load === "loading" ? (
          <p className="editor-area__note">{t("editor.loading", { name: displayName(basename(active.path)) })}</p>
        ) : null}
        {active && active.load === "error" ? <OpenFailure meta={active} /> : null}
        {!active ? <p className="editor-area__note">{t("editor.empty")}</p> : null}
      </div>
    </div>
  );
}
