// `npm run bindings:check` (after `npm run bindings`): the generated ts-rs bindings in src/ipc/bindings must match the
// committed ones. Same three checks as the CI job `checks`:
//   1. no changed tracked file (git diff),
//   2. no new untracked file (git status): a new DTO that was never committed,
//   3. no orphan: a binding whose Rust type no longer exists (ts-rs never deletes files).
import { execFileSync } from "node:child_process";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import process from "node:process";
import { fileURLToPath, pathToFileURL } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const BINDINGS = "src/ipc/bindings";

function* walk(dir) {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) yield* walk(p);
    else yield p;
  }
}

/** Binding files (`Name.ts`) without a `struct`, `enum` or `type` of that name in the given Rust sources. */
export function orphanBindings(bindingFiles, rustSources) {
  const source = rustSources.join("\n");
  return bindingFiles.filter((file) => {
    const name = file.replace(/\.ts$/, "");
    return !new RegExp(`\\b(?:struct|enum|type)\\s+${name}\\b`).test(source);
  });
}

function main() {
  const git = (...a) => execFileSync("git", a, { cwd: ROOT, encoding: "utf8" });
  const problems = [];
  try {
    execFileSync("git", ["diff", "--exit-code", "--stat", "--", BINDINGS], { cwd: ROOT, stdio: "inherit" });
  } catch {
    problems.push("generated bindings differ from the committed ones");
  }
  const untracked = git("status", "--porcelain", "--untracked-files=all", "--", BINDINGS)
    .split("\n")
    .filter((l) => l.startsWith("??"));
  if (untracked.length > 0) problems.push(`generated bindings that are not committed:\n${untracked.join("\n")}`);

  const files = readdirSync(join(ROOT, BINDINGS)).filter((f) => f.endsWith(".ts"));
  const rust = [...walk(join(ROOT, "src-tauri", "src"))].filter((p) => p.endsWith(".rs")).map((p) => readFileSync(p, "utf8"));
  const orphans = orphanBindings(files, rust);
  if (orphans.length > 0) problems.push(`bindings without a Rust type (delete them): ${orphans.join(", ")}`);

  if (problems.length > 0) {
    console.error(`bindings:check failed\n${problems.join("\n")}`);
    process.exit(1);
  }
  console.log("bindings are up to date");
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main();
