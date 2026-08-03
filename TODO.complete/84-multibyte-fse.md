# 84 — Multi-byte FSE decoder

**Priority:** Medium
**Source:** RESEARCH.md §5 (Multi-byte ANS encoding, ACM 2024)

## Context

Our FSE decoder (`omnizip-zstd/src/fse/`) consumes one symbol per
state transition. The paper *Efficient and Portable ANS Encoding
for Multi-Byte Integer Sequences* (ACM 2024) shows ~30% throughput
gain by processing 2-4 bytes per step.

The technique: precompute lookup tables that, given current state +
next K input bits, return K decoded symbols + new state. Trades
memory for throughput.

## Implementation

For each FSE table:
1. Build a level-1 decode table (current state → 1 symbol + new state).
2. Build a level-2 decode table (current state + 1 byte of input →
   up to 8 symbols + new state).
3. Decode inner loop uses level-2 table; falls back to level-1 for
   tail bytes.

Memory cost: ~16x the level-1 table size. For ZSTD this is ~64 KB.

## Acceptance criteria

- [ ] Level-2 decode table generator.
- [ ] Modified FSE decode loop using level-2 when input remains.
- [ ] ≥20% throughput improvement on Enwik8 (ZSTD level 19).
- [ ] Output byte-identical to current decoder (deterministic test).
- [ ] Workspace tests pass.

## Files

- `omnizip-zstd/src/fse/interleaved.rs` — new multi-byte decoder
- `omnizip-zstd/src/fse/mod.rs` — dispatch on table size
