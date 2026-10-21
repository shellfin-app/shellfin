import { useTranslation } from "react-i18next";
import { IconButton } from "../../components/IconButton";
import { dirname } from "../../lib/remotePath";
import { guard } from "../../lib/reportError";
import { useStore } from "../../state/store";
import { uploadFiles } from "../transfers/transferActions";
import { goUp, isConnected, isDirLike, lookupEntry, refreshDirectory, startNewEntry } from "./fileActions";

/** New file / New folder / Upload / Refresh for the current folder (the selected folder, or the folder of the selected file). */
export function FilesToolbar({ sessionId }: { sessionId: string }) {
  const { t } = useTranslation();
  const tree = useStore((s) => s.workspaces[sessionId]?.tree);
  const online = useStore((s) => s.sessionsById[sessionId]?.state.phase === "connected");
  const sftp = useStore((s) => s.sessionsById[sessionId]?.info?.sftpAvailable ?? false);
  if (!tree || tree.root === null) return null;

  const root = tree.root;
  const selected = tree.selection !== null ? lookupEntry(tree, tree.selection) : undefined;
  const target = selected && !selected.lossyName ? (isDirLike(selected) ? selected.path : dirname(selected.path)) : root;
  const enabled = online && sftp;
  const ready = () => isConnected(sessionId);

  return (
    <div className="files-toolbar" role="toolbar" aria-label={t("files.toolbar")}>
      <IconButton icon="arrowUp" iconSize={14} size={24} label={t("files.up")} disabled={!enabled || root === "/"} onClick={() => ready() && guard(goUp(sessionId))} />
      <IconButton icon="filePlus" iconSize={14} size={24} label={t("files.newFile")} disabled={!enabled} onClick={() => ready() && startNewEntry(sessionId, target, "file")} />
      <IconButton icon="folderPlus" iconSize={14} size={24} label={t("files.newFolder")} disabled={!enabled} onClick={() => ready() && startNewEntry(sessionId, target, "dir")} />
      <IconButton icon="upload" iconSize={14} size={24} label={t("files.upload")} disabled={!enabled} onClick={() => guard(uploadFiles(sessionId, target))} />
      <IconButton icon="refresh" iconSize={14} size={24} label={t("files.refresh")} disabled={!enabled} onClick={() => guard(refreshDirectory(sessionId, target))} />
    </div>
  );
}
