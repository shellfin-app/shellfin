import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../../components/Button";
import { Dialog } from "../../components/Dialog";
import { Field, Select } from "../../components/Field";
import { Icon } from "../../components/Icon";
import { DIALOG_KIND } from "../../lib/dialogs";
import { guard } from "../../lib/reportError";
import { displayName } from "../../lib/serverText";
import { useStore } from "../../state/store";
import { bookmarksOfGroup, sortedGroups } from "./bookmarkOrder";
import { deleteGroup } from "./hostActions";

/**
 * "Delete group…". An empty group needs a plain confirmation; a group with bookmarks asks where they go (Ungrouped by
 * default), so deleting a group never deletes bookmarks.
 */
export function DeleteGroupDialog({ groupId }: { groupId: string }) {
  const { t } = useTranslation();
  const dto = useStore((s) => s.bookmarks);
  const closeDialog = useStore((s) => s.closeDialog);
  const [target, setTarget] = useState("");
  const close = () => closeDialog(DIALOG_KIND.deleteGroup);
  const group = dto?.groups.find((g) => g.id === groupId);
  if (!dto || !group) return null;
  const count = bookmarksOfGroup(dto, groupId).length;
  const name = displayName(group.name);

  return (
    <Dialog
      title={t("hosts.group.deleteTitle")}
      tone="danger"
      role="alertdialog"
      width={460}
      icon={<Icon name="trash" size={20} strokeWidth={1.8} />}
      onClose={close}
      footer={
        <>
          <Button onClick={close} data-autofocus={count === 0 ? "" : undefined}>
            {t("common.cancel")}
          </Button>
          <Button
            variant="primary"
            onClick={() => {
              close();
              guard(deleteGroup(groupId, count === 0 || target === "" ? null : target));
            }}
          >
            {count === 0 ? t("common.delete") : t("hosts.group.deleteAndMove")}
          </Button>
        </>
      }
    >
      {count === 0 ? (
        <p className="dialog__text">{t("hosts.group.deleteEmptyBody", { name })}</p>
      ) : (
        <>
          <p className="dialog__text">{t("hosts.group.deleteBody", { name, count })}</p>
          <Field label={t("hosts.group.moveTo")}>
            <Select value={target} onChange={(e) => setTarget(e.target.value)} data-autofocus>
              <option value="">{t("connect.noGroup")}</option>
              {sortedGroups(dto)
                .filter((g) => g.id !== groupId)
                .map((g) => (
                  <option key={g.id} value={g.id}>
                    {g.name}
                  </option>
                ))}
            </Select>
          </Field>
        </>
      )}
    </Dialog>
  );
}
