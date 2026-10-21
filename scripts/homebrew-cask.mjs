// Homebrew tap casks (docs/ARCHITECTURE.md §11.3). Plain Node, no dependencies. The tap is the separate repository
// shellfin-app/homebrew-tap; `.github/workflows/homebrew-tap.yml` renders a cask when a release is PUBLISHED and
// pushes it there. Commands:
//
//   render  Print/write the cask of a release: `shellfin` (stable) or `shellfin@experimental` (pre-release).
//     node scripts/homebrew-cask.mjs render --repo <owner/name> --tag <vX.Y.Z> --prerelease <true|false>
//          --sums SHA256SUMS --conf src-tauri/tauri.conf.json --out <file> [--notarized <true|false>]
//     The sha256 is taken from the release's SHA256SUMS (the workflow verifies that file's attestation first); the
//     .dmg itself is never downloaded. Prints the cask token and file name on one line: `<token> <file name>`.
//
//   newer   Exit 0 and print `yes` when the tag is SemVer-greater than the cask file (or the file is missing), else
//           print `no`: the tap is never downgraded.
//     node scripts/homebrew-cask.mjs newer --cask <existing .rb or "-"> --tag <vX.Y.Z>
//
//   readme  Print the README of a fresh tap repository.
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import process from "node:process";
import { pathToFileURL } from "node:url";
import { compareSemver, channelsFor, parseSemver } from "./updater-manifest.mjs";

export const APP_ID = "app.shellfin.desktop";
export const DESCRIPTION = "Cross-platform SSH client with terminals, SFTP file tree and editor";
export const HOMEPAGE = "https://shellfin.app";

/** Homebrew's macOS symbols by major version. */
export const MACOS_NAMES = {
  11: "big_sur",
  12: "monterey",
  13: "ventura",
  14: "sonoma",
  15: "sequoia",
  26: "tahoe",
};

/** `bundle.macOS.minimumSystemVersion` ("11.0") -> `>= :big_sur`. Throws for a version Homebrew has no symbol for. */
export function macosRequirement(minimumSystemVersion) {
  const major = Number(String(minimumSystemVersion ?? "").split(".")[0]);
  const name = MACOS_NAMES[major];
  if (!name) throw new Error(`no Homebrew macOS symbol for minimumSystemVersion ${minimumSystemVersion}`);
  return `>= :${name}`;
}

/** The hash of `fileName` in a `sha256sum` file; throws unless exactly one well-formed line names it. */
export function sha256FromSums(sums, fileName) {
  const hits = String(sums)
    .split(/\r?\n/)
    .map((l) => /^([0-9a-f]{64}) [ *](.+)$/.exec(l))
    .filter((m) => m && m[2] === fileName);
  if (hits.length !== 1) throw new Error(`SHA256SUMS must list ${fileName} exactly once (found ${hits.length})`);
  return hits[0][1];
}

const ZAP = [
  `~/Library/Application Support/${APP_ID}`,
  `~/Library/Caches/${APP_ID}`,
  `~/Library/HTTPStorages/${APP_ID}`,
  `~/Library/Logs/${APP_ID}`,
  `~/Library/Preferences/${APP_ID}.plist`,
  `~/Library/Saved Application State/${APP_ID}.savedState`,
  `~/Library/WebKit/${APP_ID}`,
];

/**
 * Render a cask. `experimental` selects the token `shellfin@experimental`. The stanza order follows `brew style`.
 * `url` must be the release's aarch64 DMG; it is written with `#{version}` interpolation.
 */
export function renderCask({ repo, version, sha256, experimental, macos, notarized }) {
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repo ?? "")) throw new Error(`invalid repository: ${repo}`);
  if (!parseSemver(version)) throw new Error(`not a SemVer version: ${version}`);
  if (!/^[0-9a-f]{64}$/.test(sha256 ?? "")) throw new Error("sha256 must be 64 lowercase hex digits");
  const other = experimental ? "shellfin" : "shellfin@experimental";
  const token = experimental ? "shellfin@experimental" : "shellfin";
  const lines = [
    `cask "${token}" do`,
    `  version "${version}"`,
    `  sha256 "${sha256}"`,
    ``,
    `  url "https://github.com/${repo}/releases/download/v#{version}/Shellfin_#{version}_aarch64.dmg"`,
    `  name "Shellfin"`,
    `  desc "${DESCRIPTION}"`,
    `  homepage "${HOMEPAGE}"`,
    ``,
    `  livecheck do`,
    `    skip "Updated by the release workflow of the project"`,
    `  end`,
    ``,
    `  auto_updates true`,
    `  conflicts_with cask: "${other}"`,
    `  depends_on arch: :arm64`,
    `  depends_on macos: "${macos}"`,
    ``,
    `  app "Shellfin.app"`,
    ``,
    `  zap trash: [`,
    ...ZAP.map((p) => `    "${p}",`),
    `  ]`,
  ];
  if (!notarized) {
    lines.push(
      ``,
      `  caveats <<~EOS`,
      `    Shellfin is not notarized by Apple yet, so macOS blocks the first launch.`,
      `    Open it once, then go to System Settings > Privacy & Security, scroll to the message about Shellfin and`,
      `    choose "Open Anyway".`,
      `  EOS`,
    );
  }
  lines.push(`end`, ``);
  return lines.join("\n");
}

/** Cask token and file name for a release: a pre-release (SemVer suffix or GitHub flag) is the experimental cask. */
export function caskFor(version, githubPrerelease) {
  const experimental = !channelsFor(version, githubPrerelease).includes("stable");
  const token = experimental ? "shellfin@experimental" : "shellfin";
  return { experimental, token, fileName: `${token}.rb` };
}

/** The `version "..."` of a cask file's text, or null. */
export function caskVersion(text) {
  const m = /^\s*version "([^"]+)"\s*$/m.exec(String(text ?? ""));
  return m ? m[1] : null;
}

/** True when `version` is strictly newer than the cask text (or the text has no valid version). */
export function isNewer(version, caskText) {
  const current = caskVersion(caskText);
  if (!current || !parseSemver(current)) return true;
  return compareSemver(version, current) > 0;
}

export function tapReadme(repo = "shellfin-app/shellfin") {
  return [
    "# Shellfin Homebrew tap",
    "",
    `Casks for [Shellfin](${HOMEPAGE}) (https://github.com/${repo}). Written by the release workflow of the main repository; do not edit by hand.`,
    "",
    "```sh",
    "brew install --cask shellfin-app/tap/shellfin               # stable",
    "brew install --cask shellfin-app/tap/shellfin@experimental  # pre-releases (conflicts with the stable cask)",
    "```",
    "",
    "Apple Silicon only. Shellfin updates itself; Homebrew does not fight it (`auto_updates true`).",
    "",
  ].join("\n");
}

function args(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i += 2) {
    const key = argv[i];
    if (!key?.startsWith("--") || argv[i + 1] === undefined) throw new Error(`bad argument near ${key}`);
    out[key.slice(2)] = argv[i + 1];
  }
  return out;
}

function main(argv) {
  const [command, ...rest] = argv;
  const a = args(rest);
  if (command === "render") {
    if (!a.tag?.startsWith("v")) throw new Error(`invalid tag: ${a.tag}`);
    if (a.prerelease !== "true" && a.prerelease !== "false") throw new Error("--prerelease must be true or false");
    const version = a.tag.slice(1);
    const { experimental, token, fileName } = caskFor(version, a.prerelease === "true");
    const dmg = `Shellfin_${version}_aarch64.dmg`;
    const conf = JSON.parse(readFileSync(a.conf, "utf8"));
    const cask = renderCask({
      repo: a.repo,
      version,
      sha256: sha256FromSums(readFileSync(a.sums, "utf8"), dmg),
      experimental,
      macos: macosRequirement(conf?.bundle?.macOS?.minimumSystemVersion),
      notarized: a.notarized === "true",
    });
    writeFileSync(a.out, cask);
    console.log(`${token} ${fileName}`);
    return;
  }
  if (command === "newer") {
    const text = a.cask !== "-" && existsSync(a.cask) ? readFileSync(a.cask, "utf8") : "";
    console.log(isNewer(a.tag.replace(/^v/, ""), text) ? "yes" : "no");
    return;
  }
  if (command === "readme") {
    process.stdout.write(tapReadme());
    return;
  }
  throw new Error("usage: homebrew-cask.mjs render|newer|readme --...");
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main(process.argv.slice(2));
  } catch (e) {
    console.error(`homebrew-cask: ${e instanceof Error ? e.message : String(e)}`);
    process.exit(1);
  }
}
