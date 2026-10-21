// Third-party licence notices (docs/ARCHITECTURE.md §11.6). Generates THIRD_PARTY_NOTICES.txt, which ships in every
// installer (`bundle.resources` in src-tauri/tauri.conf.json) and is checked in CI. It also builds the installer's
// licence page src-tauri/installer/LICENSE.txt (both licence texts of the dual licence).
//
//   node scripts/notices.mjs           write THIRD_PARTY_NOTICES.txt
//   node scripts/notices.mjs --check   fail when the committed file differs from what the lockfiles produce
//
// Inputs, all reproducible from the committed lockfiles:
//   - Rust: `cargo about generate` (cargo-about 0.9.2, install it with `cargo install cargo-about --locked --features cli`)
//     with src-tauri/about.toml (the licence allow-list of deny.toml, the four shipped targets) and src-tauri/about.hbs.
//   - npm: every non-dev package of package-lock.json; its licence file is read from node_modules (run `npm ci` first).
//     The two fonts (Inter, JetBrains Mono) carry the SIL OFL 1.1 text and copyright line in their package.
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import process from "node:process";
import { fileURLToPath, pathToFileURL } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
export const OUTPUT = join(ROOT, "THIRD_PARTY_NOTICES.txt");
const RULE = "-".repeat(80);
const RULE_HEAVY = "=".repeat(80);

/** LF line endings, no trailing blanks, no trailing empty lines: the same bytes on every platform. */
export function normalize(text) {
  return `${String(text)
    .replace(/\r\n?/g, "\n")
    .split("\n")
    .map((l) => l.replace(/[ \t]+$/, ""))
    .join("\n")
    .trim()}\n`;
}

const byText = (a, b) => (a < b ? -1 : a > b ? 1 : 0);

/** Production (non-dev) packages of an npm lockfile v3: `{ name, version, license, dir }`, sorted by name. */
export function productionPackages(lock) {
  const out = [];
  for (const [path, pkg] of Object.entries(lock.packages ?? {})) {
    if (path === "" || pkg.dev === true) continue;
    const name = pkg.name ?? path.slice(path.lastIndexOf("node_modules/") + "node_modules/".length);
    out.push({ name, version: pkg.version, license: pkg.license ?? "UNKNOWN", dir: path });
  }
  return out.sort((a, b) => byText(a.name, b.name) || byText(a.version, b.version));
}

/** The text of the licence file(s) of an installed package, or `null` when it ships none. */
function licenseTextOf(dir) {
  const files = readdirSync(dir)
    .filter((f) => /^(licen[sc]e|copying|notice)(\.|$|-)/i.test(f))
    .sort(byText);
  if (files.length === 0) return null;
  return files.map((f) => normalize(readFileSync(join(dir, f), "utf8"))).join("\n");
}

/** Group packages that have the same licence text, as the Rust section does. */
export function groupByText(entries) {
  const groups = new Map();
  for (const e of entries) {
    const key = `${e.license}\u0000${e.text}`;
    if (!groups.has(key)) groups.set(key, { license: e.license, text: e.text, used: [] });
    groups.get(key).used.push(`${e.name} ${e.version}`);
  }
  return [...groups.values()].sort((a, b) => byText(a.license, b.license) || byText(a.used[0], b.used[0]));
}

function npmSection() {
  const lock = JSON.parse(readFileSync(join(ROOT, "package-lock.json"), "utf8"));
  const entries = productionPackages(lock).map((p) => {
    const dir = join(ROOT, p.dir);
    if (!existsSync(dir)) throw new Error(`${p.dir} is not installed: run \`npm ci\` before generating the notices`);
    let text = licenseTextOf(dir);
    if (text === null) {
      const meta = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
      const author = typeof meta.author === "string" ? meta.author : (meta.author?.name ?? "the package authors");
      text = `This package ships no licence file. Its package.json declares the licence ${p.license}; copyright ${author}.\n`;
    }
    return { ...p, text };
  });
  return groupByText(entries)
    .map((g) => `${RULE}\n${g.license}\n\nUsed by:\n${g.used.map((u) => `  - ${u}`).join("\n")}\n\n${g.text}`)
    .join("\n");
}

function rustSection() {
  // `-o` rather than stdout: cargo-about refuses to write to a redirected stdout on Windows PowerShell.
  const dir = mkdtempSync(join(tmpdir(), "shellfin-notices-"));
  try {
    const file = join(dir, "rust.txt");
    execFileSync("cargo", ["about", "generate", "--locked", "--fail", "-o", file, "about.hbs"], {
      cwd: join(ROOT, "src-tauri"),
      stdio: ["ignore", "inherit", "inherit"],
    });
    return normalize(readFileSync(file, "utf8"));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

const HEADER = `Shellfin third-party notices
${RULE_HEAVY}

Shellfin is licensed under MIT OR Apache-2.0 (LICENSE-MIT, LICENSE-APACHE). The installers redistribute the open-source
components listed below. Each is licensed by its authors under the terms printed with it. This file is generated from
Cargo.lock and package-lock.json (\`npm run notices\`).

Source code of MPL-2.0 components (file-level copyleft, used unmodified): the repository of each crate is listed under
"Used by" and on https://crates.io/crates/<name>; the exact version is the one named here.

Fonts: Inter and JetBrains Mono are embedded in the application and are licensed under the SIL Open Font License,
Version 1.1. Their copyright lines and the full licence text are in the npm section, under OFL-1.1.

${RULE_HEAVY}
PART 1: Rust crates
${RULE_HEAVY}

`;

export function generate() {
  return `${HEADER}${rustSection()}\n${RULE_HEAVY}\nPART 2: npm packages (production dependencies)\n${RULE_HEAVY}\n\n${npmSection()}`
    .replace(/\n{3,}/g, "\n\n");
}

/** The licence page of the Windows installer: Shellfin is dual-licensed, so it shows both texts (not just MIT). */
export const INSTALLER_LICENSE = join(ROOT, "src-tauri", "installer", "LICENSE.txt");

export function installerLicense() {
  const mit = normalize(readFileSync(join(ROOT, "LICENSE-MIT"), "utf8"));
  const apache = normalize(readFileSync(join(ROOT, "LICENSE-APACHE"), "utf8"));
  return normalize(
    `Shellfin is dual-licensed: you may use it under the MIT license OR the Apache License 2.0, at your option.\n` +
      `Both texts follow. Third-party components are listed in THIRD_PARTY_NOTICES.txt.\n\n` +
      `${RULE_HEAVY}\nMIT License\n${RULE_HEAVY}\n\n${mit}\n${RULE_HEAVY}\nApache License 2.0\n${RULE_HEAVY}\n\n${apache}`,
  );
}

function main(argv) {
  const outputs = [
    [OUTPUT, normalize(generate()), "THIRD_PARTY_NOTICES.txt"],
    [INSTALLER_LICENSE, installerLicense(), "src-tauri/installer/LICENSE.txt"],
  ];
  if (argv.includes("--check")) {
    for (const [path, text, label] of outputs) {
      const current = existsSync(path) ? normalize(readFileSync(path, "utf8")) : "";
      if (current !== text) throw new Error(`${label} is out of date: run \`npm run notices\` and commit the result`);
    }
    console.log("licence notices are up to date");
    return;
  }
  mkdirSync(dirname(INSTALLER_LICENSE), { recursive: true });
  for (const [path, text] of outputs) {
    writeFileSync(path, text);
    console.log(`wrote ${path}`);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main(process.argv.slice(2));
  } catch (e) {
    console.error(`notices: ${e instanceof Error ? e.message : String(e)}`);
    process.exit(1);
  }
}
