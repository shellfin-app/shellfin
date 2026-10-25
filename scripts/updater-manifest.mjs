// Updater manifests (docs/ARCHITECTURE.md §9.8, §11.3). Plain Node, no dependencies, so the release workflow can run
// it without `npm ci`. Two commands:
//
//   assemble  Build the release's `latest.json` (Tauri static updater format) from the signed updater artifacts of
//             the build jobs. Checks that every platform has exactly one artifact and that each `.sig` was made with
//             the release key (key id of `plugins.updater.pubkey`) for this version and file.
//     node scripts/updater-manifest.mjs assemble --dir <artifacts> --repo <owner/name> --tag <vX.Y.Z>
//          --conf src-tauri/tauri.conf.json --changelog CHANGELOG.md --out latest.json
//
//   channels  Decide which update manifests a PUBLISHED release updates and write them: stable.json (read by the app)
//             and experimental.json (its legacy mirror, see MANIFEST_FILES). Only a stable release is accepted (no
//             SemVer pre-release part and not marked as a pre-release on GitHub); a pre-release fails. A file is only
//             written when the new version is SemVer-greater than the one it holds, so republishing an old release
//             can never downgrade anyone.
//     node scripts/updater-manifest.mjs channels --manifest latest.json --tag <vX.Y.Z> --prerelease <true|false>
//          --listed <file: asset names of the `updater` release, one per line; empty when it does not exist>
//          --current <dir with the downloaded stable.json / experimental.json> --out <dir>
//     A file the listing names must have been downloaded and be a readable manifest, else the command fails (see
//     currentManifests). Prints the written file names, one per line.
//
//   notes     Write the CHANGELOG section of a version as the GitHub release notes (nothing when it has none).
//     node scripts/updater-manifest.mjs notes --changelog CHANGELOG.md --tag <vX.Y.Z> --out release-notes.md
import { Buffer } from "node:buffer";
import { existsSync, mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { basename, join } from "node:path";
import process from "node:process";
import { pathToFileURL } from "node:url";

/** Updater platform keys -> artifact file name pattern (tauri bundler names, `createUpdaterArtifacts: true`). */
export const PLATFORMS = {
  "windows-x86_64": /_x64-setup\.exe$/,
  "windows-aarch64": /_arm64-setup\.exe$/,
  "darwin-aarch64": /\.app\.tar\.gz$/,
  "linux-x86_64": /_amd64\.AppImage$/,
};

/**
 * The update manifests every stable release writes: `stable.json`, the one the app reads, and `experimental.json`, a
 * legacy mirror of it. Every install of the former experimental channel still reads `experimental.json` until it
 * updates: the 0.2.0-experimental.1 build, and 0.1.0/0.1.1 installs whose update channel setting is "Experimental".
 * So it must keep receiving every stable release and must never be deleted.
 */
export const MANIFEST_FILES = ["stable.json", "experimental.json"];
/** Longest release notes put into a manifest. The app shows at most 10 000 characters; the margin covers the
 * markers its escaping adds for invisible characters, so the final release-page link stays visible. */
export const MAX_NOTES_CHARS = 9500;
/** Asset names GitHub serves unchanged (no spaces or characters it would rewrite). */
const SAFE_ASSET = /^[A-Za-z0-9][A-Za-z0-9._+-]*$/;

// ---- SemVer (2.0.0 precedence) --------------------------------------------------------------------------------

const SEMVER =
  /^v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$/;

export function parseSemver(text) {
  const m = SEMVER.exec(String(text ?? "").trim());
  if (!m) return null;
  return {
    major: Number(m[1]),
    minor: Number(m[2]),
    patch: Number(m[3]),
    pre: m[4] ? m[4].split(".") : [],
  };
}

function compareIdentifiers(a, b) {
  const an = /^\d+$/.test(a);
  const bn = /^\d+$/.test(b);
  if (an && bn) return Math.sign(Number(a) - Number(b));
  if (an) return -1;
  if (bn) return 1;
  return a < b ? -1 : a > b ? 1 : 0;
}

/** -1, 0 or 1 by SemVer precedence (build metadata ignored). Throws on invalid input. */
export function compareSemver(a, b) {
  const x = parseSemver(a);
  const y = parseSemver(b);
  if (!x || !y) throw new Error(`not a SemVer version: ${x ? b : a}`);
  for (const k of ["major", "minor", "patch"]) if (x[k] !== y[k]) return Math.sign(x[k] - y[k]);
  if (x.pre.length === 0 || y.pre.length === 0) return Math.sign(y.pre.length - x.pre.length);
  for (let i = 0; i < Math.max(x.pre.length, y.pre.length); i++) {
    if (x.pre[i] === undefined) return -1;
    if (y.pre[i] === undefined) return 1;
    const c = compareIdentifiers(x.pre[i], y.pre[i]);
    if (c !== 0) return c;
  }
  return 0;
}

// ---- minisign (Tauri's updater key and `.sig` files) ----------------------------------------------------------

function decodeBase64Text(b64, what) {
  const text = Buffer.from(String(b64).trim(), "base64").toString("utf8");
  if (!text.startsWith("untrusted comment:")) throw new Error(`${what} is not a base64-encoded minisign file`);
  return text.split(/\r?\n/);
}

/** Key id (16 hex digits, as minisign prints it) of a Tauri updater public key (`plugins.updater.pubkey`). */
export function keyIdOfPubkey(pubkeyB64) {
  const lines = decodeBase64Text(pubkeyB64, "the public key");
  const raw = Buffer.from(lines[1] ?? "", "base64");
  if (raw.length !== 42 || raw.toString("latin1", 0, 2) !== "Ed") throw new Error("malformed minisign public key");
  return Buffer.from(raw.subarray(2, 10)).reverse().toString("hex").toUpperCase();
}

/** Key id and trusted comment of a Tauri `.sig` file's content. */
export function parseSignature(sigB64) {
  const lines = decodeBase64Text(sigB64, "the signature");
  const raw = Buffer.from(lines[1] ?? "", "base64");
  const alg = raw.toString("latin1", 0, 2);
  if (raw.length !== 74 || (alg !== "ED" && alg !== "Ed")) throw new Error("malformed minisign signature");
  const trusted = lines[2] ?? "";
  if (!trusted.startsWith("trusted comment: ")) throw new Error("minisign signature without a trusted comment");
  const fields = Object.fromEntries(
    trusted
      .slice("trusted comment: ".length)
      .split("\t")
      .map((f) => [f.slice(0, f.indexOf(":")), f.slice(f.indexOf(":") + 1)]),
  );
  return { keyId: Buffer.from(raw.subarray(2, 10)).reverse().toString("hex").toUpperCase(), fields };
}

// ---- assemble -------------------------------------------------------------------------------------------------

/** The `## [version]` section of a Keep-a-Changelog file, without its heading; `null` when absent. */
export function notesFromChangelog(changelog, version) {
  const lines = String(changelog ?? "").split(/\r?\n/);
  const start = lines.findIndex((l) => l.startsWith(`## [${version}]`));
  if (start < 0) return null;
  const rest = lines.slice(start + 1);
  const end = rest.findIndex((l) => l.startsWith("## "));
  const text = (end < 0 ? rest : rest.slice(0, end)).join("\n").trim();
  return text.length > 0 ? text : null;
}

/** Length in code points (what MAX_NOTES_CHARS counts). */
const codePoints = (text) => Array.from(text).length;

/** A top-level heading (`#` to `###`, as the app's notes parser reads them) or a top-level `- ` / `* ` list item. */
const BLOCK_START = /^(?:#{1,3}(?: |$)|[-*] )/;
const HEADING = /^#{1,3}(?: |$)/;

/**
 * Release notes bounded to `max` code points. Notes within the limit come back unchanged. Longer notes are cut after
 * the last whole top-level block that fits (a `- ` item with its indented continuation lines, or a heading with what
 * follows it up to the next block), trailing headings without items are dropped, and a final paragraph links to the
 * full notes on `releasePage`, so that clients of every version (they render the notes as they come) show where the
 * rest is. When not even the first block fits, the text is cut hard and gets the same link.
 */
export function truncateNotes(text, max, releasePage) {
  if (codePoints(text) <= max) return text;
  const lf = text.replace(/\r\n?/g, "\n");
  if (codePoints(lf) <= max) return lf;
  const footer = `\n\n[Full release notes on GitHub](${releasePage})`;
  const budget = max - codePoints(footer);
  if (budget < 2) throw new Error("release notes limit too small for the release page link");
  const blocks = [];
  for (const line of lf.split("\n")) {
    if (BLOCK_START.test(line) || blocks.length === 0) blocks.push([line]);
    else blocks[blocks.length - 1].push(line);
  }
  const kept = [];
  let used = 0;
  for (const block of blocks) {
    // Blocks keep their trailing blank lines, so joining them gives back the original text.
    const chunk = block.join("\n");
    const cost = (kept.length > 0 ? 1 : 0) + codePoints(chunk.trimEnd());
    if (used + cost > budget) break;
    kept.push({ chunk, headingOnly: HEADING.test(block[0]) && block.slice(1).every((l) => l.trim() === "") });
    used += (kept.length > 1 ? 1 : 0) + codePoints(chunk);
  }
  while (kept.length > 0 && kept[kept.length - 1].headingOnly) kept.pop();
  const body = kept
    .map((k) => k.chunk)
    .join("\n")
    .trim();
  if (body.length > 0) return `${body}${footer}`;
  return `${Array.from(lf).slice(0, budget - 1).join("").trimEnd()}…${footer}`;
}

/**
 * Build `latest.json`. `files` are the artifact base names; `readSig(name)` returns the content of `<name>.sig`
 * (or `null`). Throws with a readable message on anything that would ship a broken or unverifiable update.
 */
export function assembleManifest({ files, readSig, repo, tag, pubkey, notes, pubDate }) {
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repo ?? "")) throw new Error(`invalid repository: ${repo}`);
  const version = String(tag ?? "").replace(/^v/, "");
  if (!tag?.startsWith("v") || !parseSemver(version)) throw new Error(`invalid tag: ${tag}`);
  const keyId = keyIdOfPubkey(pubkey);
  const platforms = {};
  const pick = (key, pattern) => {
    const matches = files.filter((f) => pattern.test(f));
    if (matches.length === 0) throw new Error(`no updater artifact for ${key}`);
    if (matches.length > 1) throw new Error(`more than one updater artifact for ${key}: ${matches.join(", ")}`);
    const name = matches[0];
    if (!SAFE_ASSET.test(name)) throw new Error(`asset name GitHub would rename: ${name}`);
    const signature = readSig(name);
    if (!signature) throw new Error(`missing signature ${name}.sig for ${key}`);
    const sig = parseSignature(signature);
    if (sig.keyId !== keyId) throw new Error(`${name}.sig was made with key ${sig.keyId}, not the release key ${keyId}`);
    if (sig.fields.version !== version) {
      throw new Error(`${name}.sig was signed for version ${sig.fields.version ?? "(none)"}, not ${version}`);
    }
    if (sig.fields.file !== name) throw new Error(`${name}.sig was made for file ${sig.fields.file}`);
    platforms[key] = {
      signature: signature.trim(),
      url: `https://github.com/${repo}/releases/download/${encodeURIComponent(tag)}/${encodeURIComponent(name)}`,
    };
  };
  for (const [key, pattern] of Object.entries(PLATFORMS)) pick(key, pattern);
  const releasePage = `https://github.com/${repo}/releases/tag/${tag}`;
  return {
    version,
    notes: truncateNotes(notes ?? `Shellfin ${version}. Release notes: ${releasePage}`, MAX_NOTES_CHARS, releasePage),
    pub_date: pubDate,
    platforms,
  };
}

// ---- channels -------------------------------------------------------------------------------------------------

/** Throws unless `version` is a stable release: SemVer without a pre-release part, not marked as one on GitHub. */
export function assertStableRelease(version, githubPrerelease) {
  const v = parseSemver(version);
  if (!v) throw new Error(`not a SemVer version: ${version}`);
  if (v.pre.length > 0) throw new Error(`${version} is a pre-release version: only stable releases are published`);
  if (githubPrerelease) throw new Error(`${version} is marked as a pre-release on GitHub: only stable releases are published`);
}

/** Write a manifest file only when the new version is strictly newer than the one it holds (or it holds none). */
export function shouldWrite(newVersion, current) {
  const old = current && typeof current.version === "string" ? current.version : null;
  if (!old || !parseSemver(old)) return true;
  return compareSemver(newVersion, old) > 0;
}

/**
 * The current manifests by file name, given the asset names the `updater` release lists and `readDownloaded(name)`,
 * which returns the parsed downloaded copy or `null`. Only a file the release does not list counts as missing
 * (`null`). A listed file that was not downloaded or holds no manifest with a SemVer version throws: a failed or
 * partial download must never make an existing manifest look missing, which would let an older release overwrite it.
 */
export function currentManifests(listedNames, readDownloaded) {
  const listed = new Set(listedNames);
  const current = {};
  for (const name of MANIFEST_FILES) {
    if (!listed.has(name)) {
      current[name] = null;
      continue;
    }
    const m = readDownloaded(name);
    if (!m || typeof m.version !== "string" || !parseSemver(m.version)) {
      throw new Error(`${name} is listed on the updater release but its download is missing or not a manifest`);
    }
    current[name] = m;
  }
  return current;
}

/**
 * The manifest files (`{ name, manifest }`) a published stable release writes, given the current ones by file name
 * (missing or `null` when there is none). Throws for a pre-release.
 */
export function manifestUpdates(manifest, githubPrerelease, currentByFile) {
  assertStableRelease(manifest.version, githubPrerelease);
  return MANIFEST_FILES.filter((name) => shouldWrite(manifest.version, currentByFile[name] ?? null)).map((name) => ({
    name,
    manifest,
  }));
}

// ---- CLI ------------------------------------------------------------------------------------------------------

function args(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i += 2) {
    const key = argv[i];
    if (!key?.startsWith("--") || argv[i + 1] === undefined) throw new Error(`bad argument near ${key}`);
    out[key.slice(2)] = argv[i + 1];
  }
  return out;
}

function* walk(dir) {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) yield* walk(p);
    else yield p;
  }
}

function readJson(path) {
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch {
    return null;
  }
}

function main(argv) {
  const [command, ...rest] = argv;
  const a = args(rest);
  if (command === "assemble") {
    const paths = [...walk(a.dir)];
    const byName = new Map();
    for (const p of paths) {
      const name = basename(p);
      if (byName.has(name)) throw new Error(`duplicate artifact name: ${name}`);
      byName.set(name, p);
    }
    const files = [...byName.keys()].filter((n) => !n.endsWith(".sig"));
    const conf = JSON.parse(readFileSync(a.conf, "utf8"));
    const version = a.tag.replace(/^v/, "");
    const manifest = assembleManifest({
      files,
      readSig: (name) => (byName.has(`${name}.sig`) ? readFileSync(byName.get(`${name}.sig`), "utf8") : null),
      repo: a.repo,
      tag: a.tag,
      pubkey: conf?.plugins?.updater?.pubkey,
      notes: a.changelog ? notesFromChangelog(readFileSync(a.changelog, "utf8"), version) : null,
      pubDate: new Date().toISOString().replace(/\.\d{3}Z$/, "Z"),
    });
    writeFileSync(a.out, `${JSON.stringify(manifest, null, 2)}\n`);
    console.log(`${a.out}: ${Object.keys(manifest.platforms).join(", ")}`);
    return;
  }
  if (command === "channels") {
    const manifest = readJson(a.manifest);
    if (!manifest || !parseSemver(manifest.version) || !manifest.platforms) throw new Error(`invalid manifest ${a.manifest}`);
    if (a.tag !== undefined && manifest.version !== a.tag.replace(/^v/, "")) {
      throw new Error(`${a.manifest} is for version ${manifest.version}, not the release ${a.tag}`);
    }
    if (a.prerelease !== "true" && a.prerelease !== "false") throw new Error("--prerelease must be true or false");
    if (a.listed === undefined || a.current === undefined) throw new Error("--listed and --current are required");
    const listed = readFileSync(a.listed, "utf8")
      .split(/\r?\n/)
      .map((l) => l.trim())
      .filter(Boolean);
    const current = currentManifests(listed, (name) => {
      const p = join(a.current, name);
      return existsSync(p) ? readJson(p) : null;
    });
    mkdirSync(a.out, { recursive: true });
    for (const { name, manifest: m } of manifestUpdates(manifest, a.prerelease === "true", current)) {
      writeFileSync(join(a.out, name), `${JSON.stringify(m, null, 2)}\n`);
      console.log(name);
    }
    return;
  }
  if (command === "notes") {
    // The GitHub release notes: the CHANGELOG section of this version. Writes nothing when there is none; the release
    // workflow then falls back to generated notes.
    const notes = notesFromChangelog(readFileSync(a.changelog, "utf8"), a.tag.replace(/^v/, ""));
    if (notes === null) {
      console.log(`no CHANGELOG section for ${a.tag}`);
      return;
    }
    writeFileSync(a.out, `${notes}\n`);
    console.log(`${a.out}: ${notes.split("\n").length} lines`);
    return;
  }
  throw new Error("usage: updater-manifest.mjs assemble|channels|notes --...");
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main(process.argv.slice(2));
  } catch (e) {
    console.error(`updater-manifest: ${e instanceof Error ? e.message : String(e)}`);
    process.exit(1);
  }
}
