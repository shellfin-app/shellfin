// A scripted "shell" for the MockBackend: a small readline-style line editor (cursor movement, history, Ctrl+A/E/K/U/W/C/D/L,
// bracketed paste) plus a handful of canned commands. Output is raw bytes, what a PTY would deliver.
const GREEN = "\x1b[32m";
const BLUE = "\x1b[34m";
const RESET = "\x1b[0m";
const ESC = "\x1b";

const CANNED: Record<string, string> = {
  "docker compose ps": [
    "NAME           IMAGE               STATUS        PORTS",
    "app-web-1      app/web:2.14.0      Up 3 days     0.0.0.0:443->443/tcp",
    "app-worker-1   app/worker:2.14.0   Up 3 days",
    "app-redis-1    redis:7-alpine      Up 3 days     6379/tcp",
  ].join("\r\n"),
  "docker compose up -d worker": ["[+] Running 1/1", ` ${GREEN}✔${RESET} Container app-worker-1  ${GREEN}Started${RESET}   1.2s`].join(
    "\r\n",
  ),
  ls: "certs  logs  nginx  docker-compose.yml  deploy.sh  README.md",
  whoami: "deploy",
  uname: "Linux",
  "uname -a": "Linux prod-web-01 6.1.0-18-amd64 #1 SMP PREEMPT_DYNAMIC Debian 6.1.76-1 x86_64 GNU/Linux",
  id: "uid=1001(deploy) gid=1001(deploy) groups=1001(deploy),998(docker)",
  date: "Tue Oct 20 09:41:07 UTC 2026",
  links: [
    "Docs: https://example.com/docs/getting-started?lang=en.",
    "Hyperlink: \x1b]8;;https://example.com/status\x1b\\service status page\x1b]8;;\x1b\\",
  ].join("\r\n"),
};

const encoder = new TextEncoder();
const decoder = new TextDecoder();

interface Csi {
  params: string;
  final: string;
  length: number;
}

/** Parses `ESC [ params final` at `text[at]`: the sequence, `"incomplete"` at the end of the text, or `null`. */
function parseCsi(text: string, at: number): Csi | "incomplete" | null {
  if (at + 1 >= text.length) return "incomplete";
  if (text.charAt(at + 1) !== "[") return null;
  let j = at + 2;
  while (j < text.length && /[0-9;]/.test(text.charAt(j))) j += 1;
  if (j >= text.length) return "incomplete";
  const final = text.charAt(j);
  if (!/[A-Za-z~]/.test(final)) return null;
  return { params: text.slice(at + 2, j), final, length: j + 1 - at };
}

export interface ShellIo {
  emit(bytes: Uint8Array): void;
  /** The shell process ended (`exit`, Ctrl+D). */
  exit(code: number): void;
  /** `flood [MiB]`: the backend generates this many bytes of output. */
  flood(bytes: number): void;
}

export class FakeShell {
  private line: string[] = [];
  private cursor = 0;
  private history: string[] = [];
  private historyIndex = -1;
  private bracketedPaste = true;
  private inPaste = false;
  private carry = "";
  private cols = 80;
  private rows = 24;
  private title = "";
  private pendingFlood: number | null = null;
  exited = false;

  constructor(
    private readonly user: string,
    private readonly host: string,
    private cwd: string,
    private readonly io: ShellIo,
  ) {}

  /** The initial prompt (also after a restart). */
  start(): void {
    this.exited = false;
    this.line = [];
    this.cursor = 0;
    this.inPaste = false;
    this.carry = "";
    this.out(this.bracketedPaste ? `${ESC}[?2004h` : "");
    this.out(this.prompt());
  }

  /** A new shell after `term_restart` / a reconnect. */
  restart(): void {
    this.cwd = this.cwd || "/";
    this.start();
  }

  resize(cols: number, rows: number): void {
    this.cols = cols;
    this.rows = rows;
  }

  private out(text: string): void {
    if (text) this.io.emit(encoder.encode(text));
  }

  private prompt(): string {
    return `${GREEN}${this.user}@${this.host}${RESET}:${BLUE}${this.cwd}${RESET}$ `;
  }

  /** Redraws the edited line after a non-append edit. */
  private redraw(): string {
    const tail = this.line.length - this.cursor;
    return `\r${ESC}[2K${this.prompt()}${this.line.join("").replaceAll("\n", "\r\n")}${tail > 0 ? `${ESC}[${tail}D` : ""}`;
  }

  /** Feeds bytes typed (or pasted) by the user. */
  write(data: Uint8Array): void {
    if (this.exited) return;
    const text = this.carry + decoder.decode(data, { stream: true });
    this.carry = "";
    let out = "";
    let i = 0;
    while (i < text.length && !this.exited) {
      const ch = text.charAt(i);
      if (ch === ESC) {
        const seq = parseCsi(text, i);
        if (seq === "incomplete") {
          // Sequence cut off at the end of the chunk: keep it for the next write.
          this.carry = text.slice(i);
          break;
        }
        if (seq === null) {
          i += 1;
          continue;
        }
        i += seq.length;
        out += this.csi(seq.params, seq.final);
        continue;
      }
      i += 1;
      out += this.key(ch);
    }
    this.out(out);
    const flood = this.pendingFlood;
    if (flood !== null) {
      this.pendingFlood = null;
      this.io.flood(flood);
      this.out(this.prompt());
    }
  }

  private csi(params: string, final: string): string {
    if (final === "~" && params === "200") {
      this.inPaste = true;
      return "";
    }
    if (final === "~" && params === "201") {
      this.inPaste = false;
      return "";
    }
    switch (final) {
      case "A":
        return this.recall(1);
      case "B":
        return this.recall(-1);
      case "C":
        return this.move(this.cursor + 1);
      case "D":
        return this.move(this.cursor - 1);
      case "H":
        return this.move(0);
      case "F":
        return this.move(this.line.length);
      case "~":
        if (params === "3" && this.cursor < this.line.length) {
          this.line.splice(this.cursor, 1);
          return this.redraw();
        }
        return "";
      default:
        return "";
    }
  }

  private move(to: number): string {
    const target = Math.max(0, Math.min(this.line.length, to));
    const delta = target - this.cursor;
    this.cursor = target;
    if (delta === 0) return "";
    return delta > 0 ? `${ESC}[${delta}C` : `${ESC}[${-delta}D`;
  }

  private recall(direction: 1 | -1): string {
    const next = this.historyIndex + direction;
    if (next < -1 || next >= this.history.length) return "";
    this.historyIndex = next;
    this.line = [...(next === -1 ? "" : (this.history[this.history.length - 1 - next] ?? ""))];
    this.cursor = this.line.length;
    return this.redraw();
  }

  private key(ch: string): string {
    if (this.inPaste) {
      // Bracketed paste: line breaks are inserted into the buffer, not executed.
      if (ch === "\r" || ch === "\n") return this.insert("\n");
      return ch >= " " || ch === "\t" ? this.insert(ch) : "";
    }
    switch (ch) {
      case "\r":
      case "\n":
        return this.enter();
      case "\x7f":
      case "\b":
        if (this.cursor === 0) return "";
        this.line.splice(this.cursor - 1, 1);
        this.cursor -= 1;
        return this.cursor === this.line.length ? "\b \b" : this.redraw();
      case "\x03": // Ctrl+C
        this.line = [];
        this.cursor = 0;
        return `^C\r\n${this.prompt()}`;
      case "\x04": // Ctrl+D
        if (this.line.length === 0) return this.leave(0);
        return "";
      case "\x01": // Ctrl+A
        return this.move(0);
      case "\x05": // Ctrl+E
        return this.move(this.line.length);
      case "\x0b": // Ctrl+K: kill to end of line
        this.line.splice(this.cursor);
        return `${ESC}[K`;
      case "\x15": // Ctrl+U: kill to start of line
        this.line.splice(0, this.cursor);
        this.cursor = 0;
        return this.redraw();
      case "\x17": {
        // Ctrl+W: kill the previous word
        let start = this.cursor;
        while (start > 0 && this.line[start - 1] === " ") start -= 1;
        while (start > 0 && this.line[start - 1] !== " ") start -= 1;
        this.line.splice(start, this.cursor - start);
        this.cursor = start;
        return this.redraw();
      }
      case "\x0c": // Ctrl+L
        return `${ESC}[H${ESC}[2J${this.redraw()}`;
      case "\t":
        return "\x07";
      default:
        return ch >= " " ? this.insert(ch) : "";
    }
  }

  private insert(ch: string): string {
    this.line.splice(this.cursor, 0, ch);
    this.cursor += 1;
    if (this.cursor === this.line.length) return ch === "\n" ? "\r\n" : ch;
    return this.redraw();
  }

  private enter(): string {
    const text = this.line.join("");
    this.line = [];
    this.cursor = 0;
    this.historyIndex = -1;
    let out = "\r\n";
    for (const command of text.split("\n")) {
      const trimmed = command.trim();
      if (trimmed !== "") this.history.push(trimmed);
      out += this.run(trimmed);
      if (this.exited) return out;
    }
    return this.pendingFlood === null ? out + this.prompt() : out;
  }

  private leave(code: number): string {
    this.exited = true;
    this.io.exit(code);
    return `exit\r\n`;
  }

  private run(command: string): string {
    if (command === "") return "";
    const canned = CANNED[command];
    if (canned !== undefined) return `${canned}\r\n`;
    const [name = "", ...args] = command.split(/\s+/);
    switch (name) {
      case "pwd":
        return `${this.cwd}\r\n`;
      case "cd": {
        const target = args[0] ?? "/srv/app";
        this.cwd = target.startsWith("/") ? target : `${this.cwd.replace(/\/$/, "")}/${target}`;
        return "";
      }
      case "echo":
        return `${args.join(" ").replaceAll("$COLUMNS", String(this.cols)).replaceAll("$LINES", String(this.rows))}\r\n`;
      case "stty":
        return args[0] === "size" ? `${this.rows} ${this.cols}\r\n` : "";
      case "clear":
        return `${ESC}[H${ESC}[2J${ESC}[3J`;
      case "hostname":
        return `${this.host}\r\n`;
      case "seq": {
        const n = Math.min(Number(args[0] ?? "10") || 10, 5000);
        return Array.from({ length: n }, (_, i) => `${i + 1}\r\n`).join("");
      }
      case "title":
        this.title = args.join(" ");
        return `${ESC}]0;${this.title}\x07`;
      case "bracketed":
        this.bracketedPaste = args[0] !== "off";
        return `${ESC}[?2004${this.bracketedPaste ? "h" : "l"}`;
      case "flood": {
        const mib = Math.min(Math.max(Number(args[0] ?? "16") || 16, 1), 256);
        this.pendingFlood = mib * 1024 * 1024; // started after the echo was delivered
        return "";
      }
      case "exit":
        this.exited = true;
        this.io.exit(Number(args[0] ?? "0") || 0);
        return "exit\r\n";
      default:
        return `bash: ${name}: command not found\r\n`;
    }
  }
}
