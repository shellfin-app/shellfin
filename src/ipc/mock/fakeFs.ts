// In-memory remote file system for the MockBackend (docs/ARCHITECTURE.md §8.8). Mirrors the mockup tree and models
// the parts of §6 the frontend depends on: etags and `remoteChanged`, encodings, EOL, binary and oversized files,
// truncated listings.
import { IpcError } from "../call";
import type { DirListing, Eol, Etag, FileEntry, FileKind, SaveRequest } from "../types";
import { basename, dirname, joinPath } from "../../lib/remotePath";

interface Node {
  kind: FileKind;
  content?: string;
  mtime: number;
  permissions: number;
  /** Not decodable as text (`fileIsBinary`). */
  binary?: boolean;
  /** Declared encoding of the bytes on the server (default utf-8). A non-UTF-8 file needs an explicit encoding. */
  encoding?: string;
  eol?: Eol;
  mixedEol?: boolean;
  hasBom?: boolean;
  /** Pretend the file has this many bytes (oversized files without allocating them). */
  size?: number;
  /** Target of a symlink. */
  linkTo?: string;
}

/** Encoding labels the mock's "backend" understands (`encoding_rs` knows many more). */
const KNOWN_ENCODINGS: ReadonlySet<string> = new Set([
  "utf-8",
  "utf-16le",
  "utf-16be",
  "windows-1251",
  "koi8-u",
  "windows-1252",
]);

/** WHATWG label aliases the real backend resolves (`encoding_rs::Encoding::for_label`). */
const LABEL_ALIASES: Readonly<Record<string, string>> = {
  "iso-8859-1": "windows-1252",
  latin1: "windows-1252",
  l1: "windows-1252",
  "us-ascii": "windows-1252",
  utf8: "utf-8",
  "utf-16": "utf-16le",
};

function canonicalEncoding(label: string): string | null {
  const lower = label.trim().toLowerCase();
  const name = LABEL_ALIASES[lower] ?? lower;
  return KNOWN_ENCODINGS.has(name) ? name : null;
}

const COMPOSE = `services:
  web:
    image: app/web:2.14.0
    restart: unless-stopped
    ports:
      - "443:443"
    env_file: .env.production
    volumes:
      - ./nginx/nginx.conf:/etc/nginx/nginx.conf:ro
      - ./certs:/etc/ssl/app:ro
  worker:
    image: app/worker:2.14.0
    command: ["node", "dist/worker.js"]
    environment:
      QUEUE_CONCURRENCY: 8   # was 4
  redis:
    image: redis:7-alpine
    volumes:
      - redis-data:/data
volumes: { redis-data: {} }
`;

const NGINX = `worker_processes auto;
events { worker_connections 1024; }
http {
  include /etc/nginx/mime.types;
  server {
    listen 443 ssl;
    server_name app.example.com;
    location / { proxy_pass http://web:3000; }
  }
}
`;

const MIB = 1024 * 1024;

/** A stand-in for the sha256 of the bytes: stable for equal content, different otherwise. */
export function fakeSha(text: string): string {
  let a = 0x811c9dc5;
  let b = 0x01000193;
  for (let i = 0; i < text.length; i += 1) {
    a = Math.imul(a ^ text.charCodeAt(i), 16777619);
    b = Math.imul(b + text.charCodeAt(i), 2246822519) ^ (b >>> 13);
  }
  const hex = (n: number) => (n >>> 0).toString(16).padStart(8, "0");
  return (hex(a) + hex(b)).repeat(4);
}

const byteLength = (text: string) => new TextEncoder().encode(text).length;

export class FakeFs {
  private nodes = new Map<string, Node>();
  /** Directories whose listing is cut off (`truncated: true`). */
  private truncated = new Set<string>();
  private readonly nowSec: () => number;

  constructor(now = Date.now()) {
    const t = Math.floor(now / 1000) - 180;
    this.nowSec = () => Math.floor(Date.now() / 1000);
    const dir = (path: string) => this.nodes.set(path, { kind: "dir", mtime: t, permissions: 0o755 });
    const file = (path: string, content: string, permissions = 0o644) =>
      this.nodes.set(path, { kind: "file", content, mtime: t, permissions });

    dir("/");
    dir("/srv");
    dir("/srv/app");
    dir("/srv/app/certs");
    dir("/srv/app/logs");
    dir("/srv/app/nginx");
    file("/srv/app/nginx/nginx.conf", NGINX);
    file("/srv/app/nginx/mime.types", "types {\n  text/html html;\n  text/css css;\n}\n");
    file("/srv/app/docker-compose.yml", COMPOSE);
    file("/srv/app/.env.production", "NODE_ENV=production\n", 0o600);
    file("/srv/app/deploy.sh", "#!/bin/sh\nset -eu\ndocker compose pull\ndocker compose up -d\n", 0o755);
    file("/srv/app/README.md", "# app\n\nDeployment notes.\n");
    dir("/home");
    dir("/home/deploy");
  }

  // ---- scenario fixtures ----

  /** A binary file (`fileIsBinary`). */
  addBinary(path: string, size = 2048): void {
    this.nodes.set(path, { kind: "file", content: "\u0000\u0001\u0002", binary: true, mtime: this.nowSec() - 60, permissions: 0o644, size });
  }

  /** A file that is not UTF-8 on the server (`decodeFailed` until a matching encoding is chosen). */
  addEncoded(path: string, text: string, encoding: string): void {
    this.nodes.set(path, { kind: "file", content: text, encoding, mtime: this.nowSec() - 60, permissions: 0o644 });
  }

  /** A text file with CRLF (or mixed) line endings. */
  addEol(path: string, text: string, eol: Eol, mixedEol = false): void {
    this.nodes.set(path, { kind: "file", content: text, eol, mixedEol, mtime: this.nowSec() - 60, permissions: 0o644 });
  }

  /** A file bigger than the editor limit (`fileTooLarge`). */
  addHuge(path: string, size = 80 * MIB): void {
    this.nodes.set(path, { kind: "file", content: "", size, mtime: this.nowSec() - 60, permissions: 0o644 });
  }

  /** A folder with `count` generated files whose listing is cut off like a 100 000+ entry directory (§6.2). */
  addCrowdedDir(path: string, count: number): void {
    this.nodes.set(path, { kind: "dir", mtime: this.nowSec() - 60, permissions: 0o755 });
    for (let i = 0; i < count; i += 1) {
      this.nodes.set(`${path}/item-${String(i).padStart(5, "0")}.log`, { kind: "file", content: "x\n", mtime: this.nowSec() - 60, permissions: 0o644 });
    }
    this.truncated.add(path);
  }

  /** A file whose name contains U+FFFD (the real name is not valid UTF-8). */
  addLossyName(dirPath: string, name: string): void {
    this.nodes.set(joinPath(dirPath, name), { kind: "file", content: "?\n", mtime: this.nowSec() - 60, permissions: 0o644 });
  }

  /** A symlink to a directory or file. */
  addLink(path: string, target: string): void {
    this.nodes.set(path, { kind: "symlink", linkTo: target, mtime: this.nowSec() - 60, permissions: 0o777 });
  }

  /** Re-points a symlink to another file (someone else changed the link). */
  repointLink(path: string, target: string): void {
    const node = this.nodes.get(path);
    if (node?.kind === "symlink") node.linkTo = target;
  }

  /** The path a read or write really uses: symlinks are followed (like the backend's `realpath`). */
  resolve(path: string): string {
    let p = path;
    for (let i = 0; i < 16; i += 1) {
      const node = this.nodes.get(p);
      if (node?.kind !== "symlink" || !node.linkTo) return p;
      p = node.linkTo;
    }
    return p;
  }

  /** Someone else changed the file on the server (new content, newer mtime). */
  externalEdit(path: string, text: string): void {
    const node = this.nodes.get(this.resolve(path));
    if (!node) return;
    node.content = text;
    node.mtime = Math.max(node.mtime + 1, this.nowSec());
  }

  has(path: string): boolean {
    return this.nodes.has(path);
  }

  // ---- queries ----

  homeDir(): string {
    return "/home/deploy";
  }

  private sizeOf(node: Node): number {
    return node.size ?? (node.kind === "file" ? byteLength(node.content ?? "") : 0);
  }

  private linkTargetKind(node: Node): FileKind | null {
    if (node.kind !== "symlink" || !node.linkTo) return null;
    return this.nodes.get(node.linkTo)?.kind ?? null;
  }

  private entry(path: string, node: Node): FileEntry {
    const name = basename(path);
    return {
      name,
      path,
      kind: node.kind,
      linkTargetKind: this.linkTargetKind(node),
      size: node.kind === "dir" ? null : this.sizeOf(node),
      mtime: node.mtime,
      permissions: node.permissions,
      uid: 1000,
      gid: 1000,
      owner: "alice",
      group: "alice",
      lossyName: name.includes("�"),
    };
  }

  private notFound(path: string): IpcError {
    return new IpcError({ code: "remoteNotFound", params: { path } });
  }

  stat(path: string): FileEntry {
    const node = this.nodes.get(path);
    if (!node) throw this.notFound(path);
    return this.entry(path, node);
  }

  list(path: string): DirListing {
    const node = this.nodes.get(path);
    if (!node) throw this.notFound(path);
    if (node.kind !== "dir") throw new IpcError({ code: "notADirectory", params: { path } });
    const entries: FileEntry[] = [];
    for (const [p, n] of this.nodes) {
      if (p !== path && dirname(p) === path) entries.push(this.entry(p, n));
    }
    return { path, entries, truncated: this.truncated.has(path) };
  }

  etag(path: string): Etag {
    const node = this.nodes.get(this.resolve(path));
    if (!node) throw this.notFound(path);
    return { size: this.sizeOf(node), mtime: node.mtime, sha256: fakeSha(node.content ?? "") };
  }

  /** §6.3: size limit, binary check, encoding rules. */
  read(
    path: string,
    encodingLabel: string | null,
    limitBytes: number,
  ): { text: string; encoding: string; hasBom: boolean; eol: Eol; mixedEol: boolean; entry: FileEntry; etag: Etag; resolvedPath?: string } {
    const real = this.resolve(path);
    const node = this.nodes.get(real);
    if (!node) throw this.notFound(path);
    if (node.kind === "dir") throw new IpcError({ code: "isADirectory", params: { path } });
    const size = this.sizeOf(node);
    if (size > limitBytes) throw new IpcError({ code: "fileTooLarge", params: { path, size, limit: limitBytes } });
    if (node.binary) throw new IpcError({ code: "fileIsBinary", params: { path } });
    const explicit = encodingLabel === null ? null : canonicalEncoding(encodingLabel);
    if (encodingLabel !== null && explicit === null) {
      throw new IpcError({ code: "unsupportedEncoding", params: { encoding: encodingLabel } });
    }
    const declared = node.encoding ?? "utf-8";
    if (encodingLabel === null && declared !== "utf-8" && !node.hasBom) {
      throw new IpcError({ code: "decodeFailed", params: { path, encoding: "utf-8" } });
    }
    return {
      text: node.content ?? "",
      encoding: explicit ?? declared,
      hasBom: node.hasBom === true,
      eol: node.eol ?? "lf",
      mixedEol: node.mixedEol === true,
      entry: this.entry(real, node),
      etag: this.etag(real),
      ...(real !== path ? { resolvedPath: real } : {}),
    };
  }

  /**
   * §6.4: with `expected` and without `force`, a changed server file is `remoteChanged` (unless only the mtime moved);
   * so is `expected: null` onto an existing file, and a path that now resolves to another file than `expectedTarget`.
   */
  write(path: string, req: SaveRequest): FileEntry {
    const real = this.resolve(path);
    const prev = this.nodes.get(real);
    const target = real !== path ? { target: real } : {};
    if (!req.force) {
      const stat = prev ? this.etag(real) : { size: 0, mtime: 0, sha256: "" };
      const changed = (): never => {
        throw new IpcError({ code: "remoteChanged", params: { path, size: stat.size, mtime: stat.mtime, ...target } });
      };
      if (req.expectedTarget !== undefined && req.expectedTarget !== real) changed();
      if (!prev) {
        // The backend reports a file that vanished since it was opened as remoteChanged{0, 0} (ARCHITECTURE §6.4).
        if (req.expected !== null) changed();
      } else if (req.expected === null) {
        changed();
      } else {
        const sameStat = stat.size === req.expected.size && stat.mtime === req.expected.mtime;
        if (!sameStat && stat.sha256 !== req.expected.sha256) changed();
      }
    }
    const node: Node = {
      kind: "file",
      content: req.text,
      mtime: Math.max((prev?.mtime ?? 0) + 1, this.nowSec()),
      permissions: prev?.permissions ?? 0o644,
      eol: req.eol,
      hasBom: req.hasBom,
      ...(req.encoding !== "utf-8" ? { encoding: req.encoding } : {}),
    };
    this.nodes.set(real, node);
    return this.entry(real, node);
  }

  /** Creates (or replaces) a file as an upload would. */
  put(path: string, size: number, overwrite: boolean): FileEntry {
    if (this.nodes.has(path) && !overwrite) throw new IpcError({ code: "remoteExists", params: { path } });
    const node: Node = { kind: "file", content: "", size, mtime: this.nowSec(), permissions: 0o644 };
    this.nodes.set(path, node);
    return this.entry(path, node);
  }

  create(path: string, kind: "file" | "dir"): FileEntry {
    if (this.nodes.has(path)) throw new IpcError({ code: "remoteExists", params: { path } });
    if (this.nodes.get(dirname(path))?.kind !== "dir") throw this.notFound(dirname(path));
    const node: Node = { kind, content: kind === "file" ? "" : undefined, mtime: this.nowSec(), permissions: kind === "dir" ? 0o755 : 0o644 };
    this.nodes.set(path, node);
    return this.entry(path, node);
  }

  /** Renames or moves; a directory takes its whole subtree along. */
  rename(from: string, to: string): FileEntry {
    const node = this.nodes.get(from);
    if (!node) throw this.notFound(from);
    if (this.nodes.has(to)) throw new IpcError({ code: "remoteExists", params: { path: to } });
    if (this.nodes.get(dirname(to))?.kind !== "dir") throw this.notFound(dirname(to));
    const moved: [string, Node][] = [];
    for (const [p, n] of this.nodes) {
      if (p === from || p.startsWith(`${from}/`)) moved.push([p, n]);
    }
    for (const [p] of moved) this.nodes.delete(p);
    for (const [p, n] of moved) this.nodes.set(`${to}${p.slice(from.length)}`, n);
    return this.entry(to, node);
  }

  remove(path: string): void {
    const node = this.nodes.get(path);
    if (!node) throw this.notFound(path);
    if (node.kind === "dir" && this.list(path).entries.length > 0) {
      throw new IpcError({ code: "directoryNotEmpty", params: { path } });
    }
    this.nodes.delete(path);
  }

  join(dir: string, name: string): string {
    return joinPath(dir, name);
  }
}
