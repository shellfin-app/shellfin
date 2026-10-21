import { useTranslation } from "react-i18next";
import type { BookmarkColor } from "../ipc/types";
import { colorVar } from "../lib/bookmarkColor";
import "./components.css";

/** A small colored square for a bookmark color. Color is never the only signal: the translated name is the label. */
export function ColorMark({ color }: { color: BookmarkColor | null }) {
  const { t } = useTranslation();
  if (color === null) return null;
  const name = t(`bookmarkColor.${color}`);
  return <span className="color-mark" style={{ background: colorVar(color) }} role="img" aria-label={name} title={name} />;
}
