# Architecture Decision Records

This directory contains Architecture Decision Records (ADRs) for the
omnizip-rs workspace. Each ADR captures **why** a major architectural
decision was made, what alternatives were considered, and what
tradeoffs were accepted.

## What is an ADR?

An ADR is a short markdown document that records a single
architectural decision. ADRs are immutable once accepted — to change
a decision, write a new ADR that supersedes the prior one (with an
explicit cross-link).

## Why ADRs?

- **Onboarding** — new contributors can read 10 ADRs in 30 minutes
  and gain the mental model that took years of trial-and-error.
- **Alignment** — when a question re-surfaces in code review, point
  to the ADR rather than re-litigating.
- **Audit trail** — the rationale for "why is it this way?" doesn't
  die when a contributor leaves.

## ADR format

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

See [`templates/adr-template.md`](templates/adr-template.md) for a
copy-paste starting point.

## Numbering

ADRs are numbered `NNNN` in the order proposed. Once a number is
assigned, it is never reused. Superseding ADRs use the next number
and reference the prior ADR's number in the status line.

## Workflow

1. **Propose**: write a draft ADR as a markdown file in `docs/adr/`.
   Open a PR with the `docs` label.
2. **Discuss**: reviewers challenge the Context, Decision, and
   Consequences. Update the ADR in place.
3. **Accept**: once consensus, change status to `accepted`. Merge PR.
4. **Supersede**: if a later decision replaces this one, write a new
   ADR with status `accepted` and update this one's status to
   `superseded by ADR-MMMM`.

## Index

- [ADR-0001: Pure-Rust only (`#![forbid(unsafe_code)]`)](0001-pure-rust-only.md)
- [ADR-0002: One crate per algorithm family](0002-one-crate-per-family.md)
- [ADR-0003: Codec trait + CodecRegistry (OCP)](0003-codec-trait-registry.md)
- [ADR-0004: Determinism as a hard requirement](0004-determinism-hard-requirement.md)
- [ADR-0005: Differential parity vs C/Ruby references](0005-differential-parity.md)
- [ADR-0006: Rebase-merge only, never push to main](0006-rebase-merge-only.md)
- [ADR-0007: Brotli from-spec encoder (Phase C)](0007-brotli-from-spec-encoder.md)
- [ADR-0008: HashChainMatchFinder in omnizip-codecs](0008-shared-match-finder.md)
- [ADR-0009: Workspace lints (clippy, forbid unsafe)](0009-workspace-lints.md)
- [ADR-0010: Two-source-of-truth docs](0010-two-source-of-truth-docs.md)
