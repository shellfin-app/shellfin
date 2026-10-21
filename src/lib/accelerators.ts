// Browser accelerators the app does not use. The WebView would reload the UI (F5, Ctrl+R) while the backend
// sessions stay alive, print, view source, zoom, navigate back/forward, or open the browser find bar. They are
// cancelled in the bubble phase, and only when nothing else handled the key: a terminal that consumed it
// (Ctrl+R, Ctrl+P, Ctrl+U ... all belong to the shell) has already cancelled the event and stopped it, and the
// editor's own Ctrl+F / Ctrl+S keymap calls `preventDefault` itself.
import type { Platform } from "../ipc/types";
import { isMac, normKey, type KeyEventLike } from "./keybindings";

const FUNCTION_KEYS = new Set(["F3", "F5", "F7"]);
/** With the primary modifier: reload, print, view source, save, find, find next, open, history, downloads, bookmark. */
const MOD_LETTERS = new Set(["r", "p", "u", "s", "f", "g", "o", "h", "j", "d"]);
const ZOOM_KEYS = new Set(["=", "+", "-", "_", "0"]);
const NAV_KEYS = new Set(["ArrowLeft", "ArrowRight", "Home"]);

export function isBrowserAccelerator(e: KeyEventLike, platform: Platform): boolean {
  if (FUNCTION_KEYS.has(e.key)) return true;
  // Alt+Left / Alt+Right: history back / forward (Option+arrows move by word on macOS, so only Windows/Linux).
  if (e.altKey && !e.ctrlKey && !e.metaKey && !isMac(platform)) return NAV_KEYS.has(e.key);
  const mod = isMac(platform) ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
  if (!mod || e.altKey) return false;
  const key = normKey(e);
  return MOD_LETTERS.has(key) || ZOOM_KEYS.has(key);
}
