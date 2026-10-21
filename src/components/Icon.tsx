import { ICONS, ICON_VIEWBOX, type IconName } from "../lib/iconPaths";

export interface IconProps {
  name: IconName;
  size?: number;
  /** Stroke width; the mockups use 1.8 for large icons and 2 for small ones. */
  strokeWidth?: number;
  /** Fill the shape with the current color (favorite star). */
  filled?: boolean;
  className?: string;
}

/** Decorative inline SVG icon (always `aria-hidden`; the owning button carries the accessible name). */
export function Icon({ name, size = 16, strokeWidth = 2, filled = false, className }: IconProps) {
  const def = ICONS[name];
  return (
    <svg
      width={size}
      height={size}
      viewBox={ICON_VIEWBOX}
      fill={filled ? "currentColor" : "none"}
      stroke="currentColor"
      strokeWidth={strokeWidth}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
      className={className}
    >
      {def.rects?.map((r) => <rect key={`${r.x}-${r.y}`} x={r.x} y={r.y} width={r.width} height={r.height} rx={r.rx} />)}
      {def.circles?.map((c) => <circle key={`${c.cx}-${c.cy}`} cx={c.cx} cy={c.cy} r={c.r} />)}
      {def.paths.map((d) => <path key={d} d={d} />)}
    </svg>
  );
}
