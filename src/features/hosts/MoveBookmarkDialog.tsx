import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { Dialog } from "../../components/Dialog";
import { Field, Select } from "../../components/Field";
import { Icon } from "../../components/Icon";
import { DIALOG_KIND } from "../../lib/dialogs";
import { displayName } from "../../lib/serverText";
import { guard } from "../../lib/reportError";
import { useStore } from "../../state/store";
import { bookmarksOfGroup, sortedGroups } from "./bookmarkOrder";
import { moveBookmark } from "./hostActions";

/** "Move to group…": the keyboard/menu alternative to dragging a bookmark into another group (appended at the end). */
export function MoveBookmarkDialog({ bookmarkId }: { bookmarkId: string }) {
  const { t } = useTranslation();
  const dto = useStore((s) => s.bookmarks);
  const closeDialog = useStore((s) => s.closeDialog);
  const bookmark = dto?.bookmarks.find((b) => b.id === bookmarkId);
  const [target, setTarget] = useState<string>(bookmark?.groupId ?? "");
  const close = () => closeDialog(DIALOG_KIND.moveBookmark);
  if (!dto || !bookmark) return null;
  const current = bookmark.groupId ?? "";

  return (
    <Dialog
      title={t("hosts.move.title", { name: displayName(bookmark.name) })}
      width={440}
      icon={<Icon name="folder" size={20} strokeWidth={1.8} />}
      onClose={close}
      footer={
        <>
          <Button onClick={close}>{t("common.cancel")}</Button>
          <Button
            variant="primary"
            disabled={target === current}
            onClick={() => {
              const groupId = target === "" ? null : target;
              close();
              guard(moveBookmark(bookmarkId, groupId, bookmarksOfGroup(dto, groupId).length));
            }}
          >
            {t("hosts.move.action")}
          </Button>
        </>
      }
    >
      <Field label={t("connect.group")}>
        <Select value={target} onChange={(e) => setTarget(e.target.value)} data-autofocus>
          <option value="">{t("connect.noGroup")}</option>
          {sortedGroups(dto).map((g) => (
            <option key={g.id} value={g.id}>
              {g.name}
            </option>
          ))}
        </Select>
      </Field>
    </Dialog>
  );
}
