// Fails if the given build output (default: dist) contains the MockBackend marker.
// Run after every production build (`npm run build`), and in CI on release artifacts' frontend bundle.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import process from "node:process";

const MARKER = "__shellfin_mock_backend__";
const dir = process.argv[2] ?? "dist";

function* walk(d) {
  for (const name of readdirSync(d)) {
    const p = join(d, name);
    if (statSync(p).isDirectory()) yield* walk(p);
    else if (/\.(js|mjs|html|css|map)$/.test(name)) yield p;
  }
}

const offenders = [];
for (const file of walk(dir)) {
  if (readFileSync(file, "utf8").includes(MARKER)) offenders.push(file);
}
if (offenders.length > 0) {
  console.error(`MockBackend marker found in ${dir}:\n  ${offenders.join("\n  ")}`);
  process.exit(1);
}
console.log(`assert-no-mock: ${dir} is free of MockBackend`);
