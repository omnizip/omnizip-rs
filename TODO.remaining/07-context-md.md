# Task 07: CONTEXT.md domain glossary

## Status: done (2026-08-29)

Shipped at repo root as CONTEXT.md: two product layers (codec vs
container), codec-family/wire-format table, container inventory,
core concepts (parity, ratio, sweep, tier, match finder/hasher, bank,
parser, metablock/block/member, interop gate, determinism recording,
differential harness, release train, LimniFS, reference), each
cross-referenced to ADR-0001..0010. CLAUDE.md now points to it as
the third source-of-truth doc.

## Problem

The project has 10 ADRs but no CONTEXT.md domain glossary. The architecture skill references it but it doesn't exist. Domain terms are scattered across memory files, commit messages, and code comments.

## Action

Create CONTEXT.md with:
- Codec family names and their wire formats
- Container format names
- Key domain concepts (parity, ratio, sweep, tier, bank, hasher)
- Cross-references to the ADRs that constrain each concept

## Acceptance

- CONTEXT.md exists with accurate domain vocabulary
- Referenced by the architecture review process
