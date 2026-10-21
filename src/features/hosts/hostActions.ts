import { getBackend } from "../../ipc";
import { toIpcError } from "../../ipc/call";
import type { Bookmark, BookmarkInput } from "../../ipc/types";
import { NEW_GROUP, toBookmarkInput, type BookmarkFormState } from "../../lib/bookmarkForm";
import { bookmarkTarget, quickTarget } from "../../lib/targets";
import { useStore } from "../../state/store";
import { draftSpec, openSession } from "../session/sessionActions";
import { bookmarksOfGroup } from "./bookmarkOrder";

export async function refreshBookmarks(): Promise<void> {
  useStore.getState().setBookmarks(await getBackend().bookmarksGet());
}

export async function deleteBookmark(id: string): Promise<void> {
  await getBackend().bookmarkDelete(id);
  await refreshBookmarks();
}

/**
 * Deletes a group. The backend turns its bookmarks into ungrouped ones; for another target group they are
 * moved there first (appended, in order), so a failure half-way leaves no bookmark lost.
 */
export async function deleteGroup(id: string, moveBookmarksTo: string | null): Promise<void> {
  const backend = getBackend();
  try {
    if (moveBookmarksTo !== null) {
      const dto = useStore.getState().bookmarks;
      const moving = dto ? bookmarksOfGroup(dto, id) : [];
      for (const b of moving) {
        const latest = useStore.getState().bookmarks;
        const size = latest ? bookmarksOfGroup(latest, moveBookmarksTo).length : 0;
        useStore.getState().setBookmarks(await backend.bookmarkMove(b.id, moveBookmarksTo, size));
      }
    }
    await backend.groupDelete(id);
  } finally {
    await refreshBookmarks();
  }
}

export async function copyFromSshConfig(alias: string): Promise<Bookmark> {
  const bookmark = await getBackend().bookmarkCopyFromSshConfig(alias);
  await refreshBookmarks();
  return bookmark;
}

export type SubmitIntent = "connect" | "save";

/** Everything `submitBookmarkForm` needs to know about the edit in progress. */
export interface SubmitContext {
  /** Bookmark being edited (null = new). */
  editing: Bookmark | null;
  intent: SubmitIntent;
  /**
   * Called right after the bookmark (and an inline new group) were saved, BEFORE connecting. If the connect then
   * fails, the dialog is already in edit mode with the saved ids, so a retry updates instead of duplicating.
   */
  onSaved?: (saved: Bookmark, groupId: string | null) => void;
  /** Runs right before the session is opened (e.g. closes the failed tab this connection replaces). */
  beforeConnect?: () => Promise<void>;
}

/**
 * Save and/or connect from the bookmark dialog:
 *  - creates a new group when asked, saves the bookmark (when "Save as bookmark" is on or intent is save),
 *    stores/removes the keychain password;
 *  - for `connect`, opens the session. A password that is not in the keychain travels only inside the
 *    `session_open` draft request, never into the store (§9.2).
 */
export async function submitBookmarkForm(form: BookmarkFormState, ctx: SubmitContext): Promise<void> {
  const backend = getBackend();
  const persist = form.saveAsBookmark || ctx.intent === "save";
  // Groups belong to saved bookmarks: connect-only never creates one (no orphan groups, nothing written to a read-only store).
  let groupId: string | null = !persist || form.groupId === "" || form.groupId === NEW_GROUP ? null : form.groupId;
  if (persist && form.groupId === NEW_GROUP) {
    const groups = useStore.getState().bookmarks?.groups ?? [];
    const group = await backend.groupSave({ id: null, name: form.newGroupName.trim(), order: groups.length });
    groupId = group.id;
  }

  const typedPassword = form.auth === "password" && form.password !== "" ? form.password : null;
  let saved: Bookmark | null = null;
  let inKeychain = false;

  if (persist) {
    const input = toBookmarkInput(form, ctx.editing?.id ?? null, groupId);
    saved = await backend.bookmarkSave(input);
    if (form.auth === "password") {
      // `connection.rememberPasswords = false`: the "save" option is hidden and a new password is never stored.
      const storing = useStore.getState().settings?.connection.rememberPasswords !== false;
      if (typedPassword !== null && storing && form.savePassword) {
        try {
          await backend.bookmarkSetPassword(saved.id, typedPassword);
          inKeychain = true;
        } catch (e) {
          // The setting was turned off elsewhere in the meantime: connect with the typed password instead.
          if (toIpcError(e).error.code !== "secretStorageDisabled") throw e;
        }
      } else if (storing && !form.savePassword && ctx.editing?.auth.method === "password" && ctx.editing.auth.passwordSaved) {
        await backend.bookmarkSetPassword(saved.id, null);
      } else if (saved.auth.method === "password") {
        inKeychain = saved.auth.passwordSaved && typedPassword === null;
      }
    }
    await refreshBookmarks();
    ctx.onSaved?.(saved, groupId);
  }

  if (ctx.intent !== "connect") return;
  await ctx.beforeConnect?.();

  const input: BookmarkInput = toBookmarkInput(form, saved?.id ?? null, groupId);
  const label = input.name;
  if (saved) {
    const needsDraft = typedPassword !== null && !inKeychain;
    await openSession(
      bookmarkTarget(saved.id),
      label,
      needsDraft ? { bookmark: { ...input, id: saved.id }, password: typedPassword } : undefined,
    );
  } else {
    await openSession(quickTarget(draftSpec(input)), label, { bookmark: input, password: typedPassword });
  }
}

/** Persists a move (drag and drop, "Move up/down", "Move to group…"); the backend answers with the whole new list. */
export async function moveBookmark(id: string, groupId: string | null, index: number): Promise<void> {
  useStore.getState().setBookmarks(await getBackend().bookmarkMove(id, groupId, index));
}

export async function moveGroup(id: string, index: number): Promise<void> {
  useStore.getState().setBookmarks(await getBackend().groupMove(id, index));
}
