# TODO 155: Security audit + cargo-audit CI

## Problem

No `cargo audit` integration. Vulnerable dependencies could ship
unnoticed.

## Proposed fix

1. Add `cargo audit` to CI.
2. Document the security disclosure process in `SECURITY.md`.
3. Add `dependabot.yml` for automatic dep update PRs.
4. Pin critical deps (`wide`, `proptest`, `clap`) to specific minor
   versions; flag major bumps for review.

## Acceptance criteria

- [ ] `cargo audit` runs on every PR + nightly.
- [ ] `SECURITY.md` documents how to report vulnerabilities.
- [ ] `dependabot.yml` opens tracked update PRs.
- [ ] No advisories open on the workspace.

## Priority

P1 — supply-chain safety.
