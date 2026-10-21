// Pure logic: settings form <-> contract and validation, bookmark form fields, indentation detection, mouse modes,
// entry details, beep.
import { afterEach, describe, expect, it, vi } from "vitest";
import { makeSettings } from "../ipc/mock/fixtures";
import { playBeep, resetBeepForTests } from "./beep";
import { detectIndent } from "./detectIndent";
import { groupText, modeString, ownerText } from "./entryDetails";
import { MOUSE_MODES, RESET_MOUSE_MODES, resetModesSequence, splitMouseModes } from "./mouseModes";
import {
  CONTRAST_RANGE,
  formFromSettings,
  invalidFields,
  parseContrast,
  scrollbackMemoryMiB,
  settingsFromForm,
  type SettingsFormState,
  validDefaultUser,
  validWordSeparators,
} from "./settingsForm";
import { NOTES_MAX, STARTUP_COMMAND_MAX, emptyForm, formFromSpec, toBookmarkInput, validNotes, validStartupCommand, validateForm } from "./bookmarkForm";

describe("settings form: round trip and ranges", () => {
  it("an untouched form gives the same settings back", () => {
    const s = makeSettings();
    expect(settingsFromForm(s, formFromSettings(s))).toEqual(s);
  });

  it("carries every changed field into the contract", () => {
    const s = makeSettings();
    const base = formFromSettings(s);
    const form: SettingsFormState = {
      ...base,
      confirmQuit: "always",
      onStartup: "restore",
      notifyOnDisconnect: true,
      numbers: { ...base.numbers, "terminal.letterSpacing": "2", "terminal.scrollSensitivity": "4" },
      screenReaderMode: true,
      minimumContrastRatio: "4,5",
      wordSeparators: " /",
      mouseReporting: false,
      hideMousePointerWhileTyping: true,
      multilinePasteWarning: false,
      onExit: "closeOnSuccess",
      newTerminalCwd: "currentFolder",
      openTerminalOnConnect: false,
      termType: "vt100",
      altIsMeta: true,
      detectIndentation: false,
      newFileEol: "crlf",
      autoCloseBrackets: true,
      bracketMatching: false,
      showWhitespace: true,
      codeFolding: false,
      doubleClick: "download",
      sizeUnits: "decimal",
      dateFormat: "absolute",
      showPermissions: true,
      autoRefresh: "30",
      defaultUser: " deploy ",
      readSshConfig: false,
      rememberPasswords: false,
      preserveTimestamps: false,
      notifyOnComplete: true,
    };
    const out = settingsFromForm(s, form);
    expect(out.general).toMatchObject({ confirmQuit: "always", onStartup: "restore", notifyOnDisconnect: true });
    expect(out.terminal).toMatchObject({
      letterSpacing: 2,
      scrollSensitivity: 4,
      minimumContrastRatio: 4.5,
      wordSeparators: " /",
      mouseReporting: false,
      hideMousePointerWhileTyping: true,
      multilinePasteWarning: false,
      onExit: "closeOnSuccess",
      newTerminalCwd: "currentFolder",
      openTerminalOnConnect: false,
      termType: "vt100",
      altIsMeta: true,
      screenReaderMode: true,
    });
    expect(out.editor).toMatchObject({
      detectIndentation: false,
      newFileEol: "crlf",
      autoCloseBrackets: true,
      bracketMatching: false,
      showWhitespace: true,
      codeFolding: false,
    });
    expect(out.files).toMatchObject({ doubleClick: "download", sizeUnits: "decimal", dateFormat: "absolute", showPermissions: true, autoRefresh: 30 });
    expect(out.connection).toMatchObject({ defaultUser: "deploy", readSshConfig: false, rememberPasswords: false });
    expect(out.transfers).toMatchObject({ preserveTimestamps: false, notifyOnComplete: true });
    // what the other agents / older fields own is untouched
    expect(out.appearance).toEqual(s.appearance);
    expect(out.terminal.background).toBe(s.terminal.background);
  });

  it("uses the new ranges: scrollback 10-1 000 000, file size 1-256, parallel transfers 1-64", () => {
    const form = formFromSettings(makeSettings());
    const check = (path: string, value: string) => invalidFields({ ...form, numbers: { ...form.numbers, [path]: value } }).includes(path);
    expect(check("terminal.scrollback", "10")).toBe(false);
    expect(check("terminal.scrollback", "9")).toBe(true);
    expect(check("terminal.scrollback", "1000000")).toBe(false);
    expect(check("terminal.scrollback", "1000001")).toBe(true);
    expect(check("editor.maxFileSizeMiB", "256")).toBe(false);
    expect(check("editor.maxFileSizeMiB", "257")).toBe(true);
    expect(check("editor.maxFileSizeMiB", "0")).toBe(true);
    expect(check("transfers.maxConcurrent", "64")).toBe(false);
    expect(check("transfers.maxConcurrent", "65")).toBe(true);
  });

  it("flags invalid values by their contract path", () => {
    const f = formFromSettings(makeSettings());
    expect(invalidFields(f)).toEqual([]);
    expect(invalidFields({ ...f, numbers: { ...f.numbers, "terminal.letterSpacing": "6" } })).toEqual(["terminal.letterSpacing"]);
    expect(invalidFields({ ...f, numbers: { ...f.numbers, "terminal.scrollSensitivity": "0" } })).toEqual(["terminal.scrollSensitivity"]);
    expect(invalidFields({ ...f, minimumContrastRatio: "22" })).toEqual(["terminal.minimumContrastRatio"]);
    expect(invalidFields({ ...f, wordSeparators: "é" })).toEqual(["terminal.wordSeparators"]);
    expect(invalidFields({ ...f, defaultUser: "a b" })).toEqual(["connection.defaultUser"]);
  });
});

describe("settings value rules", () => {
  it("contrast ratio: 1 to 21, decimal point or comma", () => {
    expect(parseContrast("1")).toBe(1);
    expect(parseContrast("4,5")).toBe(4.5);
    expect(parseContrast("21")).toBe(21);
    expect(parseContrast("0.9")).toBeNull();
    expect(parseContrast("21.1")).toBeNull();
    expect(parseContrast("abc")).toBeNull();
    expect(CONTRAST_RANGE).toEqual({ min: 1, max: 21 });
  });

  it("word separators: up to 64 printable ASCII characters, may be empty", () => {
    expect(validWordSeparators("")).toBe(true);
    expect(validWordSeparators(" ()[]{}'\"")).toBe(true);
    expect(validWordSeparators("a".repeat(64))).toBe(true);
    expect(validWordSeparators("a".repeat(65))).toBe(false);
    expect(validWordSeparators("tab\there")).toBe(false);
    expect(validWordSeparators("é")).toBe(false);
  });

  it("default user: empty or 1-255 characters without whitespace or control characters", () => {
    expect(validDefaultUser("")).toBe(true);
    expect(validDefaultUser("deploy")).toBe(true);
    expect(validDefaultUser("a".repeat(255))).toBe(true);
    expect(validDefaultUser("a".repeat(256))).toBe(false);
    expect(validDefaultUser("a b")).toBe(false);
    expect(validDefaultUser("a\u0000b")).toBe(false);
  });

  it("scrollback memory estimate is lines x columns x 12 bytes", () => {
    expect(scrollbackMemoryMiB(1_000_000, 200)).toBe(Math.round((1_000_000 * 200 * 12) / 1024 / 1024));
    expect(scrollbackMemoryMiB(100_000, 100)).toBe(Math.round((100_000 * 100 * 12) / 1024 / 1024));
  });
});

describe("bookmark form: confirmConnect, startupCommand, notes", () => {
  it("startup command: one line, at most 1024 characters, no control characters", () => {
    expect(validStartupCommand("")).toBe(true);
    expect(validStartupCommand("cd /srv && ls -la")).toBe(true);
    expect(validStartupCommand("x".repeat(STARTUP_COMMAND_MAX))).toBe(true);
    expect(validStartupCommand("x".repeat(STARTUP_COMMAND_MAX + 1))).toBe(false);
    expect(validStartupCommand("ls\nrm -rf x")).toBe(false);
    expect(validStartupCommand("ls\r")).toBe(false);
    expect(validStartupCommand("a\tb")).toBe(false);
    expect(validStartupCommand("a\u001bb")).toBe(false);
    expect(validStartupCommand("a\u007fb")).toBe(false);
    expect(validStartupCommand("a\u0085b")).toBe(false);
    expect(validStartupCommand("a\u2028b")).toBe(false);
  });

  it("notes: at most 4000 characters, newline / CR / tab allowed, other control characters not", () => {
    expect(validNotes("line 1\nline 2\tx\r\n")).toBe(true);
    expect(validNotes("x".repeat(NOTES_MAX))).toBe(true);
    expect(validNotes("x".repeat(NOTES_MAX + 1))).toBe(false);
    expect(validNotes("a\u0000b")).toBe(false);
    expect(validNotes("a\u001bb")).toBe(false);
  });

  it("the form reports the invalid fields and builds the input verbatim", () => {
    const form = { ...emptyForm(), host: "h.example.com", startupCommand: "a\nb", notes: "x".repeat(NOTES_MAX + 1) };
    expect(validateForm(form)).toMatchObject({ startupCommand: "startupCommandInvalid", notes: "notesInvalid" });

    const ok = { ...emptyForm(), host: "h.example.com", confirmConnect: true, startupCommand: " tmux attach ", notes: "  note\nmore " };
    expect(validateForm(ok)).toEqual({});
    expect(toBookmarkInput(ok, null, null)).toMatchObject({ confirmConnect: true, startupCommand: " tmux attach ", notes: "  note\nmore " });
    const blank = { ...ok, startupCommand: "   ", notes: "\n " };
    expect(toBookmarkInput(blank, null, null)).toMatchObject({ startupCommand: null, notes: null });
  });

  it("the default user prefills new connections but never overrides a typed one", () => {
    expect(emptyForm("deploy").user).toBe("deploy");
    expect(formFromSpec("h.example.com", "", "deploy").user).toBe("deploy");
    expect(formFromSpec("root@h.example.com", "", "deploy").user).toBe("root");
  });
});

describe("indentation detection", () => {
  it("finds tabs", () => {
    expect(detectIndent("a\n\tb\n\tc\n\t\td\n\te\n")).toEqual({ tabs: true });
  });
  it("finds the space width", () => {
    expect(detectIndent("a {\n    b\n    c {\n        d\n    }\n    e\n}\n")).toEqual({ tabs: false, width: 4 });
    expect(detectIndent("a:\n  b: 1\n  c:\n    d: 2\n  e: 3\n")).toEqual({ tabs: false, width: 2 });
  });
  it("gives no answer when the text is ambiguous or too small", () => {
    expect(detectIndent("")).toBeNull();
    expect(detectIndent("no indentation\nat all\n")).toBeNull();
    expect(detectIndent("a\n\tb\n  c\n\td\n  e\n")).toBeNull(); // tabs and spaces mixed about equally
    expect(detectIndent("a\n  b\n")).toBeNull(); // too little evidence
  });
  it("ignores block comment continuations (a single leading space)", () => {
    expect(detectIndent("/*\n * a\n * b\n * c\n */\n")).toBeNull();
  });
});

describe("mouse modes", () => {
  it("splits mouse tracking modes from the others", () => {
    expect(splitMouseModes([1000, 1006, 25])).toEqual({ mouse: [1000, 1006], rest: [25] });
    expect(splitMouseModes([1049])).toEqual({ mouse: [], rest: [1049] });
    expect(MOUSE_MODES).toContain(1003);
    expect(MOUSE_MODES).not.toContain(1049);
  });
  it("undoes the mouse modes of a mixed sequence as one reset", () => {
    expect(resetModesSequence([1000, 1006])).toBe("\u001b[?1000;1006l");
    expect(resetModesSequence([])).toBe("");
  });
  it("resets every mouse mode", () => {
    for (const m of MOUSE_MODES) expect(RESET_MOUSE_MODES).toContain(`\u001b[?${m}l`);
  });
});

describe("file entry details", () => {
  const base = { kind: "file" as const, permissions: 0o644, owner: null, group: null, uid: 1000, gid: 100 };
  it("formats the mode like ls -l", () => {
    expect(modeString(base)).toBe("-rw-r--r--");
    expect(modeString({ ...base, kind: "dir", permissions: 0o755 })).toBe("drwxr-xr-x");
    expect(modeString({ ...base, kind: "symlink", permissions: 0o777 })).toBe("lrwxrwxrwx");
    expect(modeString({ ...base, kind: "dir", permissions: 0o1777 })).toBe("drwxrwxrwt");
    expect(modeString({ ...base, kind: "dir", permissions: 0o1776 })).toBe("drwxrwxrwT");
    expect(modeString({ ...base, permissions: 0o4755 })).toBe("-rwsr-xr-x");
    expect(modeString({ ...base, permissions: 0o2745 })).toBe("-rwxr-Sr-x");
    expect(modeString({ ...base, permissions: null })).toBeNull();
  });
  it("prefers names and falls back to numeric ids", () => {
    expect(ownerText({ owner: "alice", uid: 1000 })).toBe("alice");
    expect(ownerText({ owner: null, uid: 1000 })).toBe("1000");
    expect(ownerText({ owner: null, uid: null })).toBeNull();
    expect(groupText({ group: "staff", gid: 20 })).toBe("staff");
    expect(groupText({ group: null, gid: 20 })).toBe("20");
  });
});

describe("terminal bell beep", () => {
  afterEach(() => {
    resetBeepForTests();
    vi.unstubAllGlobals();
  });

  it("is silent without Web Audio", () => {
    expect(playBeep(1000)).toBe(false);
  });

  it("synthesizes a short tone with Web Audio and rate-limits repeats", () => {
    const osc = { type: "", frequency: { value: 0 }, connect: vi.fn(), start: vi.fn(), stop: vi.fn() };
    const gain = { gain: { setValueAtTime: vi.fn(), linearRampToValueAtTime: vi.fn() }, connect: vi.fn() };
    class FakeAudio {
      state = "running";
      currentTime = 0;
      destination = {};
      createOscillator = () => osc;
      createGain = () => gain;
      resume = () => Promise.resolve();
    }
    vi.stubGlobal("AudioContext", FakeAudio);
    expect(playBeep(1000)).toBe(true);
    expect(osc.frequency.value).toBe(880);
    expect(osc.start).toHaveBeenCalledOnce();
    expect(osc.stop).toHaveBeenCalledOnce();
    expect(playBeep(1010)).toBe(false); // too soon
    expect(playBeep(1500)).toBe(true);
  });
});
