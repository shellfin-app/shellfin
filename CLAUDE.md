@AGENTS.md

# Claude Code notes

- `src-tauri/CLAUDE.md` and `src/CLAUDE.md` import their `AGENTS.md` (Claude Code loads a nested `CLAUDE.md` when you
  work in that directory; it does not load a nested `AGENTS.md` by itself). Read them before editing there. The deep
  reference is `docs/ARCHITECTURE.md` (pointers in AGENTS.md); read the relevant section instead of guessing.
- Project permissions (read-only commands and the quality gates) are in `.claude/settings.json`. Personal overrides
  go in `.claude/settings.local.json` (listed in `.gitignore`, never commit it).
- `/gates` runs every quality gate in order and reports a summary. Run it, or the equivalent commands from
  AGENTS.md, before you tell the user the work is done. Include `cargo test --locked --features it` only when you
  touched `ssh/`, `session/`, `terminal/` or `sftp/` and Docker is available.
- Use subagents for independent work, for example a read-only review of a diff or a search across `src/` and
  `src-tauri/`. Give them the files and the rules that matter (especially the "Hard rules" in AGENTS.md); they do not
  see this conversation. Do not let two agents edit the same file at once.
- Prefer the Edit tool for source changes (see rule 9 in AGENTS.md). Use a scratch script only for a mechanical
  change across many files, written to a file, with raw strings and an assertion that each pattern matches once.
- Do not commit, push or open pull requests unless the user explicitly asks.
