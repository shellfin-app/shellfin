import { Fragment } from "react";
import { useTranslation } from "react-i18next";
import { guard } from "../../lib/reportError";
import { segments } from "../../lib/remotePath";
import { displayName, displayPath } from "../../lib/serverText";
import { useStore } from "../../state/store";
import { changeRoot, isConnected } from "./fileActions";

/** Clickable path of the tree root: each segment re-roots the tree at that folder. */
export function FilesBreadcrumb({ sessionId, root }: { sessionId: string; root: string }) {
  const { t } = useTranslation();
  const online = useStore((s) => s.sessionsById[sessionId]?.state.phase === "connected");
  const parts = segments(root);
  const crumbs = [{ name: "/", path: "/" }];
  let acc = "";
  for (const part of parts) {
    acc = `${acc}/${part}`;
    crumbs.push({ name: displayName(part), path: acc });
  }
  return (
    <nav className="breadcrumb mono" aria-label={t("files.breadcrumb")} title={displayPath(root)}>
      {crumbs.map((c, i) => {
        const current = c.path === root;
        return (
          <Fragment key={c.path}>
            {i > 1 ? <span className="breadcrumb__sep" aria-hidden="true" /> : null}
            <button
              type="button"
              className="breadcrumb__seg"
              aria-current={current ? "location" : undefined}
              disabled={!online || current}
              onClick={() => isConnected(sessionId) && guard(changeRoot(sessionId, c.path))}
            >
              <bdi>{c.name}</bdi>
            </button>
          </Fragment>
        );
      })}
    </nav>
  );
}
