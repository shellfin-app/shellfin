// Key families built dynamically from backend enums (docs/ARCHITECTURE.md §8.2.2 "Backend output is never prose").
// Each list is checked at compile time against the generated bindings: adding a variant in Rust makes `tsc`
// fail here until the list (and therefore translation.json, see i18n.test.ts) is updated.
import type {
  AuthMethodKind,
  ConfigWarningCode,
  DisconnectKind,
  EntityKind,
  ErrorCode,
  ExportSkipCode,
  HostKeyVerification,
  ImportEntryKind,
  ImportInvalidCode,
  ImportInvalidReason,
  IoOp,
  KeyLoadReason,
  PromptLastError,
  SaveStrategy,
  TimeoutOp,
} from "../ipc/types";

export const ERROR_CODES = [
  "internal", "bug", "cancelled", "promptTimeout", "storageReadOnly", "keyringUnavailable", "keyringError", "promptNotFound",
  "invalidArgument", "notFound", "io", "storageCorrupt", "invalidHostSpec", "invalidHostname", "proxyJumpCycle",
  "proxyJumpTooDeep", "proxyCommandUnsupported", "dnsFailed", "connectRefused", "connectTimeout",
  "networkUnreachable", "jumpChannelFailed", "kexFailed", "protocolError", "hostKeyUnknownStrict",
  "hostKeyRejected", "hostKeyChanged", "hostKeyRevoked", "hostKeyCaManaged", "hostKeyMismatchOnReconnect",
  "authFailed", "keyLoadFailed", "passphraseIncorrect", "knownHostsIgnored", "agentUnavailable", "agentUntrusted",
  "agentNoIdentities",
  "agentConstraintUnsupported", "tooManyAuthFailures", "disconnected", "sessionNotFound", "notConnected",
  "terminalNotFound", "channelOpenFailed", "sftpUnavailable", "remoteNotFound", "permissionDenied", "remoteExists",
  "notADirectory", "isADirectory", "notARegularFile", "directoryNotEmpty", "fileIsBinary", "remoteChanged", "fileTooLarge",
  "decodeFailed", "encodingUnmappable", "unsupportedEncoding", "sftpFailure", "timeout", "localNotFound",
  "localPermissionDenied", "localIsADirectory", "dropExpired",
  "importInvalidFile", "secretStorageDisabled",
  "updatesDisabled", "updateCheckFailed", "updateNotFound", "updateInProgress", "updateManualOnly",
  "updateDownloadFailed", "updateVerifyFailed", "updateInstallFailed",
] as const satisfies readonly ErrorCode["code"][];

export const CONFIG_WARNING_CODES = [
  "unsupportedMatchCriteria", "proxyCommandUnsupported", "includeDepthExceeded", "includeRecursiveGlob",
  "includeTooManyFiles", "includeCycle", "untrustedFile", "fileTooLarge", "badLine", "unknownToken", "invalidHostname",
] as const satisfies readonly ConfigWarningCode["code"][];

export const ENTITY_KINDS = ["bookmark", "group", "sshConfigHost", "transfer"] as const satisfies readonly EntityKind[];

export const IO_OPS = [
  "readConfig", "readKnownHosts", "writeKnownHosts", "readKey", "readStorage", "writeStorage", "readLocalFile",
  "writeLocalFile", "revealPath",
] as const satisfies readonly IoOp[];

export const TIMEOUT_OPS = ["connect", "sftpRequest", "listing", "prompt"] as const satisfies readonly TimeoutOp[];

export const KEY_LOAD_REASONS = [
  "notFound", "permissionDenied", "unsupported", "corrupt", "insecurePermissions", "notAFile", "tooLarge",
  "untrustedPath",
] as const satisfies readonly KeyLoadReason[];

export const DISCONNECT_KINDS = [
  "keepaliveTimeout", "connectionReset", "serverClosed", "networkError", "jumpHostLost", "userRequested",
] as const satisfies readonly DisconnectKind[];

export const AUTH_METHODS = [
  "none", "publickey", "agent", "password", "keyboardInteractive",
] as const satisfies readonly AuthMethodKind[];

export const SAVE_STRATEGIES = ["atomic", "inPlace", "renameFallback"] as const satisfies readonly SaveStrategy[];

export const HOST_KEY_VERIFICATIONS = [
  "knownHosts", "userConfirmed", "autoAccepted",
] as const satisfies readonly HostKeyVerification[];

/** Plural keys (`_one` / `_other`). */
export const PROMPT_ERRORS = ["passphraseIncorrect", "passwordIncorrect"] as const satisfies readonly PromptLastError[];

// Bookmark import / export (ARCHITECTURE §7.7).
export const IMPORT_INVALID_REASONS = [
  "notJson", "unknownFormat", "unsupportedVersion", "tooLarge", "tooMany", "badStructure",
] as const satisfies readonly ImportInvalidReason[];
export const IMPORT_INVALID_CODES = [
  "missing", "wrongType", "invalid", "duplicateId", "unresolvedReference",
] as const satisfies readonly ImportInvalidCode[];
export const IMPORT_ENTRY_KINDS = ["bookmark", "group"] as const satisfies readonly ImportEntryKind[];
export const EXPORT_SKIP_CODES = ["notRepresentable", "unsafeValue"] as const satisfies readonly ExportSkipCode[];

/** family prefix (the part before the enum value) -> values, for the completeness test. */
export const KEY_FAMILIES = {
  errors: ERROR_CODES,
  configWarnings: CONFIG_WARNING_CODES,
  entity: ENTITY_KINDS,
  ioOp: IO_OPS,
  timeoutOp: TIMEOUT_OPS,
  keyLoadReason: KEY_LOAD_REASONS,
  disconnectKind: DISCONNECT_KINDS,
  authMethod: AUTH_METHODS,
  saveStrategy: SAVE_STRATEGIES,
  hostKeyVerification: HOST_KEY_VERIFICATIONS,
  importInvalidReason: IMPORT_INVALID_REASONS,
  importInvalidCode: IMPORT_INVALID_CODES,
  importEntry: IMPORT_ENTRY_KINDS,
  exportSkipCode: EXPORT_SKIP_CODES,
} as const;

// ---- exhaustiveness: every variant in the bindings is listed above ----
type Assert<T extends true> = T;
type NoneMissing<All, Listed extends readonly unknown[]> = [Exclude<All, Listed[number]>] extends [never] ? true : false;

export type _ErrorCodes = Assert<NoneMissing<ErrorCode["code"], typeof ERROR_CODES>>;
export type _ConfigWarnings = Assert<NoneMissing<ConfigWarningCode["code"], typeof CONFIG_WARNING_CODES>>;
export type _EntityKinds = Assert<NoneMissing<EntityKind, typeof ENTITY_KINDS>>;
export type _IoOps = Assert<NoneMissing<IoOp, typeof IO_OPS>>;
export type _TimeoutOps = Assert<NoneMissing<TimeoutOp, typeof TIMEOUT_OPS>>;
export type _KeyLoadReasons = Assert<NoneMissing<KeyLoadReason, typeof KEY_LOAD_REASONS>>;
export type _DisconnectKinds = Assert<NoneMissing<DisconnectKind, typeof DISCONNECT_KINDS>>;
export type _AuthMethods = Assert<NoneMissing<AuthMethodKind, typeof AUTH_METHODS>>;
export type _SaveStrategies = Assert<NoneMissing<SaveStrategy, typeof SAVE_STRATEGIES>>;
export type _HostKeyVerifications = Assert<NoneMissing<HostKeyVerification, typeof HOST_KEY_VERIFICATIONS>>;
export type _PromptErrors = Assert<NoneMissing<PromptLastError, typeof PROMPT_ERRORS>>;

// ---- bookmark import / export (ARCHITECTURE §7.7) ----
export type _ImportReasons = Assert<NoneMissing<ImportInvalidReason, typeof IMPORT_INVALID_REASONS>>;
export type _ImportCodes = Assert<NoneMissing<ImportInvalidCode, typeof IMPORT_INVALID_CODES>>;
export type _ImportEntries = Assert<NoneMissing<ImportEntryKind, typeof IMPORT_ENTRY_KINDS>>;
export type _ExportSkipCodes = Assert<NoneMissing<ExportSkipCode, typeof EXPORT_SKIP_CODES>>;
