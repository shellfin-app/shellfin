import { describe, expect, it, vi } from "vitest";
import { isRawBytes } from "../backend";
import type { TermMessage } from "../backend";
import type { TermEvent } from "../types";
import { FakeShell } from "./fakeShell";
import { MockBackend } from "./mockBackend";
import { BACKLOG_LIMIT, FLOW_WINDOW, TerminalOutput, floodChunks } from "./termFlow";

const enc = new TextEncoder();
const dec = new TextDecoder();
const tick = (ms = 5) => new Promise<void>((r) => setTimeout(r, ms));
const ESC = String.fromCharCode(0x1b);

function shell() {
  const out: string[] = [];
  const exits: number[] = [];
  const floods: number[] = [];
  const sh = new FakeShell("deploy", "box", "/srv/app", {
    emit: (b) => out.push(dec.decode(b)),
    exit: (c) => exits.push(c),
    flood: (n) => floods.push(n),
  });
  sh.start();
  out.length = 0;
  return { sh, out, exits, floods, text: () => out.join("") };
}
const type = (sh: FakeShell, s: string) => sh.write(enc.encode(s));

describe("FakeShell (realistic mock shell)", () => {
  it("enables bracketed paste and prints a coloured prompt on start", () => {
    const out: string[] = [];
    new FakeShell("deploy", "box", "/srv/app", { emit: (b) => out.push(dec.decode(b)), exit: () => undefined, flood: () => undefined }).start();
    expect(out.join("")).toContain(`${ESC}[?2004h`);
    expect(out.join("")).toContain("deploy@box");
  });

  it("echoes typed characters, handles Backspace, runs commands and prints the next prompt", () => {
    const t = shell();
    type(t.sh, "whoamx");
    type(t.sh, "\x7f");
    type(t.sh, "i\r");
    expect(t.text()).toContain("whoamx\b \b");
    expect(t.text()).toMatch(/\r\ndeploy\r\n.*deploy@box/s);
  });

  it("supports cursor editing: Ctrl+A / Ctrl+E / Ctrl+K / Ctrl+U / Ctrl+W and arrow keys", () => {
    const t = shell();
    type(t.sh, "echo hello world");
    type(t.sh, "\x01"); // Ctrl+A
    type(t.sh, `${ESC}[C`.repeat(5)); // move after "echo "
    type(t.sh, "\x0b"); // Ctrl+K kills "hello world"
    expect(t.text()).toContain(`${ESC}[K`);
    type(t.sh, "ok\r");
    expect(t.text()).toContain("\r\nok\r\n");

    const u = shell();
    type(u.sh, "garbage");
    type(u.sh, "\x15"); // Ctrl+U
    type(u.sh, "pwd\r");
    expect(u.text()).toContain("\r\n/srv/app\r\n");

    const w = shell();
    type(w.sh, "echo one two");
    type(w.sh, "\x17"); // Ctrl+W removes "two"
    type(w.sh, "\r");
    expect(w.text()).toContain("\r\none\r\n"); // "echo one " ran
  });

  it("recalls history with Up/Down", () => {
    const t = shell();
    type(t.sh, "echo first\r");
    type(t.sh, `${ESC}[A\r`);
    expect(t.text().match(/first\r\n/g)?.length).toBeGreaterThanOrEqual(2);
  });

  it("Ctrl+C aborts the line, Ctrl+L clears, unknown commands fail like bash", () => {
    const t = shell();
    type(t.sh, "sleep 100\x03");
    expect(t.text()).toContain("^C\r\n");
    type(t.sh, "\x0c");
    expect(t.text()).toContain(`${ESC}[2J`);
    type(t.sh, "nope\r");
    expect(t.text()).toContain("bash: nope: command not found");
  });

  it("inserts bracketed-paste line breaks into the buffer; Enter then runs every line", () => {
    const t = shell();
    type(t.sh, `${ESC}[200~echo a\necho b${ESC}[201~`);
    expect(t.text()).not.toContain("\r\na\r\n");
    type(t.sh, "\r");
    expect(t.text()).toContain("\r\na\r\n");
    expect(t.text()).toContain("\r\nb\r\n");
  });

  it("handles an escape sequence split across writes", () => {
    const t = shell();
    type(t.sh, "echo x");
    type(t.sh, `${ESC}[`);
    type(t.sh, "D");
    type(t.sh, "y\r");
    expect(t.text()).toContain("\r\nyx\r\n"); // the cursor moved one to the left before 'y' was typed
  });

  it("exit and Ctrl+D end the shell (exit event once, further input ignored)", () => {
    const t = shell();
    type(t.sh, "exit 7\r");
    expect(t.exits).toEqual([7]);
    expect(t.sh.exited).toBe(true);
    type(t.sh, "echo more\r");
    expect(t.exits).toEqual([7]);
    const d = shell();
    type(d.sh, "\x04");
    expect(d.exits).toEqual([0]);
  });

  it("stty size and $COLUMNS follow resizes", () => {
    const t = shell();
    t.sh.resize(132, 43);
    type(t.sh, "stty size\r");
    expect(t.text()).toContain("\r\n43 132\r\n");
  });

  it("flood starts after the echo and ends with a fresh prompt", () => {
    const t = shell();
    type(t.sh, "flood 2\r");
    expect(t.floods).toEqual([2 * 1024 * 1024]);
    expect(t.text().endsWith("$ ")).toBe(true);
  });
});

describe("TerminalOutput (backlog, ack window, overflow — §4.5)", () => {
  function pipe() {
    const messages: TermMessage[] = [];
    const out = new TerminalOutput((m) => messages.push(m));
    const bytes = () => messages.filter(isRawBytes).reduce((n, m) => n + m.byteLength, 0);
    const events = () => messages.filter((m): m is TermEvent => !isRawBytes(m));
    return { out, messages, bytes, events };
  }
  const chunk = new Uint8Array(64 * 1024);

  it("delivers up to the window, holds the rest until acked", () => {
    const p = pipe();
    for (let i = 0; i < 80; i++) p.out.push(chunk); // 5 MiB
    expect(p.bytes()).toBe(FLOW_WINDOW);
    p.out.ack(FLOW_WINDOW);
    expect(p.bytes()).toBe(80 * chunk.length);
    expect(p.events()).toEqual([]);
  });

  it("discards new output beyond the backlog limit and reports it once the backlog is below 1 MiB again", () => {
    const p = pipe();
    const produced = 20 * 1024 * 1024;
    for (let sent = 0; sent < produced; sent += chunk.length) p.out.push(chunk);
    expect(p.out.backlog).toBeLessThanOrEqual(BACKLOG_LIMIT);
    expect(p.events()).toEqual([]); // still behind
    // The frontend catches up.
    while (p.out.unacked > 0) p.out.ack(p.out.unacked);
    const dropped = p.events();
    expect(dropped).toHaveLength(1);
    expect(dropped[0]).toMatchObject({ type: "outputDropped" });
    const droppedBytes = (dropped[0] as { bytes: number }).bytes;
    expect(p.bytes() + droppedBytes).toBe(produced); // nothing unaccounted for
    expect(droppedBytes).toBeGreaterThan(0);
  });

  it("keeps control events in order behind pending output", () => {
    const p = pipe();
    for (let i = 0; i < 70; i++) p.out.push(chunk);
    p.out.event({ type: "exit", code: 0, signal: null });
    expect(p.events()).toEqual([]); // output ahead of it is still held back by the window
    p.out.ack(FLOW_WINDOW);
    expect(p.events()).toEqual([{ type: "exit", code: 0, signal: null }]);
    expect(isRawBytes(p.messages[p.messages.length - 1] as TermMessage)).toBe(false);
  });

  it("floodChunks yields 64 KiB chunks that split escape sequences", () => {
    const chunks = floodChunks(200 * 1024);
    expect(chunks).toHaveLength(4);
    expect(chunks.every((c) => c.length === 64 * 1024)).toBe(true);
  });
});

describe("MockBackend terminals", () => {
  async function open(options: ConstructorParameters<typeof MockBackend>[0] = {}) {
    const backend = new MockBackend(options);
    const sessionId = await backend.openSession({ target: { kind: "sshConfig", alias: "prod-web-01" }, probeOnly: false }, () => undefined);
    const messages: TermMessage[] = [];
    const termId = await backend.openTerminal(sessionId, { cols: 80, rows: 24, pixelWidth: 0, pixelHeight: 0 }, "/var/log", (m) => messages.push(m));
    await tick();
    const text = () => messages.filter(isRawBytes).map((m) => dec.decode(m)).join("");
    return { backend, sessionId, termId, messages, text };
  }

  it("starts in the requested cwd and reports resizes to stty", async () => {
    const t = await open();
    expect(t.text()).toContain("/var/log");
    await t.backend.termResize(t.termId, { cols: 100, rows: 40, pixelWidth: 0, pixelHeight: 0 });
    await t.backend.termWrite(t.termId, Array.from(enc.encode("stty size\r")));
    expect(t.text()).toContain("40 100");
  });

  it("exit emits an exit event; term_restart respawns the shell, also a running one, without reporting its exit", async () => {
    const t = await open();
    const initial = t.text().length;
    await t.backend.termRestart(t.termId); // running: a new shell, no exit event
    expect(t.messages.some((m) => !isRawBytes(m))).toBe(false);
    expect(t.text().length).toBeGreaterThan(initial);
    const before = t.text().length;
    await t.backend.termWrite(t.termId, Array.from(enc.encode("exit 2\r")));
    expect(t.messages.filter((m): m is TermEvent => !isRawBytes(m))).toEqual([{ type: "exit", code: 2, signal: null }]);
    await t.backend.termRestart(t.termId);
    expect(t.text().length).toBeGreaterThan(before);
    expect(t.text().match(/deploy@prod-web-01/g)?.length).toBeGreaterThanOrEqual(2);
    await expect(t.backend.termRestart("term-nope")).rejects.toMatchObject({ error: { code: "terminalNotFound" } });
  });

  it("terminalFlood: without acks the burst is bounded and outputDropped follows once the frontend catches up", async () => {
    const t = await open({ scenario: "terminalFlood", floodBytes: 16 * 1024 * 1024 });
    await tick(80);
    const delivered = () => t.messages.filter(isRawBytes).reduce((n, m) => n + m.byteLength, 0);
    expect(delivered()).toBeLessThanOrEqual(FLOW_WINDOW + 1024); // stuck at the window until acks arrive
    expect(t.messages.some((m) => !isRawBytes(m))).toBe(false);
    for (let guard = 0; guard < 400; guard++) {
      await t.backend.termAck(t.termId, 256 * 1024);
      if (t.messages.some((m) => !isRawBytes(m))) break;
    }
    expect(t.messages.filter((m): m is TermEvent => !isRawBytes(m))[0]).toMatchObject({ type: "outputDropped" });
  });

  it("after an automatic reconnect the terminal gets reconnected and a fresh shell prompt (§5.8)", async () => {
    const t = await open({ scenario: "dropAfter10s", dropAfterMs: 20 });
    const promptsBefore = t.text().match(/deploy@prod-web-01/g)?.length ?? 0;
    await vi.waitFor(() => expect(t.messages.some((m) => !isRawBytes(m) && m.type === "reconnected")).toBe(true), { timeout: 10_000 });
    await tick();
    expect(t.text().match(/deploy@prod-web-01/g)?.length ?? 0).toBeGreaterThan(promptsBefore);
  }, 15_000);
});
