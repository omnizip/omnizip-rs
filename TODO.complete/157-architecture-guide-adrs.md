# TODO 157: Architecture guide + ADRs

## Problem

No central architecture document. New contributors have to read
every crate to understand the design.

## Proposed fix

1. `docs/architecture.md` with:
   - Workspace layout (one crate per codec).
   - Codec trait + registry model.
   - Match finder sharing pattern.
   - Reusable compressor pattern.
   - Streaming / async direction.
2. `docs/adr/` directory for architecture decision records:
   - ADR-0001: one crate per codec
   - ADR-0002: forbid(unsafe_code)
   - ADR-0003: deterministic encoding required
   - ADR-0004: shared HashChainMatchFinder
   - ADR-0005: Reusable `*Compressor` pattern
   - ADR-0006: Phase A/B/C decoder-first porting strategy
3. Cross-link from `CLAUDE.md`.

## Acceptance criteria

- [ ] Architecture guide lands.
- [ ] At least 6 ADRs committed.
- [ ] CLAUDE.md links to architecture docs.

## Priority

P2.
