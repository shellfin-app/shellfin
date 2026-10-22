// Session lifecycle glue between the Backend and the store (docs/ARCHITECTURE.md §5.8, §8.3).
import { getBackend } from "../../ipc";
import type {
  AppError,
  BookmarkInput,
  OpenSessionRequest,
  PromptAnswer,
  RecentTarget,
  SessionEvent,
  SessionInfo,
  Target,
} from "../../ipc/types";
import { guard, reportError } from "../../lib/reportError";
import { closeSessionDialog, confirmCloseTerminalsDialog } from "../../lib/dialogs";
import { quickTarget } from "../../lib/targets";
import { useStore } from "../../state/store";
import { dropBuffer } from "../editor/bufferStore";
import { dirtyBuffers, runPendingSaves } from "../editor/editorActions";
import { loadRoot, refreshTree } from "../files/fileActions";
import { confirmCloseWithTerminals, openTerminalTab, runningTerminals } from "../terminal/terminalActions";
import { disposeTerminalIfLoaded } from "../terminal/terminalLoader";
import { requestConnectConfirm } from "./connectConfirm";
import { armStartupCommand, disarmStartupCommand, startupCommandFor } from "./startupCommand";

export function sameTarget(a: RecentTarget, b: RecentTarget): boolean {
  if (a.kind !== b.kind) return false;
  switch (a.kind) {
    case "sshConfig":
      return b.kind === "sshConfig" && a.alias === b.alias;
    case "bookmark":
      return b.kind === "bookmark" && a.id === b.id;
    case "quick":
      return b.kind === "quick" && a.spec === b.spec;
  }
}

/** Finds an already-open session for a target, if any. */
export function findSession(target: RecentTarget): string | null {
  const { sessionsById, sessionOrder } = useStore.getState();
  for (const id of sessionOrder) {
    const entry = sessionsById[id];
    if (entry && sameTarget(entry.target, target)) return id;
  }
  return null;
}

/** A not-yet-saved connection. The password only ever travels in the `session_open` request, never into the store. */
export interface DraftConnection {
  bookmark: BookmarkInput;
  password: string | null;
}

/** `user@host:port` — what recents record for a draft (§5.1 rule 4). */
export function draftSpec(bookmark: BookmarkInput): string {
  const user = bookmark.user ? `${bookmark.user}@` : "";
  return `${user}${bookmark.host}:${bookmark.port ?? 22}`;
}

async function onConnected(sessionId: string, info: SessionInfo): Promise<void> {
  const store = useStore.getState();
  store.ensureWorkspace(sessionId);
  const root = info.startDir ?? info.homeDir;
  const ws = useStore.getState().workspaces[sessionId];
  if (root && ws && ws.tree.root === null) {
    store.setTreeRoot(sessionId, root);
    await loadRoot(sessionId, root);
  }
  // The tab may have been closed while the root listing loaded: nothing more to do, and no error toasts.
  if (!useStore.getState().sessionsById[sessionId]) return;
  // `terminal.openTerminalOnConnect` (default on): the first terminal opens with the connection.
  if (ws &&ws.terminalTabs.length === 0 && useStore.getState().settings?.terminal.openTerminalOnConnect !== false) {
    try {
      await openTerminalTab(sessionId);
    } catch (e) {
      reportError(e);
    }
  }
}

/**
 * The connection came back after a loss (§5.8 effects): the tree marks its listings stale and refreshes the expanded
 * folders, saves that were requested while offline run now (with etag checks). Terminals are respawned by the
 * backend, which announces it on each terminal channel (`reconnected`).
 */
async function onReconnected(sessionId: string, info: SessionInfo): Promise<void> {
  const store = useStore.getState();
  store.markListingsStale(sessionId);
  const tasks: Promise<unknown>[] = [runPendingSaves(sessionId)];
  const ws = useStore.getState().workspaces[sessionId];
  const root = info.startDir ?? info.homeDir;
  if (info.sftpAvailable && ws && ws.tree.root === null && root) {
    // SFTP failed on the first connect and works now: the tree gets its root only here.
    store.setTreeRoot(sessionId, root);
    tasks.push(loadRoot(sessionId, root));
  } else if (info.sftpAvailable) {
    tasks.push(refreshTree(sessionId));
  }
  await Promise.all(tasks);
}

/** Applies a backend event to the store and runs the follow-up side effects (§5.8 effects). */
function applyEvent(sessionId: string, event: SessionEvent): void {
  const store = useStore.getState();
  const entry = store.sessionsById[sessionId];
  if (!entry) return;
  store.applySessionEvent(sessionId, event);
  if (event.type === "warning") {
    store.pushNotice(event.warning, "warning");
  } else if (event.type === "state" && event.state.phase === "connected" && !entry.probe) {
    // `entry` is the state before this event: a session that had info was connected once and has now recovered.
    guard(entry.info ? onReconnected(sessionId, event.state.info) : onConnected(sessionId, event.state.info));
    // A first-time host key was written to known_hosts before `connected`: the Home card's count is stale.
    guard(getBackend().envInfo().then((env) => useStore.getState().setEnv(env)));
    // A remembered password flips the bookmark's `passwordSaved` flag without an event.
    if (entry.target.kind === "bookmark") guard(refreshBookmarksFromBackend());
  }
}

async function refreshBookmarksFromBackend(): Promise<void> {
  useStore.getState().setBookmarks(await getBackend().bookmarksGet());
}

/** In-flight `openSession` calls per target: a double click must not open two sessions. */
const pendingOpens = new Map<string, Promise<string>>();

function targetKey(target: RecentTarget): string {
  return JSON.stringify(target);
}

/**
 * Opens a session for `target` (or activates the existing / in-flight one). Returns the sessionId.
 * With `draft`, the request carries the draft (incl. its password) while the store only keeps `target`.
 */
export function openSession(
  target: RecentTarget,
  label: string,
  draft?: DraftConnection,
  options: { forceNew?: boolean; preferredInstance?: number } = {},
): Promise<string> {
  // `forceNew` ("Open in new tab", "Duplicate connection"): a second session to the same target, never the existing one.
  if (options.forceNew) return doOpenSession(target, label, draft, options.preferredInstance);
  const existing = findSession(target);
  if (existing) {
    useStore.getState().setActiveSession(existing);
    return Promise.resolve(existing);
  }
  const key = targetKey(target);
  const inFlight = pendingOpens.get(key);
  if (inFlight) return inFlight;
  const promise = doOpenSession(target, label, draft, options.preferredInstance).finally(() => pendingOpens.delete(key));
  pendingOpens.set(key, promise);
  return promise;
}

/**
 * A user-initiated connect (double click, menu, palette, recents, quick connect, startup restore): like `openSession`,
 * but a bookmark with "Confirm before connecting" asks first. `null` = the user said no. Opening an already open
 * connection only activates its tab and never asks. Automatic reconnect does not go through here.
 */
export async function requestOpenSession(
  target: RecentTarget,
  label: string,
  draft?: DraftConnection,
  options: { forceNew?: boolean; background?: boolean } = {},
): Promise<string | null> {
  if (target.kind === "bookmark" && (options.forceNew || findSession(target) === null)) {
    const bookmark = useStore.getState().bookmarks?.bookmarks.find((b) => b.id === target.id);
    if (bookmark?.confirmConnect && !(await requestConnectConfirm(bookmark.name, { background: options.background === true }))) {
      return null;
    }
  }
  return openSession(target, label, draft, options.forceNew === true ? { forceNew: true } : {});
}

async function doOpenSession(
  target: RecentTarget,
  label: string,
  draft?: DraftConnection,
  preferredInstance?: number,
): Promise<string> {
  // Events can arrive before the command resolves with the id; queue them until the session is registered.
  let sessionId: string | null = null;
  const queued: SessionEvent[] = [];
  const request: OpenSessionRequest = {
    target: draft ? { kind: "draft", bookmark: draft.bookmark, password: draft.password } : target,
    probeOnly: false,
  };

  sessionId = await getBackend().openSession(request, (event) => {
    if (sessionId) applyEvent(sessionId, event);
    else queued.push(event);
  });
  useStore.getState().addSession(sessionId, target, label, false, preferredInstance);
  // The bookmark's startup command waits for the first terminal of this connection (never for a probe).
  armStartupCommand(sessionId, startupCommandFor(target, draft?.bookmark.startupCommand ?? null));
  for (const event of queued) applyEvent(sessionId, event);
  return sessionId;
}

export interface ProbeResult {
  ok: boolean;
  /** Why the test failed (null when cancelled or closed without an error). */
  error: AppError | null;
}

export interface Probe {
  result: Promise<ProbeResult>;
  /** Aborts the test (closes the probe session). */
  cancel(): void;
}

/**
 * "Test connection" (§5.8 `probeOnly`): connects, authenticates and closes. Prompts of the probe session are
 * shown by the normal prompt host, because the probe is registered as a hidden session (no tab, no workspace).
 */
export function startProbe(draft: DraftConnection, onSession?: (sessionId: string) => void): Probe {
  const backend = getBackend();
  let sessionId: string | null = null;
  const queued: SessionEvent[] = [];
  let settle: (r: ProbeResult) => void = () => undefined;
  const result = new Promise<ProbeResult>((resolve) => {
    settle = resolve;
  });
  let finished = false;

  const finish = (r: ProbeResult) => {
    if (finished) return;
    finished = true;
    settle(r);
    if (sessionId) {
      const id = sessionId;
      useStore.getState().removeSession(id);
      backend.sessionClose(id, null).catch(() => undefined);
    }
  };
  const handle = (id: string, event: SessionEvent) => {
    applyEvent(id, event);
    if (event.type !== "state") return;
    const phase = event.state.phase;
    if (phase === "connected") finish({ ok: true, error: null });
    else if (phase === "disconnected") finish({ ok: false, error: event.state.error });
    else if (phase === "closed") finish({ ok: false, error: null });
  };

  const request: Target = { kind: "draft", bookmark: draft.bookmark, password: draft.password };
  backend.openSession({ target: request, probeOnly: true }, (event) => {
    if (sessionId) handle(sessionId, event);
    else queued.push(event);
  }).then(
    (id) => {
      if (finished) {
        // Cancelled before `session_open` resolved: the backend session still exists and must be closed.
        backend.sessionClose(id, null).catch(() => undefined);
        return;
      }
      sessionId = id;
      useStore.getState().addSession(id, quickTarget(draftSpec(draft.bookmark)), draft.bookmark.name, true);
      onSession?.(id);
      for (const e of queued) handle(id, e);
    },
    (e: unknown) => {
      finish({ ok: false, error: reportableError(e) });
    },
  );

  return { result, cancel: () => finish({ ok: false, error: null }) };
}

function reportableError(e: unknown): AppError {
  const err = (e as { error?: AppError }).error;
  return err ?? { code: "internal" };
}

/** Asks the backend to reconnect now (waiting stage) or to retry a failed connection (`canRetry`). */
export async function reconnectNow(sessionId: string): Promise<void> {
  await getBackend().sessionReconnectNow(sessionId);
}

/**
 * Closes a session from the UI: with unsaved editor tabs the user is asked first (Save all / Discard / Cancel).
 * Plain `closeSession` is for flows that have already decided.
 */
export function requestCloseSession(sessionId: string): void {
  if (dirtyBuffers(sessionId).length > 0) {
    useStore.getState().openDialog(closeSessionDialog(sessionId));
    return;
  }
  // Running shells are lost with the connection: ask first (setting `general.confirmCloseWithTerminals`).
  if (confirmCloseWithTerminals() && runningTerminals(sessionId).length > 0) {
    useStore.getState().openDialog(confirmCloseTerminalsDialog(sessionId));
    return;
  }
  guard(closeSession(sessionId));
}

/** Closes a session: backend, terminals, buffers and store state. */
export async function closeSession(sessionId: string): Promise<void> {
  const store = useStore.getState();
  const ws = store.workspaces[sessionId];
  const lastDir = ws?.tree.root ?? null;
  disarmStartupCommand(sessionId);
  for (const termId of ws?.terminalTabs ?? []) disposeTerminalIfLoaded(termId, false);
  for (const bufferId of ws?.editorTabs ?? []) dropBuffer(bufferId);
  store.removeSession(sessionId);
  try {
    await getBackend().sessionClose(sessionId, lastDir);
  } catch {
    // session_close is idempotent and never fails meaningfully.
  }
}

/** Closes a session and opens its target again (a fresh connection that picks up the saved bookmark). */
export async function closeSessionThenReopen(sessionId: string): Promise<void> {
  const entry = useStore.getState().sessionsById[sessionId];
  if (!entry) return;
  const { target, label, tabName, instance } = entry;
  const { sessionOrder, sessionsById } = useStore.getState();
  const index = sessionOrder.indexOf(sessionId);
  // Decided before the close, while this session is still in the store: a duplicate tab to the same target (one that
  // exists now) must not be taken for the reopened connection, but an open that starts while `session_close` is
  // pending (e.g. a sidebar double click) is not a duplicate and must not be forced into a second tab.
  const hasDuplicate = sessionOrder.some((id) => {
    const other = sessionsById[id];
    return id !== sessionId && other !== undefined && sameTarget(other.target, target);
  });
  await closeSession(sessionId);
  // The tab keeps its place, its rename and its number (if still free). Without a duplicate the normal path applies:
  // the reopen joins an existing or in-flight open of the same target (then `preferredInstance` cannot apply, the
  // joined session already has its number) or registers itself in `pendingOpens` for later opens to join.
  const newId = await openSession(target, label, undefined, {
    forceNew: hasDuplicate,
    preferredInstance: instance,
  });
  const store = useStore.getState();
  if (index >= 0) store.placeSession(newId, index);
  if (tabName) store.renameSession(newId, tabName);
}

/** "Reconnect to apply changes": unsaved editor tabs are asked about first, like closing the connection. */
export function requestReconnectWithChanges(sessionId: string): void {
  if (dirtyBuffers(sessionId).length > 0) {
    useStore.getState().openDialog(closeSessionDialog(sessionId, true));
    return;
  }
  guard(closeSessionThenReopen(sessionId));
}

/**
 * Sends a prompt answer. Resolves `true` when the prompt is settled (answered, or stale = `promptNotFound`) and
 * `false` when sending failed for another reason: that error is reported and the dialog must re-enable itself.
 */
export async function answerPrompt(sessionId: string, promptId: string, answer: PromptAnswer): Promise<boolean> {
  try {
    await getBackend().sessionAnswerPrompt(sessionId, promptId, answer);
    return true;
  } catch (e) {
    if ((e as { error?: AppError }).error?.code === "promptNotFound") {
      // Only drop the prompt this answer was for: a late `promptNotFound` must not delete a newer retry prompt.
      if (useStore.getState().prompts[sessionId]?.id === promptId) useStore.getState().clearPrompt(sessionId);
      return true; // the prompt is gone either way
    }
    reportError(e);
    return false; // the dialog must become usable again
  }
}
