// The Tauri CLI for the release and CI build jobs (docs/ARCHITECTURE.md §11.3), without `npm ci` and without
// compiling it. Plain Node, no dependencies.
//
//   fetch  Download the prebuilt CLI of this platform, the npm package `@tauri-apps/cli-<triple>` at the version
//          package-lock.json records, verify the tarball against the lockfile's `integrity` (sha512), and extract only
//          the native addon `cli.<triple>.node`. No npm, no package script, no JavaScript of the package runs.
//     node scripts/tauri-cli.mjs fetch --lock package-lock.json --package package.json --out <file.node>
//
//   run    Run the CLI from that addon (what @tauri-apps/cli's own main.js does, minus its JavaScript).
//     node scripts/tauri-cli.mjs run --cli <file.node> -- <tauri arguments...>
//
// Supported platforms: exactly the four release targets (win32-x64-msvc, win32-arm64-msvc, linux-x64-gnu,
// darwin-arm64); anything else fails.
import { Buffer } from "node:buffer";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import process from "node:process";
import { pathToFileURL } from "node:url";
import { gunzipSync } from "node:zlib";

const REGISTRY = "https://registry.npmjs.org/";
const EXACT_VERSION = /^\d+\.\d+\.\d+$/;

/** Node platform/arch -> npm platform package suffix of @tauri-apps/cli (the four release targets only). */
export const TRIPLES = {
  "win32-x64": "win32-x64-msvc",
  "win32-arm64": "win32-arm64-msvc",
  "linux-x64": "linux-x64-gnu",
  "darwin-arm64": "darwin-arm64",
};

export function tripleFor(platform, arch) {
  const triple = TRIPLES[`${platform}-${arch}`];
  if (!triple) throw new Error(`no prebuilt Tauri CLI for ${platform}-${arch} (release targets only)`);
  return triple;
}

/**
 * What to download for `triple`: the version of @tauri-apps/cli must be exact and the same in package.json and
 * package-lock.json, the platform package must have that version, and its URL must be the registry's canonical one.
 */
export function cliPackage(lock, pkg, triple) {
  const want = pkg?.devDependencies?.["@tauri-apps/cli"];
  if (typeof want !== "string" || !EXACT_VERSION.test(want)) {
    throw new Error(`@tauri-apps/cli in package.json is not an exact version: ${JSON.stringify(want)}`);
  }
  const main = lock?.packages?.["node_modules/@tauri-apps/cli"];
  if (main?.version !== want) throw new Error(`package-lock.json has @tauri-apps/cli ${main?.version}, not ${want}`);
  const name = `@tauri-apps/cli-${triple}`;
  const entry = lock.packages[`node_modules/${name}`];
  if (!entry) throw new Error(`package-lock.json has no ${name}`);
  if (entry.version !== want) throw new Error(`package-lock.json has ${name} ${entry.version}, not ${want}`);
  const url = `${REGISTRY}${name}/-/cli-${triple}-${want}.tgz`;
  if (entry.resolved !== url) throw new Error(`${name} resolves to ${JSON.stringify(entry.resolved)}, not ${url}`);
  const m = /^sha512-([A-Za-z0-9+/]{86}==)$/.exec(entry.integrity ?? "");
  if (!m) throw new Error(`${name} has no sha512 integrity in package-lock.json`);
  return { name, version: want, url, sha512: m[1], member: `package/cli.${triple}.node` };
}

/** Throws unless `bytes` has the sha512 (base64) of the lockfile. */
export function checkIntegrity(bytes, sha512Base64) {
  const got = createHash("sha512").update(bytes).digest("base64");
  if (got !== sha512Base64) throw new Error("tarball sha512 differs from the package-lock.json integrity");
}

function field(header, start, length) {
  const raw = header.subarray(start, start + length);
  const end = raw.indexOf(0);
  return raw.subarray(0, end === -1 ? length : end).toString("utf8");
}

/**
 * The content of the one regular file `member` in an (uncompressed) ustar archive. A missing or duplicate member, a
 * truncated archive or a bad header checksum throws. PAX/GNU long-name entries are skipped, not interpreted, so a
 * long name cannot alias the member.
 */
export function extractMember(tar, member) {
  let found = null;
  let offset = 0;
  while (offset + 512 <= tar.length) {
    const header = tar.subarray(offset, offset + 512);
    if (header.every((b) => b === 0)) break;
    let sum = 0;
    for (let i = 0; i < 512; i++) sum += i >= 148 && i < 156 ? 32 : header[i];
    const stored = parseInt(field(header, 148, 8).trim(), 8);
    if (stored !== sum) throw new Error(`bad tar header checksum at offset ${offset}`);
    const size = parseInt(field(header, 124, 12).trim() || "0", 8);
    if (!Number.isSafeInteger(size) || size < 0) throw new Error(`bad tar size at offset ${offset}`);
    const type = String.fromCharCode(header[156] || 48);
    const prefix = field(header, 345, 155);
    const name = prefix ? `${prefix}/${field(header, 0, 100)}` : field(header, 0, 100);
    const start = offset + 512;
    if (start + size > tar.length) throw new Error("truncated tar archive");
    if (name === member) {
      if (type !== "0") throw new Error(`${member} is not a regular file`);
      if (found) throw new Error(`${member} appears twice`);
      found = Buffer.from(tar.subarray(start, start + size));
    }
    offset = start + Math.ceil(size / 512) * 512;
  }
  if (!found) throw new Error(`${member} not in the tarball`);
  return found;
}

async function download(url) {
  const { fetch, AbortSignal } = globalThis; // Node >= 18 built-ins
  let last;
  for (let attempt = 1; attempt <= 3; attempt++) {
    try {
      // Redirects are followed: the lockfile integrity, not the transport, decides what is accepted.
      const res = await fetch(url, { signal: AbortSignal.timeout(120_000) });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      return Buffer.from(await res.arrayBuffer());
    } catch (err) {
      last = err;
      console.error(`download attempt ${attempt} failed: ${err instanceof Error ? err.message : String(err)}`);
    }
  }
  throw new Error(`cannot download ${url}: ${last instanceof Error ? last.message : String(last)}`);
}

async function fetchCli({ lock, package: pkgFile, out }) {
  const triple = tripleFor(process.platform, process.arch);
  const spec = cliPackage(JSON.parse(readFileSync(lock, "utf8")), JSON.parse(readFileSync(pkgFile, "utf8")), triple);
  const tgz = await download(spec.url);
  checkIntegrity(tgz, spec.sha512);
  const addon = extractMember(gunzipSync(tgz), spec.member);
  writeFileSync(out, addon);
  const sha256 = createHash("sha256").update(addon).digest("hex");
  console.log(`${spec.name}@${spec.version}: integrity ok, ${spec.member} -> ${out} (sha256 ${sha256})`);
}

function runCli(cli, args) {
  const mod = { exports: {} };
  process.dlopen(mod, cli);
  const { run, logError } = mod.exports;
  if (typeof run !== "function") throw new Error(`${cli} is not the Tauri CLI addon`);
  run(args, "tauri", (error) => {
    if (error) {
      if (typeof logError === "function") logError(error.message);
      else console.error(error.message);
      process.exit(1);
    }
  });
}

function parseOptions(argv, allowed) {
  const opts = {};
  for (let i = 0; i < argv.length; i += 2) {
    const key = argv[i]?.startsWith("--") ? argv[i].slice(2) : undefined;
    const value = argv[i + 1];
    if (!key || !allowed.includes(key) || value === undefined || key in opts) {
      throw new Error(`bad arguments near ${JSON.stringify(argv[i])}`);
    }
    opts[key] = value;
  }
  for (const key of allowed) if (!(key in opts)) throw new Error(`--${key} is required`);
  return opts;
}

async function main(argv) {
  const [command, ...rest] = argv;
  if (command === "fetch") return fetchCli(parseOptions(rest, ["lock", "package", "out"]));
  if (command === "run") {
    const sep = rest.indexOf("--");
    if (sep === -1) throw new Error("run: missing -- before the tauri arguments");
    return runCli(parseOptions(rest.slice(0, sep), ["cli"]).cli, rest.slice(sep + 1));
  }
  throw new Error(
    "usage: tauri-cli.mjs fetch --lock <package-lock.json> --package <package.json> --out <file> | run --cli <file> -- <args>",
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2)).catch((err) => {
    console.error(`tauri-cli: ${err instanceof Error ? err.message : String(err)}`);
    process.exit(1);
  });
}
