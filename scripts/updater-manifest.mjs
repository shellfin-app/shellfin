// Updater manifests (docs/ARCHITECTURE.md §9.8, §11.3). Plain Node, no dependencies, so the release workflow can run
// it without `npm ci`. Two commands:
//
//   assemble  Build the release's `latest.json` (Tauri static updater format) from the signed updater artifacts of
//             the build jobs. Checks that every platform has exactly one artifact and that each `.sig` was made with
//             the release key (key id of `plugins.updater.pubkey`) for this version and file.
//     node scripts/updater-manifest.mjs assemble --dir <artifacts> --repo <owner/name> --tag <vX.Y.Z>
//          --conf src-tauri/tauri.conf.json --changelog CHANGELOG.md --out latest.json
//
//   channels  Decide which channel files a PUBLISHED release updates and write them: a stable release (no SemVer
//             pre-release part and not marked as a pre-release on GitHub) -> stable.json and experimental.json; any
//             pre-release -> experimental.json only. A channel file is only written when the new version is
//             SemVer-greater than the one it holds, so republishing an old release can never downgrade anyone.
//     node scripts/updater-manifest.mjs channels --manifest latest.json --tag <vX.Y.Z> --prerelease <true|false>
//          --current <dir with the current stable.json / experimental.json, if any> --out <dir>
//     Prints the written file names, one per line.
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

export const CHANNELS = ["stable", "experimental"];
/** Longest release notes put into a manifest (the app shows at most 10 000 characters). */
export const MAX_NOTES_CHARS = 8000;
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

function truncate(text, max) {
  const chars = Array.from(text);
  return chars.length <= max ? text : `${chars.slice(0, max - 1).join("")}…`;
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
    notes: truncate(notes ?? `Shellfin ${version}. Release notes: ${releasePage}`, MAX_NOTES_CHARS),
    pub_date: pubDate,
    platforms,
  };
}

// ---- channels -------------------------------------------------------------------------------------------------

/** A stable release reaches both channels; anything marked or versioned as a pre-release only `experimental`. */
export function channelsFor(version, githubPrerelease) {
  const v = parseSemver(version);
  if (!v) throw new Error(`not a SemVer version: ${version}`);
  return githubPrerelease || v.pre.length > 0 ? ["experimental"] : ["stable", "experimental"];
}

/** Write a channel file only when the new version is strictly newer than the one it holds (or it holds none). */
export function shouldWrite(newVersion, current) {
  const old = current && typeof current.version === "string" ? current.version : null;
  if (!old || !parseSemver(old)) return true;
  return compareSemver(newVersion, old) > 0;
}

/** The channel files (`{ name, manifest }`) a published release writes, given the current ones (or `null`). */
export function channelUpdates(manifest, githubPrerelease, currentByChannel) {
  return channelsFor(manifest.version, githubPrerelease)
    .filter((ch) => shouldWrite(manifest.version, currentByChannel[ch] ?? null))
    .map((ch) => ({ name: `${ch}.json`, manifest }));
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
    const current = {};
    for (const ch of CHANNELS) {
      const p = a.current ? join(a.current, `${ch}.json`) : null;
      current[ch] = p && existsSync(p) ? readJson(p) : null;
    }
    mkdirSync(a.out, { recursive: true });
    for (const { name, manifest: m } of channelUpdates(manifest, a.prerelease === "true", current)) {
      writeFileSync(join(a.out, name), `${JSON.stringify(m, null, 2)}\n`);
      console.log(name);
    }
    return;
  }
  if (command === "notes") {
    // The GitHub release notes: the CHANGELOG section of this version. Writes nothing when there is none (pre-release
    // tags usually have no section); the release workflow then falls back to generated notes.
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
