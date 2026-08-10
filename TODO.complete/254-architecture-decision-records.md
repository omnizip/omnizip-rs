# 254 — Architecture Decision Records (ADRs)

- **Priority:** P3 (documentation — onboarding, alignment)
- **Crate:** workspace (`docs/adr/`)
- **Depends on:** none
- **Estimated effort:** 2 days

## Problem

Major architectural decisions are scattered across:

- `CLAUDE.md` (workspace-level invariants)
- `PLAN.md` (LZMA + ZSTD porting plan)
- `TODO.complete/README.md` (status table)
- Various BUGREPORT files (incident-driven rationale)
- Code comments (per-module rationale)

New contributors (or future-you) have no single place to learn WHY
the workspace is structured this way. Decisions made and superseded
aren't visible. Refactors re-litigate settled debates.

## Design

### ADR format

Follow the widely-adopted Michael Nygard format:

```markdown
# ADR-NNNN: Title

- **Status:** proposed | accepted | deprecated | superseded by ADR-MMMM
- **Date:** YYYY-MM-DD
- **Deciders:** names of who participated

## Context

What is the issue being addressed? What constraints apply? What
alternatives were considered?

## Decision

What we decided to do.

## Consequences

Positive: ...
Negative: ...
Neutral: ...

## References

Links to related PRs, issues, external docs.
```

### Initial ADRs to write

1. **ADR-0001: Pure-Rust only (`#![forbid(unsafe_code)]`)**
   - Why: auditability, WASM compat, no C toolchain
   - Tradeoff: slower than C reference for some operations

2. **ADR-0002: One crate per algorithm family**
   - Why: independent versioning, feature flags, compile parallelism
   - Tradeoff: workspace is wide

3. **ADR-0003: Codec trait + CodecRegistry (OCP)**
   - Why: adding codecs without modifying dispatch
   - Tradeoff: dynamic dispatch overhead (small)

4. **ADR-0004: Determinism as a hard requirement**
   - Why: LimniFS content-addressed storage
   - Tradeoff: no HashMap iteration in encode paths

5. **ADR-0005: Differential parity vs C/Ruby references**
   - Why: avoid regressions vs reference implementations
   - Tradeoff: CI runtime, fixture management

6. **ADR-0006: Rebase-merge only, never push to main**
   - Why: review discipline, CI gating
   - Tradeoff: slower than direct push

7. **ADR-0007: Brotli from-spec encoder (Phase C)**
   - Why: ratio gap with vendored wrapper
   - Tradeoff: complexity, ongoing bug surface

8. **ADR-0008: HashChainMatchFinder in omnizip-codecs**
   - Why: DRY across LZMA, Brotli, LZ4_HC, ZSTD
   - Tradeoff: lowest-common-denominator API

9. **ADR-0009: Workspace lints (clippy, forbid unsafe)**
   - Why: catch bugs uniformly
   - Tradeoff: pedantic warnings need per-crate allow

10. **ADR-0010: Two-source-of-truth docs**
    - PLAN.md (LZMA/ZSTD) + TODO.complete/ (everything else)
    - Tradeoff: overlap requires manual sync

### File layout

```
docs/adr/
├── README.md                # how to write an ADR
├── 0001-pure-rust-only.md
├── 0002-one-crate-per-family.md
├── ...
└── templates/
    └── adr-template.md
```

### Workflow

- Any non-trivial decision gets an ADR before code is written.
- ADRs are numbered (NNNN) in the order proposed.
- Once accepted, an ADR is immutable except for status changes.
- To change a decision, write a new ADR that supersedes the prior
  one (with explicit cross-link).

## Acceptance criteria

- [ ] `docs/adr/README.md` explains the format and workflow.
- [ ] 10 initial ADRs written (0001-0010 above).
- [ ] Linked from `CLAUDE.md` and top-level `README.md`.
- [ ] At least one new ADR created during the next non-trivial
      decision (proves the workflow works).

## Why this matters

A new contributor reading ADRs 0001-0010 in 30 minutes gets the
mental model that took years of trial-and-error to build. Without
ADRs, the same questions get re-asked in every review.
