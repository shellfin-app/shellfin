// The npm packages of Tauri and their Rust crates must be on the same major.minor release (docs/ARCHITECTURE.md
// §11.3): `@tauri-apps/api` with the `tauri` crate and every `@tauri-apps/plugin-<name>` with `tauri-plugin-<name>`.
// The Tauri CLI checks this itself during `tauri build` through `npm list`, which finds nothing in the build jobs
// (they install no npm package), so this script makes the check explicit. Plain Node, lockfiles only.
//
//   node scripts/tauri-versions.mjs --lock package-lock.json --cargo-lock src-tauri/Cargo.lock
import { readFileSync } from "node:fs";
import process from "node:process";
import { pathToFileURL } from "node:url";

/** `name -> [versions]` of the packages in a Cargo.lock. */
export function cargoLockVersions(text) {
  const out = new Map();
  for (const block of text.replace(/\r\n/g, "\n").split(/^\[\[package\]\]\s*$/m).slice(1)) {
    const name = /^name = "([^"]+)"$/m.exec(block)?.[1];
    const version = /^version = "([^"]+)"$/m.exec(block)?.[1];
    if (!name || !version) continue;
    out.set(name, [...(out.get(name) ?? []), version]);
  }
  return out;
}

const majorMinor = (v) => /^(\d+)\.(\d+)\./.exec(v)?.slice(1, 3).join(".") ?? null;

/** Mismatches between the top-level npm Tauri packages and their crates, as messages (empty: consistent). */
export function tauriMismatches(npmLock, cargoLockText) {
  const crates = cargoLockVersions(cargoLockText);
  const pairs = [];
  for (const [key, entry] of Object.entries(npmLock?.packages ?? {})) {
    const m = /^node_modules\/@tauri-apps\/(api|plugin-([a-z0-9-]+))$/.exec(key);
    if (!m || entry?.dev) continue;
    pairs.push({ npm: `@tauri-apps/${m[1]}`, version: entry.version, crate: m[1] === "api" ? "tauri" : `tauri-plugin-${m[2]}` });
  }
  if (!pairs.some((p) => p.npm === "@tauri-apps/api")) return ["package-lock.json has no @tauri-apps/api"];
  const problems = [];
  for (const { npm, version, crate } of pairs) {
    const versions = crates.get(crate) ?? [];
    if (versions.length !== 1) {
      problems.push(`${npm} ${version}: expected exactly one ${crate} in Cargo.lock, found ${versions.length}`);
      continue;
    }
    const a = majorMinor(version);
    const b = majorMinor(versions[0]);
    if (a === null || a !== b) problems.push(`${npm} ${version} and ${crate} ${versions[0]} differ in major.minor`);
  }
  return problems;
}

function main(argv) {
  if (argv.length !== 4 || argv[0] !== "--lock" || argv[2] !== "--cargo-lock") {
    throw new Error("usage: tauri-versions.mjs --lock <package-lock.json> --cargo-lock <Cargo.lock>");
  }
  const problems = tauriMismatches(JSON.parse(readFileSync(argv[1], "utf8")), readFileSync(argv[3], "utf8"));
  if (problems.length > 0) throw new Error(`Tauri npm packages and crates disagree:\n  ${problems.join("\n  ")}`);
  console.log("tauri-versions: npm packages and crates are on the same major.minor");
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main(process.argv.slice(2));
  } catch (err) {
    console.error(`tauri-versions: ${err instanceof Error ? err.message : String(err)}`);
    process.exit(1);
  }
}
