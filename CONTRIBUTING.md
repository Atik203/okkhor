# Contributing

## Setup

Install Rust stable 1.85 or newer. On Windows, use MSVC for production builds.
The current development host uses a user-level GNU toolchain plus MSYS2 mingw64;
see `AGENTS.md` for the exact shell setup.

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Workflow

1. Pick one roadmap milestone or one small vertical slice.
2. Read the relevant docs and ADR.
3. Add or update tests first for core behavior.
4. Implement the smallest coherent change.
5. Measure hot-path changes before and after.
6. Update documentation in the same change.
7. Run the checks and include the output in the PR.

## Commits

Use Conventional Commits:

```text
feat(learn): add recency-weighted word ranking
fix(win): preserve focus when showing suggestions
docs(adr): record native popup decision
test(core): cover Bengali grapheme boundaries
```

Keep commits focused. Do not mix formatting-only changes with behavior changes.

## Pull requests

- Explain the problem, the change, and what was measured.
- Link the roadmap item or ADR.
- List tests run.
- Call out privacy, compatibility, or performance impact.
- Do not include generated binaries, proprietary assets, secrets, or user data.

## Scope

This is a long project. Do not implement future phases early. Do not add a new
crate or process without an ADR. If a requirement is unclear, ask before
building.
