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
- [ ] `cargo run --package akkhor-daemon -- self-check`
- [ ] hot-path benchmark before/after (if applicable)

## Risk

<!-- Privacy, security, compatibility, performance, migration. -->

## Checklist

- [ ] Tests added or updated
- [ ] Docs updated in the same change
- [ ] No secrets, user text, or proprietary assets
- [ ] No new `unsafe` outside `akkhor-win`/FFI
- [ ] No new dependency without a license/reason note
