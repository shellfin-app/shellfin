// SHA-256 manifest of the built frontend (docs/ARCHITECTURE.md §11.3). Plain Node, no dependencies: the release build
// jobs run it on a runner that holds the signing keys, where no npm package is installed or executed.
//
// The release builds `dist/` once, in a job without secrets, and hands it to the signing jobs as an artifact. Those
// jobs verify every file against the manifest before `tauri build` embeds it, and again afterwards.
//
//   write   Hash every file below <dir> and write the manifest. Prints the SHA-256 of the manifest itself (the release
//           passes it to the build jobs as a job output, so the manifest cannot be swapped together with the files).
//     node scripts/dist-manifest.mjs write --dir dist --out dist.sha256
//
//   verify  Fail unless <dir> holds exactly the files of the manifest with exactly those hashes: a changed, missing or
//           extra file, a symlink or other non-regular file, a malformed manifest or (with --sha256) a manifest whose
//           own hash differs all fail.
//     node scripts/dist-manifest.mjs verify --dir dist --manifest dist.sha256 [--sha256 <hex>]
//
// Format: one line per file, `<sha256 hex>  <path>\n` (the `sha256sum` format, so `sha256sum -c` works inside <dir>),
// paths relative to <dir> with `/` separators, sorted by code unit. Paths that could be read differently on another
// platform are refused: every segment must match [A-Za-z0-9._@+~-]+ (so no backslash, space, control or non-ASCII
// character; the path must also be NFC), must not be `.`/`..`, end in `.` or be a reserved Windows device name
// (`con`, `nul.js`, ...), and no two paths may differ only in letter case (Windows and macOS would merge them).
// A repeated option fails.
import { Buffer } from "node:buffer";
import { createHash } from "node:crypto";
import { lstatSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import process from "node:process";
import { pathToFileURL } from "node:url";

const HEX64 = /^[0-9a-f]{64}$/;
const LINE = /^([0-9a-f]{64}) {2}(.+)$/;
/** Characters a path segment may contain (what Vite emits); everything else is refused. */
const SEGMENT = /^[A-Za-z0-9._@+~-]+$/;
/** Device names Windows reserves, also with an extension (`nul.txt`). */
const WINDOWS_RESERVED = /^(con|prn|aux|nul|com[0-9¹²³]|lpt[0-9¹²³])(\.|$)/i;

export function sha256Hex(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

/** Throws unless `path` is a portable relative path (see the header). */
export function checkPath(path) {
  if (typeof path !== "string" || path.length === 0) throw new Error("empty path");
  if (path.normalize("NFC") !== path) throw new Error(`path not in Unicode NFC: ${JSON.stringify(path)}`);
  for (const segment of path.split("/")) {
    if (segment === "" || segment === "." || segment === "..") {
      throw new Error(`path with an empty, '.' or '..' segment: ${JSON.stringify(path)}`);
    }
    if (!SEGMENT.test(segment)) throw new Error(`path with a character outside [A-Za-z0-9._@+~-]: ${JSON.stringify(path)}`);
    if (segment.endsWith(".")) throw new Error(`path segment ending in '.': ${JSON.stringify(path)}`);
    if (WINDOWS_RESERVED.test(segment)) throw new Error(`path with a reserved Windows name: ${JSON.stringify(path)}`);
  }
}

function sortEntries(entries) {
  return [...entries].sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
}

function checkEntries(entries) {
  const seen = new Map();
  for (const { path, sha256 } of entries) {
    checkPath(path);
    if (!HEX64.test(sha256)) throw new Error(`invalid SHA-256 for ${JSON.stringify(path)}`);
    const folded = path.toLowerCase();
    if (seen.has(folded)) {
      throw new Error(`paths differ only in letter case: ${JSON.stringify(seen.get(folded))} and ${JSON.stringify(path)}`);
    }
    seen.set(folded, path);
  }
}

/** Every regular file below `dir` as sorted `{ path, sha256 }`. Symlinks and other non-regular files throw. */
export function hashTree(dir) {
  const entries = [];
  const walk = (abs, rel) => {
    for (const ent of readdirSync(abs, { withFileTypes: true })) {
      const absChild = join(abs, ent.name);
      const relChild = rel === "" ? ent.name : `${rel}/${ent.name}`;
      const st = lstatSync(absChild);
      if (st.isDirectory()) walk(absChild, relChild);
      else if (st.isFile()) entries.push({ path: relChild, sha256: sha256Hex(readFileSync(absChild)) });
      else throw new Error(`not a regular file or directory: ${JSON.stringify(relChild)}`);
    }
  };
  if (!lstatSync(dir).isDirectory()) throw new Error(`not a directory: ${dir}`);
  walk(dir, "");
  const sorted = sortEntries(entries);
  checkEntries(sorted);
  return sorted;
}

export function formatManifest(entries) {
  const sorted = sortEntries(entries);
  checkEntries(sorted);
  if (sorted.length === 0) throw new Error("no files");
  return sorted.map((e) => `${e.sha256}  ${e.path}\n`).join("");
}

/** Parses a manifest written by formatManifest; anything else (order, duplicates, blank lines, CRLF) throws. */
export function parseManifest(text) {
  if (typeof text !== "string" || text.length === 0) throw new Error("empty manifest");
  if (!text.endsWith("\n")) throw new Error("manifest does not end with a newline");
  const entries = text
    .slice(0, -1)
    .split("\n")
    .map((line, i) => {
      const m = LINE.exec(line);
      if (!m) throw new Error(`malformed manifest line ${i + 1}`);
      return { sha256: m[1], path: m[2] };
    });
  checkEntries(entries);
  for (let i = 1; i < entries.length; i++) {
    if (!(entries[i - 1].path < entries[i].path)) throw new Error(`manifest not sorted or duplicate at line ${i + 1}`);
  }
  return entries;
}

/** Differences between the manifest and the files found, as messages (empty: identical). */
export function compareTrees(expected, actual) {
  const want = new Map(expected.map((e) => [e.path, e.sha256]));
  const have = new Map(actual.map((e) => [e.path, e.sha256]));
  const problems = [];
  for (const [path, sha] of want) {
    if (!have.has(path)) problems.push(`missing: ${path}`);
    else if (have.get(path) !== sha) problems.push(`changed: ${path}`);
  }
  for (const path of have.keys()) {
    if (!want.has(path)) problems.push(`extra: ${path}`);
  }
  return problems;
}

/** Writes the manifest of `dir` to `out` and returns the manifest's own SHA-256. */
export function writeManifest(dir, out) {
  const text = formatManifest(hashTree(dir));
  writeFileSync(out, text);
  return sha256Hex(Buffer.from(text, "utf8"));
}

/** Throws with every difference unless `dir` matches the manifest (and the manifest matches `manifestSha256`). */
export function verifyManifest(dir, manifestFile, manifestSha256) {
  const bytes = readFileSync(manifestFile);
  if (manifestSha256 !== undefined) {
    if (!HEX64.test(manifestSha256)) throw new Error("--sha256 is not a lowercase SHA-256 hex digest");
    if (sha256Hex(bytes) !== manifestSha256) throw new Error("the manifest's SHA-256 differs from the expected one");
  }
  const expected = parseManifest(bytes.toString("utf8"));
  const problems = compareTrees(expected, hashTree(dir));
  if (problems.length > 0) {
    throw new Error(`${dir} does not match the manifest (${problems.length}):\n  ${problems.join("\n  ")}`);
  }
  return expected.length;
}

export function parseArgs(argv) {
  const opts = {};
  for (let i = 0; i < argv.length; i += 2) {
    const key = argv[i];
    const value = argv[i + 1];
    if (!key?.startsWith("--") || value === undefined) throw new Error(`bad arguments near ${JSON.stringify(key)}`);
    if (key.slice(2) in opts) throw new Error(`${key} given twice`);
    opts[key.slice(2)] = value;
  }
  return opts;
}

function main(argv) {
  const [command, ...rest] = argv;
  const opts = parseArgs(rest);
  if (command === "write" && opts.dir && opts.out && Object.keys(opts).length === 2) {
    console.log(writeManifest(opts.dir, opts.out));
    return;
  }
  const verifyKeys = Object.keys(opts).every((k) => ["dir", "manifest", "sha256"].includes(k));
  if (command === "verify" && opts.dir && opts.manifest && verifyKeys) {
    const n = verifyManifest(opts.dir, opts.manifest, opts.sha256);
    console.log(`dist-manifest: ${opts.dir} matches the manifest (${n} files)`);
    return;
  }
  throw new Error(
    "usage: dist-manifest.mjs write --dir <dir> --out <file> | verify --dir <dir> --manifest <file> [--sha256 <hex>]",
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main(process.argv.slice(2));
  } catch (err) {
    console.error(`dist-manifest: ${err instanceof Error ? err.message : String(err)}`);
    process.exit(1);
  }
}
