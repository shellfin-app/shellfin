import { describe, expect, it } from "vitest";
import { errorMessage } from "./errorMessage";

describe("errorMessage (§9.5)", () => {
  it("neutralises control and bidi characters in string params", () => {
    const evil = `/tmp/a${String.fromCodePoint(0x202e)}gpj.exe${String.fromCodePoint(0x1b)}[2J`;
    const msg = errorMessage({ code: "remoteNotFound", params: { path: evil } });
    expect(msg).toContain("⟨U+202E⟩");
    expect(msg).toContain(String.fromCharCode(92) + "x1b");
    expect(msg).not.toContain(String.fromCodePoint(0x202e));
    expect(msg).not.toContain(String.fromCodePoint(0x1b));
  });

  it("truncates very long params and still translates enum params", () => {
    const msg = errorMessage({ code: "remoteNotFound", params: { path: "x".repeat(1000) } });
    expect(msg.length).toBeLessThan(300);
    expect(errorMessage({ code: "notFound", params: { what: "sshConfigHost", id: "a" } })).toContain("SSH config host");
  });
});
