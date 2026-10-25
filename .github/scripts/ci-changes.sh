#!/usr/bin/env bash
# Change detection for the `changes` job of ci.yml (docs/ARCHITECTURE.md §11.2). Prints `name=true|false` lines for
# $GITHUB_OUTPUT; a job runs when its output is true:
#   code       anything except documentation changed                     -> checks
#   rust       the Rust core, its build inputs (npm manifests too) or CI -> msrv, unit-native, integration
#   packaging  rust, or what else the bundles are built from             -> build
#   cask       packaging, or the Homebrew cask renderer                  -> homebrew-cask
# Fails open: every event except pull_request, an empty diff and any failure to compute the diff print all true.
#
#   ci-changes.sh              EVENT_NAME, BASE_SHA, HEAD_SHA from the environment; diffs the merge base of the pull
#                              request (BASE...HEAD, needs the history of both, e.g. checkout fetch-depth 0)
#   ci-changes.sh --classify   classify NUL-separated paths from stdin (used by the above; for tests)
set -euo pipefail

emit() {
  printf 'code=%s\nrust=%s\npackaging=%s\ncask=%s\n' "$1" "$2" "$3" "$4"
}

all_true() {
  echo "$1: running every job" >&2
  emit true true true true
}

classify() {
  local f n=0 code=false rust=false packaging=false cask=false
  # `|| [ -n "$f" ]`: a last path without a terminating NUL still counts.
  while IFS= read -r -d '' f || [ -n "$f" ]; do
    n=$((n + 1))
    # Documentation: nothing that CI builds, tests or bundles reads it. Test fixtures (any extension) are read by the
    # Rust tests (fixture_keys_are_all_test_keys), so they never count as documentation.
    case "$f" in
      src-tauri/tests/*) ;;
      *.md | *.mdc | docs/* | assets/* | .github/ISSUE_TEMPLATE/* | .github/CODEOWNERS | .claude/* | .cursor/* | .vscode/*)
        continue ;;
    esac
    code=true
    case "$f" in
      # The Rust crate with everything compiled into it (tauri.conf.json, capabilities, icons, installer, tests),
      # toolchain and cargo policy files, line-ending rules, and CI itself (a workflow change runs every job). The npm
      # manifests too: tauri.conf.json takes the app version from package.json when the crate compiles, and the lock
      # file goes with it.
      src-tauri/* | Cargo.toml | Cargo.lock | */Cargo.toml | */Cargo.lock | rust-toolchain* | */rust-toolchain* | \
        deny.toml | .cargo/* | .gitattributes | .github/* | package.json | package-lock.json)
        rust=true packaging=true cask=true ;;
      # What the bundles are built from besides the crate: the bundled notices, the frontend build configuration
      # (Vite config and plugins, the HTML entry, TypeScript config, npm and Node settings) and the no-mock check of
      # `npm run build` (tauri.conf.json `beforeBuildCommand`), the manifest check that hands dist/ to `build`
      # (scripts/dist-manifest.mjs), the prebuilt Tauri CLI fetcher and the npm/crate version check
      # (scripts/tauri-cli.mjs, scripts/tauri-versions.mjs). scripts/freeze-check.mjs is not here: only `checks` runs it
      # (`npm run test:freeze`).
      THIRD_PARTY_NOTICES.txt | vite.config.* | vite-plugins/* | index.html | tsconfig*.json | \
        scripts/assert-no-mock.mjs | scripts/dist-manifest.mjs | scripts/tauri-cli.mjs | scripts/tauri-versions.mjs | \
        .npmrc | .nvmrc)
        packaging=true cask=true ;;
      scripts/homebrew-cask*)
        cask=true ;;
    esac
  done
  if [ "$n" -eq 0 ]; then
    all_true "empty diff"
    return
  fi
  echo "$n changed files" >&2
  emit "$code" "$rust" "$packaging" "$cask"
}

if [ "${1:-}" = "--classify" ]; then
  classify
  exit 0
fi

if [ "${EVENT_NAME:-}" != "pull_request" ]; then
  all_true "event ${EVENT_NAME:-unknown} (not a pull request)"
  exit 0
fi

is_sha() {
  case "$1" in
    "" | *[!0-9a-f]*) return 1 ;;
  esac
  [ "${#1}" -eq 40 ] || [ "${#1}" -eq 64 ]
}

if ! is_sha "${BASE_SHA:-}" || ! is_sha "${HEAD_SHA:-}"; then
  echo "::warning::change detection: no valid base/head commit" >&2
  all_true "no diff"
  exit 0
fi

files="$(mktemp)"
trap 'rm -f "$files"' EXIT
# --no-renames: a moved file counts at its old and its new path.
if ! git diff --name-only --no-renames -z "$BASE_SHA...$HEAD_SHA" -- > "$files"; then
  echo "::warning::change detection: cannot diff $BASE_SHA...$HEAD_SHA" >&2
  all_true "no diff"
  exit 0
fi
classify < "$files"
