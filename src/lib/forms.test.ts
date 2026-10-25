import { describe, expect, it } from "vitest";
import type { Bookmark, Settings } from "../ipc/types";
import {
  NEW_GROUP,
  bindingChanged,
  decodeJump,
  emptyForm,
  encodeJump,
  formFromBookmark,
  parsePort,
  toBookmarkInput,
  validateForm,
} from "./bookmarkForm";
import { FIELD, formFromSettings, invalidFields, settingsFromForm } from "./settingsForm";
import { serverFingerprintCommand } from "./shellquote";

const saved: Bookmark = {
  id: "b1",
  name: "nas",
  host: "nas.local",
  port: 2222,
  user: "admin",
  auth: { method: "password", passwordSaved: true },
  proxyJump: { kind: "sshConfig", alias: "bastion" },
  groupId: null,
  order: 0,
  color: null,
  tags: [],
  startDir: "/data",
  useSshConfig: true,
  uploadLimitKiBps: null,
  downloadLimitKiBps: null,
  confirmConnect: false,
  startupCommand: null,
  notes: null,
  createdAtMs: 1,
  updatedAtMs: 1,
};

describe("bookmark form", () => {
  it("validates required fields, port range, key file and new group name", () => {
    expect(validateForm({ ...emptyForm(), host: "h" })).toEqual({});
    expect(validateForm({ ...emptyForm(), saveAsBookmark: false })).toEqual({ host: "hostRequired" });
    expect(validateForm({ ...emptyForm(), name: "n", host: "h", port: "70000" }).port).toBe("portInvalid");
    expect(validateForm({ ...emptyForm(), name: "n", host: "h", port: "22x" }).port).toBe("portInvalid");
    expect(validateForm({ ...emptyForm(), name: "n", host: "h", auth: "key" }).identityFile).toBe("keyRequired");
    expect(validateForm({ ...emptyForm(), name: "n", host: "h", groupId: NEW_GROUP }).newGroupName).toBe("groupNameRequired");
    expect(validateForm({ ...emptyForm(), name: "n", host: "h", port: "2222", auth: "key", identityFile: "~/.ssh/k" })).toEqual({});
  });

  it("parses ports", () => {
    expect(parsePort("")).toBeNull();
    expect(parsePort(" 22 ")).toBe(22);
    expect(parsePort("0")).toBeNaN();
    expect(parsePort("65535")).toBe(65535);
  });

  it("builds the contract input for every auth mode (password never inside)", () => {
    const base = { ...emptyForm(), name: " n ", host: " h ", user: " u ", port: "2200", startDir: " /x " };
    expect(toBookmarkInput({ ...base, auth: "key", identityFile: "~/.ssh/k" }, null, "g1")).toMatchObject({
      id: null, name: "n", host: "h", user: "u", port: 2200, startDir: "/x", groupId: "g1", auth: { method: "key", identityFile: "~/.ssh/k" },
    });
    const pw = toBookmarkInput({ ...base, auth: "password", password: "hunter2" }, "b1", null);
    expect(pw.auth).toEqual({ method: "password" });
    expect(JSON.stringify(pw)).not.toContain("hunter2");
    expect(toBookmarkInput({ ...base, auth: "agent" }, null, null).auth).toEqual({ method: "agent" });
    expect(toBookmarkInput({ ...base, auth: "auto", user: "", port: "" }, null, null)).toMatchObject({ user: null, port: null, auth: { method: "auto" } });
  });

  it("round-trips ProxyJump references", () => {
    for (const ref of [{ kind: "sshConfig", alias: "a" }, { kind: "bookmark", id: "i" }, { kind: "spec", spec: "u@h:22" }] as const) {
      expect(decodeJump(encodeJump(ref))).toEqual(ref);
    }
    expect(decodeJump("")).toBeNull();
  });

  it("warns only when host/port/user/jump (or the auth method) of a saved-password bookmark change", () => {
    const form = formFromBookmark(saved);
    expect(bindingChanged(saved, form)).toBe(false);
    expect(bindingChanged(saved, { ...form, host: "other" })).toBe(true);
    expect(bindingChanged(saved, { ...form, port: "22" })).toBe(true);
    expect(bindingChanged(saved, { ...form, user: "root" })).toBe(true);
    expect(bindingChanged(saved, { ...form, proxyJump: "" })).toBe(true);
    expect(bindingChanged(saved, { ...form, name: "renamed", startDir: "/y" })).toBe(false);
    expect(bindingChanged({ ...saved, auth: { method: "password", passwordSaved: false } }, { ...form, host: "other" })).toBe(false);
    expect(bindingChanged({ ...saved, auth: { method: "agent" } }, { ...form, host: "other" })).toBe(false);
  });
});

describe("settings form", () => {
  const settings = {
    version: 1,
    locale: "system",
    general: {
      confirmCloseWithTerminals: true,
      confirmQuit: "withConnections",
      onStartup: "welcome",
      notifyOnDisconnect: false,
      autoUpdate: true,
    },
    terminal: {
      fontSize: 13, scrollback: 10000, cursorBlink: true, copyOnSelect: false, fontFamily: null,
      cursorStyle: "block", lineHeight: 1.2, bell: "off", rightClick: "menu",
      letterSpacing: 0, mouseReporting: true, hideMousePointerWhileTyping: false, minimumContrastRatio: 1,
      wordSeparators: " ()[]{}'\"", multilinePasteWarning: true, onExit: "keep", newTerminalCwd: "home",
      openTerminalOnConnect: true, termType: "xterm-256color", altIsMeta: false, scrollSensitivity: 1, background: null,
      screenReaderMode: false,
    },
    editor: {
      fontSize: 13, tabSize: 4, wordWrap: false, maxFileSizeMiB: 5, fontFamily: null, lineNumbers: true,
      highlightActiveLine: true, indentWithTabs: false, trimTrailingWhitespace: false, insertFinalNewline: false,
      detectIndentation: true, newFileEol: "lf", autoCloseBrackets: false, bracketMatching: true,
      showWhitespace: "none", codeFolding: true, fontLigatures: false,
    },
    connection: {
      keepaliveIntervalSecs: 15, keepaliveCountMax: 3, connectTimeoutSecs: 15, promptTimeoutSecs: 110,
      reconnect: { enabled: true, maxAttempts: 5 }, defaultUser: "", readSshConfig: true, rememberPasswords: true,
    },
    files: {
      showHidden: true, foldersFirst: true, confirmDelete: true, doubleClick: "open", sizeUnits: "binary",
      dateFormat: "relative", showPermissions: false, autoRefresh: 0, columns: true,
    },
    ui: { sidebarWidth: 280, terminalPanelHeight: 300, terminalPanelOpen: true },
    transfers: {
      uploadLimitKiBps: 0, downloadLimitKiBps: 0, maxConcurrent: 3, onConflict: "ask", preserveTimestamps: true,
      notifyOnComplete: false,
    },
    advanced: { logLevel: "info" },
    appearance: { theme: "dark", accent: "teal", uiFontSize: 13, reduceMotion: "system" },
  } satisfies Settings;

  it("round-trips unchanged settings", () => {
    expect(settingsFromForm(settings, formFromSettings(settings))).toEqual(settings);
  });

  it("flags values outside the §4.4 ranges", () => {
    const form = formFromSettings(settings);
    form.numbers[FIELD.TERMINAL_FONT_SIZE] = "7";
    form.numbers[FIELD.PROMPT_TIMEOUT] = "111";
    form.numbers[FIELD.KEEPALIVE_COUNT] = "abc";
    form.numbers[FIELD.RECONNECT_ATTEMPTS] = "0";
    expect(invalidFields(form).sort()).toEqual([FIELD.KEEPALIVE_COUNT, FIELD.PROMPT_TIMEOUT, FIELD.TERMINAL_FONT_SIZE].sort());
  });
});

describe("server-side fingerprint command (§5.3)", () => {
  it("derives the command from the key type", () => {
    expect(serverFingerprintCommand("ssh-ed25519")).toBe("ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub");
    expect(serverFingerprintCommand("ecdsa-sha2-nistp256")).toContain("ssh_host_ecdsa_key.pub");
    expect(serverFingerprintCommand("ssh-rsa")).toContain("ssh_host_rsa_key.pub");
  });
});
