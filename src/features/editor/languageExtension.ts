// CodeMirror language support for the table in `lib/languages.ts`. One `StreamLanguage` per parser, shared by every
// buffer of that language: `StreamLanguage.define` builds a parser and a node set each time.
import { StreamLanguage, type StreamParser } from "@codemirror/language";
import type { Extension } from "@codemirror/state";
import { nginx } from "@codemirror/legacy-modes/mode/nginx";
import { properties } from "@codemirror/legacy-modes/mode/properties";
import { shell } from "@codemirror/legacy-modes/mode/shell";
import { yaml } from "@codemirror/legacy-modes/mode/yaml";
import type { LanguageInfo } from "../../lib/languages";

const PARSERS: Record<Exclude<LanguageInfo["id"], "plain">, StreamParser<unknown>> = {
  yaml,
  shell,
  nginx,
  properties,
};

const defined = new Map<LanguageInfo["id"], Extension>();

export function languageExtension(info: LanguageInfo): Extension {
  if (info.id === "plain") return [];
  let extension = defined.get(info.id);
  if (extension === undefined) {
    extension = StreamLanguage.define(PARSERS[info.id]);
    defined.set(info.id, extension);
  }
  return extension;
}
