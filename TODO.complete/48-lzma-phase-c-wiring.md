# Task 48: LZMA Phase C wiring

## Status: pending
## Priority: P0

## Problem

`Lzma1Encoder::encode` emits literal-only. The match finder exists but
isn't driven.

## Plan

- Add `rep0` field + `dict_size` field to `Lzma1Encoder`.
- Add `encode_match(distance, length, pos_state, output)` — follows
  `encode_eopm` pattern with real values.
- Modify `encode_literal` to use `encode_matched` when
  `state.is_match_context()`.
- Rewrite `encode()` to drive `MatchFinder` with greedy parsing.
- Thread `dict_size` from `Lzma2Encoder`.

## Acceptance

- Existing LZMA1 round-trip tests pass.
- Repetitive input compresses better than literal-only.
- Determinism test passes.
