// SVG path data for the UI icons (24x24 viewBox, stroke icons unless noted). Geometry only — no user-facing text.
// Kept outside src/components so the no-literal-string lint does not have to look at path data.

export const ICON_VIEWBOX = "0 0 24 24";

export type IconName =
  | "terminal"
  | "plus"
  | "close"
  | "search"
  | "servers"
  | "folder"
  | "file"
  | "chevronDown"
  | "chevronRight"
  | "star"
  | "key"
  | "settings"
  | "portForward"
  | "home"
  | "refresh"
  | "arrowUp"
  | "split"
  | "edit"
  | "trash"
  | "copy"
  | "check"
  | "shield"
  | "wifiOff"
  | "upload"
  | "download"
  | "filePlus"
  | "folderPlus"
  | "clock"
  | "alert"
  | "transfers"
  | "lock"
  | "eye"
  | "eyeOff"
  | "plug"
  | "loader"
  | "winMinimize"
  | "winMaximize"
  | "winRestore"
  | "chevronUp"
  | "panelLeft"
  | "panelBottom"
  | "panes"
  | "more"
  | "menu"
  | "chevronLeft"
  | "help";

export interface IconDef {
  /** Path `d` attributes. */
  paths: string[];
  /** Extra shapes. */
  circles?: { cx: number; cy: number; r: number }[];
  rects?: { x: number; y: number; width: number; height: number; rx: number }[];
}

export const ICONS: Record<IconName, IconDef> = {
  terminal: { paths: ["M4 17l6-5-6-5", "M12 19h8"] },
  plus: { paths: ["M12 5v14M5 12h14"] },
  close: { paths: ["M6 6l12 12M18 6L6 18"] },
  search: { paths: ["M20 20l-4-4"], circles: [{ cx: 11, cy: 11, r: 7 }] },
  servers: {
    paths: ["M7 7.5h.01M7 16.5h.01"],
    rects: [
      { x: 3, y: 4, width: 18, height: 7, rx: 2 },
      { x: 3, y: 13, width: 18, height: 7, rx: 2 },
    ],
  },
  folder: { paths: ["M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"] },
  file: { paths: ["M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z", "M14 3v5h5"] },
  chevronDown: { paths: ["M6 9l6 6 6-6"] },
  chevronRight: { paths: ["M9 6l6 6-6 6"] },
  chevronLeft: { paths: ["M15 6l-6 6 6 6"] },
  menu: { paths: ["M4 6h16M4 12h16M4 18h16"] },
  star: { paths: ["M12 3l2.8 5.7 6.2.9-4.5 4.4 1 6.2L12 17.3 6.5 20.2l1-6.2L3 9.6l6.2-.9z"] },
  key: { paths: ["M11 12l9-9M17 6l3 3"], circles: [{ cx: 8, cy: 15, r: 4 }] },
  settings: {
    paths: ["M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9L7 7M17 17l2.1 2.1M4.9 19.1L7 17M17 7l2.1-2.1"],
    circles: [{ cx: 12, cy: 12, r: 3 }],
  },
  portForward: { paths: ["M4 8h14l-3-3M20 16H6l3 3"] },
  home: { paths: ["M3 11l9-7 9 7", "M5 10v10h14V10"] },
  arrowUp: { paths: ["M12 19V5M6 11l6-6 6 6"] },
  refresh: { paths: ["M20 11a8 8 0 1 0-2.3 5.7", "M20 5v6h-6"] },
  split: { paths: ["M12 4v16"], rects: [{ x: 3, y: 4, width: 18, height: 16, rx: 2 }] },
  edit: { paths: ["M4 20h4L19 9l-4-4L4 16z", "M13.5 6.5l4 4"] },
  trash: { paths: ["M4 7h16M10 11v6M14 11v6", "M6 7l1 13h10l1-13", "M9 7V4h6v3"] },
  copy: { paths: ["M5 15V5a2 2 0 0 1 2-2h8"], rects: [{ x: 9, y: 7, width: 12, height: 14, rx: 2 }] },
  check: { paths: ["M5 12l4 4 10-10"] },
  shield: { paths: ["M12 3l8 3v6c0 4.5-3.4 8.3-8 9-4.6-.7-8-4.5-8-9V6z", "M12 8v5M12 16h.01"] },
  upload: { paths: ["M12 16V4M7 9l5-5 5 5", "M4 20h16"] },
  download: { paths: ["M12 4v12M7 11l5 5 5-5", "M4 20h16"] },
  filePlus: { paths: ["M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z", "M14 3v5h5", "M12 12v6M9 15h6"] },
  folderPlus: {
    paths: ["M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z", "M12 11v6M9 14h6"],
  },
  clock: { paths: ["M12 7v5l3 2"], circles: [{ cx: 12, cy: 12, r: 9 }] },
  alert: { paths: ["M12 4l9 16H3z", "M12 10v4M12 17h.01"] },
  transfers: { paths: ["M7 4v14M3 14l4 4 4-4", "M17 20V6M13 10l4-4 4 4"] },
  wifiOff: { paths: ["M2 8.8a15 15 0 0 1 20 0M5 12.6a10 10 0 0 1 14 0M8.5 16.4a5 5 0 0 1 7 0M12 20h.01", "M3 3l18 18"] },
  lock: { paths: ["M8 11V8a4 4 0 0 1 8 0v3"], rects: [{ x: 5, y: 11, width: 14, height: 10, rx: 2 }] },
  eye: { paths: ["M2 12s3.6-7 10-7 10 7 10 7-3.6 7-10 7S2 12 2 12z"], circles: [{ cx: 12, cy: 12, r: 3 }] },
  eyeOff: { paths: ["M3 3l18 18", "M10.6 6.1A9.7 9.7 0 0 1 12 6c6.4 0 10 6 10 6a17 17 0 0 1-3.2 3.9M6.6 7.6A17 17 0 0 0 2 12s3.6 6 10 6a9.6 9.6 0 0 0 4-.9"] },
  plug: { paths: ["M9 3v5M15 3v5M6 8h12v3a6 6 0 0 1-12 0z", "M12 17v4"] },
  loader: { paths: ["M21 12a9 9 0 1 1-6.2-8.55"] },
  winMinimize: { paths: ["M6 12h12"] },
  winMaximize: { paths: [], rects: [{ x: 6, y: 6, width: 12, height: 12, rx: 1 }] },
  winRestore: { paths: ["M9 9V6h9v9h-3"], rects: [{ x: 6, y: 9, width: 9, height: 9, rx: 1 }] },
  chevronUp: { paths: ["M6 15l6-6 6 6"] },
  panelLeft: { paths: ["M9 4v16"], rects: [{ x: 3, y: 4, width: 18, height: 16, rx: 2 }] },
  panelBottom: { paths: ["M3 14h18"], rects: [{ x: 3, y: 4, width: 18, height: 16, rx: 2 }] },
  panes: { paths: ["M4 12h16"], rects: [{ x: 4, y: 4, width: 16, height: 16, rx: 2 }] },
  more: { paths: ["M5 12h.01M12 12h.01M19 12h.01"] },
  help: { paths: ["M9.5 9.5a2.5 2.5 0 1 1 3.5 2.3c-.7.4-1 .9-1 1.7", "M12 17h.01"], circles: [{ cx: 12, cy: 12, r: 9 }] },
};
