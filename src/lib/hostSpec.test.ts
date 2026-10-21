import { describe, expect, it } from "vitest";
import { parseHostSpec } from "./hostSpec";
import { editorStatus } from "./editorStatus";
import { currentStep } from "./connectSteps";
import { parsePaletteQuery } from "./paletteQuery";
import type { BufferMeta } from "../state/slices/buffers";

describe("parseHostSpec", () => {
  it("splits user, host and port", () => {
    expect(parseHostSpec("root@10.1.2.3:2200")).toEqual({ user: "root", host: "10.1.2.3", port: 2200 });
    expect(parseHostSpec("deploy@example.com")).toEqual({ user: "deploy", host: "example.com", port: null });
    expect(parseHostSpec("example.com:2222")).toEqual({ user: null, host: "example.com", port: 2222 });
    expect(parseHostSpec("u@[::1]:22")).toEqual({ user: "u", host: "::1", port: 22 });
    expect(parseHostSpec("  a@b:1  ")).toEqual({ user: "a", host: "b", port: 1 });
  });

  it("leaves plain hosts and junk alone", () => {
    expect(parseHostSpec("example.com")).toBeNull();
    expect(parseHostSpec("::1")).toBeNull();
    expect(parseHostSpec("fe80::1")).toBeNull();
    expect(parseHostSpec("host:99999")).toBeNull();
    expect(parseHostSpec("two words@host")).toBeNull();
    expect(parseHostSpec("@host")).toBeNull();
    expect(parseHostSpec("")).toBeNull();
  });
});

describe("editorStatus", () => {
  const base = { saving: false, conflict: null, pendingSave: false, dirty: false, savedAtMs: null, load: "ready" } as unknown as BufferMeta;
  it("never says saved while the buffer is dirty or offline", () => {
    expect(editorStatus({ ...base, dirty: true, savedAtMs: 5 }, true)).toEqual({ kind: "unsaved", savedAtMs: 5 });
    expect(editorStatus({ ...base, dirty: true, savedAtMs: 5 }, false)).toEqual({ kind: "offlineDirty" });
    expect(editorStatus({ ...base, savedAtMs: 5 }, true)).toEqual({ kind: "saved", savedAtMs: 5 });
    expect(editorStatus({ ...base, dirty: true, saving: true }, true)).toEqual({ kind: "saving" });
    expect(editorStatus({ ...base, conflict: { size: 1, mtime: 1 } }, true)).toEqual({ kind: "conflict" });
    expect(editorStatus({ ...base, dirty: true, pendingSave: true }, false)).toEqual({ kind: "pendingSave" });
    expect(editorStatus(base, true)).toEqual({ kind: "none" });
  });
});

describe("small helpers", () => {
  it("maps the session phase and prompt to a step", () => {
    expect(currentStep("resolving", undefined)).toBe("resolve");
    expect(currentStep("connecting", undefined)).toBe("connect");
    expect(currentStep("connecting", "hostKeyUnknown")).toBe("hostKey");
    expect(currentStep("authenticating", "password")).toBe("auth");
    expect(currentStep("openingChannels", undefined)).toBe("open");
  });

  it("parses palette prefixes", () => {
    expect(parsePaletteQuery("> Save")).toEqual({ prefix: "commands", text: "save" });
    expect(parsePaletteQuery("@nas")).toEqual({ prefix: "hosts", text: "nas" });
    expect(parsePaletteQuery("dep")).toEqual({ prefix: "", text: "dep" });
  });

  it("A bare bracketed IPv6 host loses its brackets", () => {
    expect(parseHostSpec("[::1]")).toEqual({ user: null, host: "::1", port: null });
    expect(parseHostSpec("[::1]:22")).toEqual({ user: null, host: "::1", port: 22 });
  });
});
