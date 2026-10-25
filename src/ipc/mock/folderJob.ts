// MockBackend: one folder download job (docs/ARCHITECTURE.md §6.6 "Folders"), modelled on the backend contract:
// scan (links, specials, invalid and lossy names, name collisions, hard limits) -> `scanned` -> optional decisions
// (large tree, root exists, low space) -> copy with deferred conflicts and item failures -> `done` with a summary.
// Same event caps as the backend: at most 1 000 `itemIssue`s, at most 50 conflict samples, progress in batches.
import type {
  AppError,
  ConflictPolicy,
  ConflictSample,
  DecisionRequest,
  SkipCounts,
  SkipReason,
  TransferDecision,
  TransferEvent,
} from "../types";
import type { FakeFs } from "./fakeFs";

export const MOCK_TREE_LIMITS = { depth: 64, entries: 200_000 } as const;
/** Soft threshold: above it the job asks "Copy N files (X)?" after the scan. */
export const LARGE_TREE = { files: 10_000, bytes: 10 * 1024 ** 3 } as const;
const MAX_ISSUES = 1000;
const MAX_SAMPLE = 50;
const SCAN_BATCH = 2000;
const LOSSY = String.fromCodePoint(0xfffd);

/** What the mock pretends exists on the local disk (the folder picker's parent). */
export interface MockLocalDisk {
  /** Entries in the chosen parent folder, by name. `files` (merge targets) by path relative to that folder. */
  folders: Record<string, { isDir: boolean; files?: Record<string, { size: number; mtime: number }> }>;
  freeBytes: number;
  /** Relative paths whose copy fails once with a retryable `permissionDenied` (a retry then succeeds). */
  failing: string[];
}

export interface FolderJobContext {
  fs: FakeFs;
  local: MockLocalDisk;
  pause(): Promise<void>;
  /** The session still exists (events are delivered). */
  alive(): boolean;
  /** The session is connected (copying can go on). */
  connected(): boolean;
  onConflict(): ConflictPolicy;
  /** The job finished; `retained` = keep it for `transfer_retry`. */
  finished(retained: boolean): void;
}

interface Item {
  id: number;
  relPath: string;
  remotePath: string;
  name: string;
  size: number;
  mtime: number;
  status: "pending" | "done" | "failed" | "skipped" | "conflict";
}

type ConflictDecision = "replaceAll" | "skipAll" | "replaceOlder";
const CONFLICT_DECISIONS: readonly TransferDecision[] = ["replaceAll", "skipAll", "replaceOlder"];

const emptyCounts = (): SkipCounts => ({ symlink: 0, special: 0, invalidName: 0, lossyName: 0, nameCollision: 0, unknownType: 0, exists: 0 });

/** Roughly `sanitize_local_name` (`sftp/names.rs`): Windows-reserved characters and device names, trailing dots. */
export function mockLocalName(name: string): string {
  const RESERVED = '<>:"|?*\\/';
  const replaced = Array.from(name, (ch) => {
    const code = ch.codePointAt(0) ?? 0;
    return RESERVED.includes(ch) || code < 0x20 || (code >= 0x7f && code <= 0x9f) ? "_" : ch;
  }).join("");
  let out = replaced.replace(/[. ]+$/, "");
  const stem = out.split(".")[0]?.toUpperCase() ?? "";
  if (/^(CON|PRN|AUX|NUL|COM[0-9]|LPT[0-9])$/.test(stem)) out = `${out.slice(0, stem.length)}_${out.slice(stem.length)}`;
  return out === "" ? "_" : out;
}

function invalidName(name: string): boolean {
  return name === "" || name === "." || name === ".." || name.includes("/") || name.includes("\u0000");
}

export class MockFolderJob {
  cancelled = false;
  done = false;
  private items: Item[] = [];
  private dirs = 0;
  private skipped = emptyCounts();
  private failed = 0;
  private renamed = 0;
  private issuesSent = 0;
  private bytesDone = 0;
  private targets: Record<string, { size: number; mtime: number }> = {};
  private conflictPolicy: ConflictDecision | null = null;
  /** The `round` of the `conflicts` events (a conflict decision ends it, as `requeue_conflicts` does). */
  private conflictRound = 0;
  private waiter: { accept: readonly TransferDecision[]; resolve: (d: TransferDecision | null) => void } | null = null;
  /** The pre-copy decision chain (stage 0 large tree, 1 root exists, 2 low space). */
  private pre: { stage: number; accept: readonly TransferDecision[]; resolve: (outcome: "go" | "cancel" | "skipped") => void } | null = null;
  /** A retry asks only about free space. */
  private retrying = false;

  constructor(
    readonly id: string,
    readonly sessionId: string,
    /** Remote folder name (server text), as `started.name`. */
    readonly name: string,
    private readonly remoteRoot: string,
    private localName: string,
    private readonly onEvent: (event: TransferEvent) => void,
    private readonly ctx: FolderJobContext,
  ) {}

  /** `transfer_retry`: a new job with the same manifest; failed and conflicting items run again. */
  retry(id: string, onEvent: (event: TransferEvent) => void, overwrite: boolean, ctx: FolderJobContext): MockFolderJob {
    const next = new MockFolderJob(id, this.sessionId, this.name, this.remoteRoot, this.localName, onEvent, ctx);
    next.items = this.items.map((i) => ({ ...i, status: i.status === "failed" || i.status === "conflict" ? "pending" : i.status }));
    next.dirs = this.dirs;
    next.skipped = { ...this.skipped };
    next.renamed = this.renamed;
    next.bytesDone = this.bytesDone;
    next.targets = this.targets;
    next.conflictPolicy = overwrite ? "replaceAll" : this.conflictPolicy;
    return next;
  }

  /**
   * `transfer_decide`; a decision that does not fit what the job waits for is ignored. The decisions before the copy
   * run as a synchronous chain: the next `awaitingDecision` (large tree -> root exists -> low space) is emitted INSIDE
   * this call, before the command resolves, so the frontend's answer/next-request race is reproducible.
   */
  decide(decision: TransferDecision): void {
    const pre = this.pre;
    if (pre) {
      if (!pre.accept.includes(decision)) return;
      if (decision === "cancel") {
        this.pre = null;
        pre.resolve("cancel");
        return;
      }
      this.applyPre(pre.stage, decision);
      this.stepPre(pre.stage + 1);
      return;
    }
    if (CONFLICT_DECISIONS.includes(decision)) {
      if (this.conflictPolicy !== null || !this.items.some((i) => i.status === "conflict")) return;
      this.conflictPolicy = decision as ConflictDecision;
      // Same as the backend: the decision ends the conflict round.
      this.conflictRound += 1;
    }
    const w = this.waiter;
    if (w && w.accept.includes(decision)) {
      this.waiter = null;
      w.resolve(decision);
    }
  }

  cancel(): void {
    this.cancelled = true;
    const pre = this.pre;
    this.pre = null;
    pre?.resolve("cancel");
    const w = this.waiter;
    this.waiter = null;
    w?.resolve(null);
  }

  private emit(event: TransferEvent): void {
    if (this.ctx.alive()) this.onEvent(event);
  }

  private waitFor(accept: readonly TransferDecision[]): Promise<TransferDecision | null> {
    if (this.cancelled) return Promise.resolve(null);
    return new Promise((resolve) => {
      this.waiter = { accept, resolve };
    });
  }

  /** The pre-copy chain; resolves when the copy may start, was cancelled or ends `skipped`. */
  private runPre(): Promise<"go" | "cancel" | "skipped"> {
    if (this.cancelled) return Promise.resolve("cancel");
    return new Promise((resolve) => {
      this.pre = { stage: -1, accept: [], resolve };
      this.stepPre(0);
    });
  }

  /** Ask the question of the first stage from `stage` on that applies, or finish the chain. */
  private stepPre(stage: number): void {
    const pre = this.pre;
    if (!pre) return;
    const ask = (s: number, request: DecisionRequest, accept: readonly TransferDecision[]) => {
      this.pre = { stage: s, accept, resolve: pre.resolve };
      this.emit({ type: "awaitingDecision", transferId: this.id, request });
    };
    const done = (outcome: "go" | "cancel" | "skipped") => {
      this.pre = null;
      pre.resolve(outcome);
    };
    const bytes = this.totalBytes();
    if (stage <= 0 && !this.retrying && (this.items.length > LARGE_TREE.files || bytes > LARGE_TREE.bytes)) {
      return ask(0, { kind: "largeTree", files: this.items.length, bytes }, ["proceed", "cancel"]);
    }
    const existing = this.ctx.local.folders[this.localName];
    if (stage <= 1 && !this.retrying && existing) {
      const policy = this.ctx.onConflict();
      if (policy === "skip") return done("skipped");
      if (policy === "ask") {
        const request: DecisionRequest = { kind: "rootExists", name: this.localName, isDir: existing.isDir };
        return ask(1, request, existing.isDir ? ["merge", "keepBoth", "cancel"] : ["keepBoth", "cancel"]);
      }
      this.applyPre(1, existing.isDir ? "merge" : "keepBoth");
    }
    const needed = this.items.filter((i) => i.status === "pending").reduce((n, i) => n + i.size, 0);
    if (stage <= 2 && needed > this.ctx.local.freeBytes) {
      return ask(2, { kind: "lowSpace", needed, available: this.ctx.local.freeBytes }, ["proceed", "cancel"]);
    }
    done("go");
  }

  /** The effect of a pre-copy answer (root exists: merge or keep both). */
  private applyPre(stage: number, decision: TransferDecision): void {
    if (stage !== 1) return;
    const existing = this.ctx.local.folders[this.localName];
    if (decision === "merge") this.targets = { ...(existing?.files ?? {}) };
    else if (decision === "keepBoth") {
      let n = 2;
      while (this.ctx.local.folders[`${this.localName} (${n})`] && n < 99) n += 1;
      this.localName = `${this.localName} (${n})`;
    }
  }

  private issue(itemId: number | null, relPath: string, issue: Extract<TransferEvent, { type: "itemIssue" }>["issue"]): void {
    if (issue.kind === "skipped") this.skipped[issue.reason] += 1;
    else this.failed += 1;
    if (this.issuesSent >= MAX_ISSUES) return;
    this.issuesSent += 1;
    this.emit({ type: "itemIssue", transferId: this.id, itemId, relPath, issue });
  }

  private skip(itemId: number | null, relPath: string, reason: SkipReason): void {
    this.issue(itemId, relPath, { kind: "skipped", reason });
  }

  private finish(event: TransferEvent, retained: boolean): void {
    this.done = true;
    this.ctx.finished(retained);
    this.emit(event);
  }

  private cancelled_(): void {
    this.finish({ type: "cancelled", transferId: this.id }, false);
  }

  private fail(error: AppError, retryable: boolean): void {
    this.finish({ type: "failed", transferId: this.id, error, retryable }, retryable);
  }

  private totalBytes(): number {
    return this.items.reduce((n, i) => n + i.size, 0);
  }

  private progress(current: string | undefined): void {
    this.emit({
      type: "progress",
      transferId: this.id,
      bytes: this.bytesDone,
      total: this.totalBytes(),
      filesDone: this.items.filter((i) => i.status === "done").length,
      filesTotal: this.items.length,
      ...(current !== undefined ? { current } : {}),
      failed: this.failed,
      skipped: { ...this.skipped },
    });
  }

  /** The walk; `false` when the job ended (limit, cancel). */
  private async scan(): Promise<boolean> {
    const fs = this.ctx.fs;
    if (fs.isTruncated(this.remoteRoot)) {
      this.fail({ code: "treeTooLarge", params: { limit: "directory", value: 100_000 } }, false);
      return false;
    }
    const entries = fs.walk(this.remoteRoot);
    const skippedDirs: string[] = [];
    const names = new Map<string, Set<string>>();
    let seen = 0;
    for (const e of entries) {
      if (this.cancelled) {
        this.cancelled_();
        return false;
      }
      if (skippedDirs.some((d) => e.relPath.startsWith(`${d}/`))) continue;
      seen += 1;
      if (e.depth > MOCK_TREE_LIMITS.depth) {
        this.fail({ code: "treeTooLarge", params: { limit: "depth", value: e.depth } }, false);
        return false;
      }
      if (seen > MOCK_TREE_LIMITS.entries) {
        this.fail({ code: "treeTooLarge", params: { limit: "entries", value: seen } }, false);
        return false;
      }
      if (e.kind === "dir" && fs.isTruncated(e.path)) {
        this.fail({ code: "treeTooLarge", params: { limit: "directory", value: 100_000 } }, false);
        return false;
      }
      const parent = e.relPath.slice(0, Math.max(0, e.relPath.length - e.name.length - 1));
      let reason: SkipReason | null = null;
      if (invalidName(e.name)) reason = "invalidName";
      else if (e.name.includes(LOSSY)) reason = "lossyName";
      else if (e.kind === "symlink") reason = "symlink";
      else if (e.kind === "other") reason = "special";
      else {
        const local = mockLocalName(e.name);
        const folded = local.toLowerCase();
        const set = names.get(parent) ?? new Set<string>();
        names.set(parent, set);
        if (set.has(folded)) reason = "nameCollision";
        else {
          set.add(folded);
          if (local !== e.name) this.renamed += 1;
        }
      }
      if (reason !== null) {
        if (e.kind === "dir") skippedDirs.push(e.relPath);
        this.skip(null, e.relPath, reason);
      } else if (e.kind === "dir") {
        this.dirs += 1;
      } else {
        this.items.push({ id: this.items.length, relPath: e.relPath, remotePath: e.path, name: e.name, size: e.size, mtime: e.mtime, status: "pending" });
      }
      if (seen % SCAN_BATCH === 0) {
        this.emit({ type: "scanProgress", transferId: this.id, files: this.items.length, dirs: this.dirs, bytes: this.totalBytes() });
        await this.ctx.pause();
      }
    }
    return true;
  }

  private conflictSample(): ConflictSample[] {
    return this.items
      .filter((i) => i.status === "conflict")
      .slice(0, MAX_SAMPLE)
      .map((i) => {
        const target = this.targets[i.relPath];
        return { itemId: i.id, relPath: i.relPath, source: { size: i.size, mtime: i.mtime }, target: { size: target?.size ?? null, mtime: target?.mtime ?? null } };
      });
  }

  private copy(item: Item): void {
    const failAt = this.ctx.local.failing.indexOf(item.relPath);
    if (failAt >= 0) {
      this.ctx.local.failing.splice(failAt, 1);
      item.status = "failed";
      this.issue(item.id, item.relPath, { kind: "failed", error: { code: "permissionDenied", params: { path: item.remotePath } }, retryable: true });
      return;
    }
    item.status = "done";
    this.bytesDone += item.size;
    this.targets[item.relPath] = { size: item.size, mtime: item.mtime };
  }

  private apply(item: Item, decision: ConflictDecision): void {
    const target = this.targets[item.relPath];
    const older = target !== undefined && target.mtime < item.mtime;
    if (decision === "replaceAll" || (decision === "replaceOlder" && older)) this.copy(item);
    else {
      item.status = "skipped";
      this.skip(item.id, item.relPath, "exists");
    }
  }

  private copyOrDefer(item: Item): void {
    if (this.targets[item.relPath] === undefined) return this.copy(item);
    const policy = this.ctx.onConflict();
    if (policy === "overwrite") return this.copy(item);
    if (policy === "skip") return this.apply(item, "skipAll");
    if (this.conflictPolicy !== null) return this.apply(item, this.conflictPolicy);
    item.status = "conflict";
    const count = this.items.filter((i) => i.status === "conflict").length;
    this.emit({ type: "conflicts", transferId: this.id, count, sample: this.conflictSample(), round: this.conflictRound });
  }

  async run(retry: boolean): Promise<void> {
    // A retry knows its manifest: `total` = the remaining bytes, no scan and no `scanned`, straight to the copy.
    const remaining = this.items.filter((i) => i.status === "pending").reduce((n, i) => n + i.size, 0);
    this.emit({ type: "started", transferId: this.id, name: this.name, direction: "download", total: retry ? remaining : null, kind: "folder" });
    await this.ctx.pause();
    if (!retry && !(await this.scan())) return;
    if (this.cancelled) return this.cancelled_();
    const bytes = this.totalBytes();

    if (!retry) {
      this.emit({ type: "scanned", transferId: this.id, files: this.items.length, dirs: this.dirs, bytes, skipped: { ...this.skipped }, renamed: this.renamed });
    }
    this.retrying = retry;
    const outcome = await this.runPre();
    if (outcome === "skipped") return this.finish({ type: "skipped", transferId: this.id }, false);
    if (outcome === "cancel") return this.cancelled_();

    const pending = this.items.filter((i) => i.status === "pending");
    const every = Math.max(1, Math.ceil(pending.length / 25));
    let sinceProgress = 0;
    this.progress(undefined);
    for (const item of pending) {
      if (this.cancelled) return this.cancelled_();
      if (!this.ctx.connected()) return this.fail({ code: "disconnected", params: { reason: "connectionReset", message: null } }, true);
      this.copyOrDefer(item);
      sinceProgress += 1;
      if (sinceProgress >= every) {
        sinceProgress = 0;
        this.progress(item.name);
        await this.ctx.pause();
      }
    }
    // Everything else is copied: wait for the answer to the deferred conflicts (no more progress meanwhile).
    while (this.items.some((i) => i.status === "conflict")) {
      const decision = this.conflictPolicy ?? (await this.waitFor(CONFLICT_DECISIONS));
      if (this.cancelled || decision === null) return this.cancelled_();
      for (const item of this.items.filter((i) => i.status === "conflict")) this.apply(item, decision as ConflictDecision);
    }
    this.progress(undefined);
    await this.ctx.pause();
    if (this.cancelled) return this.cancelled_();
    // The result is on the "local disk" now: a second download of the same folder finds it.
    this.ctx.local.folders[this.localName] = { isDir: true, files: { ...this.targets } };
    const filesDone = this.items.filter((i) => i.status === "done").length;
    const retryable = this.failed > 0;
    this.finish(
      {
        type: "done",
        transferId: this.id,
        bytes: this.bytesDone,
        entry: null,
        summary: { localName: this.localName, filesDone, skipped: { ...this.skipped }, failed: this.failed, renamed: this.renamed, retryable },
      },
      retryable,
    );
  }
}
