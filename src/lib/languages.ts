// Editor language table and status-bar display names. Technical names (YAML, UTF-8, LF ...) are data, not UI
// copy, so they live outside the no-literal-string lint scope (docs/ARCHITECTURE.md §8.2.2).
// Light on purpose (the status bar uses it): the CodeMirror parsers are in `features/editor/languageExtension.ts`,
// which loads with the editor.
import type { Eol } from "../ipc/types";
import { basename } from "./remotePath";

export interface LanguageInfo {
  /** Stable id; `languageExtension` maps it to a parser. */
  id: "plain" | "yaml" | "shell" | "nginx" | "properties";
  /** Technical display name (YAML, Shell ...), not translated; `null` = plain text (UI shows `editor.language.plain`). */
  label: string | null;
}

const PLAIN: LanguageInfo = { id: "plain", label: null };
const YAML: LanguageInfo = { id: "yaml", label: "YAML" };
const SHELL: LanguageInfo = { id: "shell", label: "Shell" };
const NGINX: LanguageInfo = { id: "nginx", label: "Nginx" };
const PROPERTIES: LanguageInfo = { id: "properties", label: "Properties" };

const BY_EXTENSION: Record<string, LanguageInfo> = {
  yml: YAML,
  yaml: YAML,
  sh: SHELL,
  bash: SHELL,
  conf: NGINX,
  env: PROPERTIES,
  properties: PROPERTIES,
};

export function languageForPath(path: string): LanguageInfo {
  const name = basename(path).toLowerCase();
  if (name.startsWith(".env")) return PROPERTIES;
  const dot = name.lastIndexOf(".");
  const ext = dot >= 0 ? name.slice(dot + 1) : "";
  return BY_EXTENSION[ext] ?? PLAIN;
}

export function encodingLabel(encoding: string, hasBom = false): string {
  return hasBom ? `${encoding.toUpperCase()} BOM` : encoding.toUpperCase();
}

export function eolLabel(eol: Eol): string {
  switch (eol) {
    case "lf":
      return "LF";
    case "crlf":
      return "CRLF";
    case "cr":
      return "CR";
  }
}

/** Encodings offered by "Reopen with encoding…" / "Save with encoding…" (§6.3). WHATWG names as the backend returns them: iso-8859-1 is windows-1252 there, so it is not offered twice. Labels are technical names. */
export const ENCODINGS = ["utf-8", "utf-16le", "utf-16be", "windows-1251", "koi8-u", "windows-1252"] as const;
