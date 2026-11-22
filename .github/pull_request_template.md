## Issue

Issue: #

<!-- Use `Refs #N` while the work is partial; use `Closes #N` only when every acceptance criterion in the issue is met. Progress notes belong on the issue. -->

## Problem

<!-- What user-visible or engineering problem does this solve? -->

## Change

<!-- What changed, and why this layer? -->

## Roadmap / ADR

<!-- Link the milestone or ADR. -->

## Verification

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `cargo run --package okkhor-daemon -- self-check`
- [ ] hot-path benchmark before/after (if applicable)

## Risk

<!-- Privacy, security, compatibility, performance, migration. -->

## Checklist

- [ ] Tests added or updated
- [ ] Docs updated in the same change
- [ ] Linked issue updated with progress and test results
- [ ] Linked issue closed only when acceptance criteria are met
- [ ] No secrets, user text, or proprietary assets
- [ ] No new `unsafe` outside `okkhor-win`/FFI
- [ ] No new dependency without a license/reason note
