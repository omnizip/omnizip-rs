# TODO 150: Multi-platform CI matrix

## Problem

Local dev is macOS. CI runs Linux only. Windows untested. LimniFS
runs on all three.

## Proposed fix

`.github/workflows/ci.yml` runs on every PR:

- **Matrix**: `os: [ubuntu-latest, macos-latest, windows-latest]`
  × `toolchain: [stable, beta]`.
- **Steps**:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings` (allow
    existing pedantic warnings on warnings-as-errors basis).
  - `cargo test --workspace`
  - `cargo test --workspace --test differential` (subset, if ref
    tools available)
- **Fail-fast**: false (one platform failing doesn't cancel others).
- **Caching**: `Swatinem/rust-cache@v2`.

## Acceptance criteria

- [ ] Workflow runs on every PR.
- [ ] All 6 matrix combinations pass on a clean PR.
- [ ] Total wall time < 15 minutes.

## Priority

P1 — LimniFS deploys on all three platforms; we need to verify.
