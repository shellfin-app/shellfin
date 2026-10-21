---
description: Run every Shellfin quality gate (Rust and frontend) and summarise the result
allowed-tools: Bash(cd src-tauri && cargo fmt *) Bash(cd src-tauri && cargo clippy *) Bash(cd src-tauri && cargo test *) Bash(npm run notices:check) Bash(npm run lint) Bash(npm run typecheck) Bash(npm run test) Bash(npm run i18n:check) Bash(npm run build) Bash(npm run test:freeze) Bash(git status *) Bash(git diff *)
---

Run the quality gates from AGENTS.md, in this order, and keep going after a failure so the report is complete.
Run each command from the repository root with the `cd src-tauri && ...` form for cargo.

1. `cd src-tauri && cargo fmt --check`
2. `cd src-tauri && cargo clippy --locked --all-targets -- -D warnings`
3. `cd src-tauri && cargo clippy --locked --all-targets --features it -- -D warnings`
4. `cd src-tauri && cargo clippy --locked --all-targets --features e2e -- -D warnings`
5. `cd src-tauri && cargo test --locked` (also regenerates the ts-rs bindings)
6. `cd src-tauri && cargo test --locked --features e2e --lib e2e::`
7. `git status --short src/ipc/bindings`: any change here means the committed bindings are out of date; report it
   (do not hand-edit them; `npm run bindings` regenerates).
8. `npm run lint`
9. `npm run typecheck`
10. `npm run test`
11. `npm run i18n:check`
12. `npm run notices:check` (needs `cargo-about`; if it is not installed, say so instead of failing the gate)
13. `npm run build`
14. `npm run test:freeze`

Do not run the Docker integration suite (`--features it` tests) unless `$ARGUMENTS` contains `integration`; then also
run `cd src-tauri && cargo test --locked --features it` and remove the fixtures afterwards with
`docker compose -p shellfin-it -f src-tauri/tests/sshd/compose.yml down -v`.

Finish with one table: gate, pass or fail, and for each failure the first relevant error lines and the likely cause.
Do not fix anything unless the user asked; just report.
