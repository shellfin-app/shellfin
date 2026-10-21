## Summary

<!-- What does this change and why? Keep it short. -->

## Linked issue

<!-- Closes #123, or "none" for trivial fixes. Larger changes should be discussed in an issue first. -->

## Type of change

- [ ] Bug fix
- [ ] New feature
- [ ] Refactor / cleanup
- [ ] Documentation
- [ ] Build / CI
- [ ] Translation

## Checklist

Quality gates (see CONTRIBUTING.md):

- [ ] `cd src-tauri && cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked`
- [ ] `cd src-tauri && cargo clippy --locked --all-targets --features it -- -D warnings`
- [ ] `cd src-tauri && cargo clippy --locked --all-targets --features e2e -- -D warnings`
- [ ] `cd src-tauri && cargo test --locked --features e2e --lib e2e::`
- [ ] `npm run lint`
- [ ] `npm run typecheck`
- [ ] `npm run test`
- [ ] `npm run i18n:check`
- [ ] `npm run build`
- [ ] `npm run test:freeze`
- [ ] If dependencies changed: `npm run notices` and the regenerated `THIRD_PARTY_NOTICES.txt` is committed
- [ ] If `ssh/`, `session/`, `terminal/` or `sftp/` changed: `cd src-tauri && cargo test --locked --features it` (needs Docker)

Other:

- [ ] New or changed behavior has tests (security-sensitive code includes tests with hostile input)
- [ ] No hard-coded user-facing text; strings go through i18n
- [ ] If Rust DTOs changed: bindings regenerated with `npm run bindings` and committed (never hand-edited)
- [ ] No secrets, private keys, real hostnames, usernames or other personal data in code, tests, logs or screenshots
- [ ] Documentation updated (README, `docs/ARCHITECTURE.md`, `CHANGELOG.md` under `[Unreleased]`) where relevant
- [ ] Commit messages follow Conventional Commits
- [ ] I agree that my contribution is licensed under MIT OR Apache-2.0 (see CONTRIBUTING.md)

## Notes for reviewers

<!-- Risks, trade-offs, how you tested manually, platforms tried. -->
