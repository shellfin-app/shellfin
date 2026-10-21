// Pure logic of the bookmark / new-connection dialog (docs/ARCHITECTURE.md §4.4 BookmarkInput).
import type { AuthInput, Bookmark, BookmarkColor, BookmarkInput, JumpRef } from "../ipc/types";
import { fieldFromKiBps, kibpsFromField, type BandwidthField } from "./bandwidth";
import { parseHostSpec } from "./hostSpec";

export type AuthMode = "auto" | "key" | "password" | "agent";

/** What the footer buttons of the bookmark dialog do: connect without keeping it, save only, or both. */
export type SubmitAction = "connect" | "save" | "saveConnect";
export const SUBMIT_ACTION = { connect: "connect", save: "save", saveConnect: "saveConnect" } as const satisfies Record<SubmitAction, SubmitAction>;

/** Per-bookmark transfer speed: the global setting, no limit, or a limit of its own. */
export type LimitMode = "global" | "unlimited" | "limit";

/** Sentinel group value: "create a new group" (its name is typed in `newGroupName`). */
export const NEW_GROUP = "__new__";

export interface BookmarkFormState {
  name: string;
  host: string;
  port: string;
  user: string;
  auth: AuthMode;
  identityFile: string;
  /** Typed password: lives ONLY in the dialog's local state, never in the store (§9.2). */
  password: string;
  savePassword: boolean;
  /** "" = none, otherwise `ssh:<alias>` / `bm:<id>` / `spec:<spec>` (see encodeJump). */
  proxyJump: string;
  /** "" = no group, NEW_GROUP, or a group id. */
  groupId: string;
  newGroupName: string;
  /** "" = no color. */
  color: BookmarkColor | "";
  startDir: string;
  saveAsBookmark: boolean;
  uploadMode: LimitMode;
  uploadLimit: BandwidthField;
  downloadMode: LimitMode;
  downloadLimit: BandwidthField;
  /** "Confirm before connecting". */
  confirmConnect: boolean;
  /** One line, sent to the first terminal after connecting (never logged). */
  startupCommand: string;
  /** Plain text. */
  notes: string;
}

export const STARTUP_COMMAND_MAX = 1024;
export const NOTES_MAX = 4000;

/** One line: no control character at all (tab, CR, LF, DEL, C1 included) and no U+2028 / U+2029. */
export function validStartupCommand(text: string): boolean {
  if ([...text].length > STARTUP_COMMAND_MAX) return false;
  for (const ch of text) {
    const code = ch.codePointAt(0) ?? 0;
    if (code < 0x20 || (code >= 0x7f && code <= 0x9f) || code === 0x2028 || code === 0x2029) return false;
  }
  return true;
}

/** Notes: LF, CR and tab are fine, every other control character is not. */
export function validNotes(text: string): boolean {
  if ([...text].length > NOTES_MAX) return false;
  for (const ch of text) {
    const code = ch.codePointAt(0) ?? 0;
    if (code === 0x09 || code === 0x0a || code === 0x0d) continue;
    if (code < 0x20 || (code >= 0x7f && code <= 0x9f)) return false;
  }
  return true;
}

const DEFAULT_LIMIT: BandwidthField = { value: "1024", unit: "KiB" };

function limitState(kibps: number | null): { mode: LimitMode; field: BandwidthField } {
  if (kibps === null) return { mode: "global", field: DEFAULT_LIMIT };
  if (kibps === 0) return { mode: "unlimited", field: DEFAULT_LIMIT };
  return { mode: "limit", field: fieldFromKiBps(kibps) };
}

/** `undefined` = inherit the global setting (the field is left out of the input). */
function limitValue(mode: LimitMode, field: BandwidthField): number | undefined {
  if (mode === "global") return undefined;
  if (mode === "unlimited") return 0;
  return kibpsFromField(field) ?? undefined;
}

export type FormErrorKey =
  | "hostRequired"
  | "portInvalid"
  | "keyRequired"
  | "groupNameRequired"
  | "limitInvalid"
  | "startupCommandInvalid"
  | "notesInvalid";

export type FormErrors = Partial<
  Record<"host" | "port" | "identityFile" | "newGroupName" | "uploadLimit" | "downloadLimit" | "startupCommand" | "notes", FormErrorKey>
>;

/** `defaultUser` (`connection.defaultUser`) prefills the user of a new bookmark. */
export function emptyForm(defaultUser = ""): BookmarkFormState {
  return {
    name: "",
    host: "",
    port: "",
    user: defaultUser,
    auth: "auto",
    identityFile: "",
    password: "",
    savePassword: true,
    proxyJump: "",
    groupId: "",
    newGroupName: "",
    color: "",
    startDir: "",
    saveAsBookmark: true,
    uploadMode: "global",
    uploadLimit: DEFAULT_LIMIT,
    downloadMode: "global",
    downloadLimit: DEFAULT_LIMIT,
    confirmConnect: false,
    startupCommand: "",
    notes: "",
  };
}

/** New-connection form prefilled from a `[user@]host[:port]` text (quick connect -> bookmark, pasted target). */
export function formFromSpec(spec: string, name = "", defaultUser = ""): BookmarkFormState {
  const parts = parseHostSpec(spec);
  return {
    ...emptyForm(),
    name,
    host: parts ? parts.host : spec.trim(),
    user: parts?.user ?? defaultUser,
    port: parts?.port != null ? String(parts.port) : "",
  };
}

export function encodeJump(ref: JumpRef | null): string {
  if (!ref) return "";
  switch (ref.kind) {
    case "sshConfig":
      return `ssh:${ref.alias}`;
    case "bookmark":
      return `bm:${ref.id}`;
    case "spec":
      return `spec:${ref.spec}`;
  }
}

export function decodeJump(value: string): JumpRef | null {
  if (value.startsWith("ssh:")) return { kind: "sshConfig", alias: value.slice(4) };
  if (value.startsWith("bm:")) return { kind: "bookmark", id: value.slice(3) };
  if (value.startsWith("spec:")) return { kind: "spec", spec: value.slice(5) };
  return null;
}

export function formFromBookmark(b: Bookmark): BookmarkFormState {
  return {
    name: b.name,
    host: b.host,
    port: b.port === null ? "" : String(b.port),
    user: b.user ?? "",
    auth: b.auth.method,
    identityFile: b.auth.method === "key" ? b.auth.identityFile : "",
    password: "",
    savePassword: b.auth.method === "password" ? b.auth.passwordSaved : true,
    proxyJump: encodeJump(b.proxyJump),
    groupId: b.groupId ?? "",
    newGroupName: "",
    color: b.color ?? "",
    startDir: b.startDir ?? "",
    saveAsBookmark: true,
    uploadMode: limitState(b.uploadLimitKiBps).mode,
    uploadLimit: limitState(b.uploadLimitKiBps).field,
    downloadMode: limitState(b.downloadLimitKiBps).mode,
    downloadLimit: limitState(b.downloadLimitKiBps).field,
    confirmConnect: b.confirmConnect,
    startupCommand: b.startupCommand ?? "",
    notes: b.notes ?? "",
  };
}

/** Port field -> number | null (empty) | NaN (invalid). */
export function parsePort(text: string): number | null {
  const t = text.trim();
  if (t === "") return null;
  if (!/^\d+$/.test(t)) return Number.NaN;
  const n = Number(t);
  return n >= 1 && n <= 65535 ? n : Number.NaN;
}

export function validateForm(form: BookmarkFormState): FormErrors {
  const errors: FormErrors = {};
  // The name is optional: it defaults to the host (`toBookmarkInput`).
  if (form.host.trim() === "") errors.host = "hostRequired";
  if (Number.isNaN(parsePort(form.port))) errors.port = "portInvalid";
  if (form.auth === "key" && form.identityFile.trim() === "") errors.identityFile = "keyRequired";
  if (form.uploadMode === "limit" && kibpsFromField(form.uploadLimit) === null) errors.uploadLimit = "limitInvalid";
  if (form.downloadMode === "limit" && kibpsFromField(form.downloadLimit) === null) errors.downloadLimit = "limitInvalid";
  if (form.saveAsBookmark && form.groupId === NEW_GROUP && form.newGroupName.trim() === "") errors.newGroupName = "groupNameRequired";
  if (!validStartupCommand(form.startupCommand)) errors.startupCommand = "startupCommandInvalid";
  if (!validNotes(form.notes)) errors.notes = "notesInvalid";
  return errors;
}

function authInput(form: BookmarkFormState): AuthInput {
  switch (form.auth) {
    case "key":
      return { method: "key", identityFile: form.identityFile.trim() };
    case "password":
      return { method: "password" };
    case "agent":
      return { method: "agent" };
    case "auto":
      return { method: "auto" };
  }
}

/** Builds the contract input. `groupId` must already be resolved (a NEW_GROUP is created by the caller). */
export function toBookmarkInput(form: BookmarkFormState, id: string | null, groupId: string | null): BookmarkInput {
  const port = parsePort(form.port);
  return {
    id,
    name: form.name.trim() || form.host.trim(),
    host: form.host.trim(),
    port: port === null || Number.isNaN(port) ? null : port,
    user: form.user.trim() === "" ? null : form.user.trim(),
    auth: authInput(form),
    proxyJump: decodeJump(form.proxyJump),
    groupId,
    color: form.color === "" ? null : form.color,
    tags: [],
    startDir: form.startDir.trim() === "" ? null : form.startDir.trim(),
    useSshConfig: true,
    confirmConnect: form.confirmConnect,
    // Stored verbatim (a leading space can matter to the shell); whitespace only means none. Never logged.
    startupCommand: form.startupCommand.trim() === "" ? null : form.startupCommand,
    notes: form.notes.trim() === "" ? null : form.notes,
    ...optionalLimit("uploadLimitKiBps", limitValue(form.uploadMode, form.uploadLimit)),
    ...optionalLimit("downloadLimitKiBps", limitValue(form.downloadMode, form.downloadLimit)),
  };
}

function optionalLimit<K extends "uploadLimitKiBps" | "downloadLimitKiBps">(key: K, value: number | undefined): Partial<Record<K, number>> {
  return value === undefined ? {} : ({ [key]: value } as Record<K, number>);
}

/**
 * True when the edit changes what a saved password is bound to (host, port, user or jump host, §9.2):
 * the backend then forgets the password, so the dialog warns first.
 */
export function bindingChanged(original: Bookmark, form: BookmarkFormState): boolean {
  if (original.auth.method !== "password" || !original.auth.passwordSaved) return false;
  const port = parsePort(form.port);
  return (
    original.host !== form.host.trim() ||
    original.port !== (Number.isNaN(port) ? null : port) ||
    (original.user ?? "") !== form.user.trim() ||
    encodeJump(original.proxyJump) !== form.proxyJump ||
    form.auth !== "password"
  );
}
