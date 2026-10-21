// Keyboard shortcuts (docs/ARCHITECTURE.md §8.7). `Mod` = Ctrl on Windows/Linux, Cmd on macOS.
import type { Platform } from "../ipc/types";

export type ShortcutId =
  | "palette"
  | "paletteInTerminal"
  | "newConnection"
  | "toggleSidebar"
  | "toggleTerminalPanel"
  | "newTerminal"
  | "terminalFind"
  | "save"
  | "saveAll"
  | "closeTab"
  | "nextTab"
  | "prevTab"
  // Menu-bar shortcuts and focus movement (NOT in a focused terminal: it owns those keys, see `matchShortcut`).
  | "settings"
  | "fullScreen"
  | "zoomIn"
  | "zoomOut"
  | "zoomReset"
  | "focusFiles"
  | "focusEditor"
  | "focusTerminal"
  | "focusNextRegion"
  | "focusPrevRegion";

export interface KeyEventLike {
  key: string;
  /** Physical key (used where Option changes `key` on macOS). */
  code?: string;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
  /** Present on real KeyboardEvents; AltGr (Ctrl+Alt on Windows) is detected through it. */
  getModifierState?(key: "AltGraph"): boolean;
}

/**
 * Layout-independent key name for shortcut matching (as VS Code and browsers do): a single Latin letter or digit in
 * `event.key` wins (keeps AZERTY/Dvorak semantics); any other key (a Cyrillic letter, say) falls back to the physical
 * `event.code` (`KeyS` -> "s", `Digit1` -> "1"). With AltGr held the key is a typed character (AltGr+S on Polish is
 * "ś"), so it is never mapped to the physical key.
 */
export function normKey(e: KeyEventLike): string {
  const key = e.key.toLowerCase();
  if (/^[a-z0-9]$/.test(key)) return key;
  if (e.getModifierState?.("AltGraph") === true) return key;
  const m = /^(?:Key([A-Z])|Digit([0-9]))$/.exec(e.code ?? "");
  if (m) return (m[1] ?? m[2] ?? key).toLowerCase();
  // Punctuation shortcuts (Mod+, Mod+= Mod+-): on a Cyrillic layout the physical Comma key types "б", so the physical
  // key decides when `key` is not printable ASCII.
  const punctuation = e.code === undefined ? undefined : PUNCTUATION_CODES[e.code];
  return punctuation !== undefined && !/^[ -~]$/.test(e.key) ? punctuation : key;
}

const PUNCTUATION_CODES: Readonly<Record<string, string>> = {
  Comma: ",",
  Period: ".",
  Minus: "-",
  Equal: "=",
  Slash: "/",
  Backquote: "`",
};

/** Backquote key: `key` on Latin layouts, the physical key where the layout types something else (Cyrillic "ґ"). */
function isBackquote(e: KeyEventLike): boolean {
  return e.key === "`" || e.key === "~" || (e.code === "Backquote" && !/^[!-~]$/.test(e.key));
}
export function isMac(platform: Platform): boolean {
  return platform === "macos";
}

/** True when the event carries the platform's primary modifier ("Mod"). */
function mod(e: KeyEventLike, platform: Platform): boolean {
  return isMac(platform) ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
}

/**
 * Matches an app shortcut. `inTerminal` follows §8.7: plain Ctrl+K goes to the shell on Windows/Linux, so the
 * palette is Ctrl+Shift+K there; Cmd+K reaches no shell on macOS.
 */
export function matchShortcut(e: KeyEventLike, platform: Platform, inTerminal: boolean): ShortcutId | null {
  const key = normKey(e);
  if (key === "k" && !e.altKey) {
    if (inTerminal && !isMac(platform)) {
      return mod(e, platform) && e.shiftKey ? "paletteInTerminal" : null;
    }
    if (mod(e, platform) && !e.shiftKey) return "palette";
  }
  // Terminal panel shortcuts work everywhere, also with the terminal focused (§8.7 "always").
  if (e.ctrlKey && !e.altKey && !e.metaKey) {
    if (!e.shiftKey && isBackquote(e)) return "toggleTerminalPanel";
    if (e.shiftKey && isBackquote(e)) return "newTerminal";
  }
  // From here on the terminal owns every key (F6 is mc/htop/nano, Ctrl+_ undoes in readline, Ctrl+= and Ctrl+, are
  // program keys): the menu bar and the registry still offer these actions.
  if (inTerminal) {
    // Find: plain Ctrl+F is readline's forward-char, so Win/Linux use Ctrl+Shift+F; Cmd+F reaches no shell.
    if (key === "f" && !e.altKey) {
      if (isMac(platform) ? e.metaKey && !e.ctrlKey && !e.shiftKey : e.ctrlKey && !e.metaKey && e.shiftKey) {
        return "terminalFind";
      }
    }
    // Cmd chords reach no shell on macOS, so those stay app shortcuts (handled below).
    if (!(isMac(platform) && e.metaKey)) return null;
  }
  // F6 / Shift+F6 move the focus between sidebar, editor and terminal; F11 is full screen.
  if (!inTerminal && !e.ctrlKey && !e.altKey && !e.metaKey) {
    if (e.key === "F6") return e.shiftKey ? "focusPrevRegion" : "focusNextRegion";
    if (e.key === "F11" && !e.shiftKey) return "fullScreen";
  }
  if (mod(e, platform) && !e.altKey) {
    // Zoom (terminal + editor font): Mod+= / Mod+- / Mod+0; Shift only changes what the key types ("+", "_").
    if (key === "=" || key === "+") return "zoomIn";
    if (key === "-" || key === "_") return "zoomOut";
    if (key === "0" && !e.shiftKey) return "zoomReset";
    if (key === "," && !e.shiftKey) return "settings";
    if (e.shiftKey && key === "e") return "focusFiles";
    if (e.shiftKey && key === "y") return "focusEditor";
    if (e.shiftKey && key === "j") return "focusTerminal";
  }
  if (inTerminal) return null;
  if (mod(e, platform) && !e.shiftKey && !e.altKey) {
    if (key === "n") return "newConnection";
    if (key === "b") return "toggleSidebar";
    if (key === "s") return "save";
    if (key === "w") return "closeTab";
  }
  // Save all: Mod+Alt+S. On macOS Option+S types a different character, so the physical key decides; on Windows the
  // key must really be "s" (AltGr+S is a letter on several layouts and reports Ctrl+Alt).
  if (mod(e, platform) && !e.shiftKey && e.altKey && (isMac(platform) ? e.code === "KeyS" : key === "s")) return "saveAll";
  if (e.ctrlKey && !e.metaKey && !e.altKey && e.key === "Tab") return e.shiftKey ? "prevTab" : "nextTab";
  return null;
}

/**
 * Terminal clipboard keys (§8.6/§8.7): Ctrl+Shift+C / Ctrl+Shift+V and Shift+Insert on Windows/Linux (plain
 * Ctrl+C/V belong to the remote), Cmd+C / Cmd+V on macOS. Paste is the native `paste` event (sanitised by the
 * single capture-phase listener), so the registry only lets the browser's default action run for it.
 */
export function terminalClipboardKey(e: KeyEventLike, platform: Platform): "copy" | "paste" | null {
  const key = normKey(e);
  if (isMac(platform)) {
    if (e.metaKey && !e.ctrlKey && !e.shiftKey && !e.altKey) return key === "c" ? "copy" : key === "v" ? "paste" : null;
    return null;
  }
  if (e.ctrlKey && e.shiftKey && !e.altKey && !e.metaKey) return key === "c" ? "copy" : key === "v" ? "paste" : null;
  if (e.shiftKey && !e.ctrlKey && !e.altKey && !e.metaKey && e.key === "Insert") return "paste";
  return null;
}

/** `Mod+1..9` -> zero-based connection tab index (works everywhere, also inside the terminal; §8.7). */
export function connectionTabIndex(e: KeyEventLike, platform: Platform): number | null {
  if (!mod(e, platform) || e.shiftKey || e.altKey) return null;
  const key = normKey(e);
  return /^[1-9]$/.test(key) ? Number(key) - 1 : null;
}

/** Display text of a key event ("Ctrl+B", "⌘B") for hints that name the keys the user just pressed. */
export function describeKeyEvent(e: KeyEventLike, platform: Platform): string {
  const key = normKey(e);
  const name = key.length === 1 ? key.toUpperCase() : e.key;
  if (isMac(platform)) {
    return `${e.ctrlKey ? "⌃" : ""}${e.altKey ? "⌥" : ""}${e.shiftKey ? "⇧" : ""}${e.metaKey ? "⌘" : ""}${name}`;
  }
  return [e.ctrlKey ? "Ctrl" : "", e.altKey ? "Alt" : "", e.shiftKey ? "Shift" : "", e.metaKey ? "Meta" : "", name]
    .filter(Boolean)
    .join("+");
}
