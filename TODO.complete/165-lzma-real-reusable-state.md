# TODO 165: LZMA real reusable state — probability-model warmup

## Problem

TODO 146 added `LzmaCompressor` but it only caches `LzmaOptions` —
no probability-model state carries across calls. Each `compress()`
rebuilds the bit models from cold.

For LimniFS's max-ratio tournament (where many small inputs go
through LZMA), the cold-start cost dominates.

## Scope

Real amortisation requires:

1. **Probability model reuse**: `BitModel` arrays (`is_match`,
   `is_rep`, `literal_encoder`, `length_encoder`, `distance_encoder`)
   persist across calls. Adaptation state carries forward.
2. **Match finder reuse**: hash table + chain persist (already
   done for ZSTD via `ZstdCompressor`).
3. **Dictionary warmup**: optional dictionary content seeded once,
   reused across calls.

The wire format includes "state reset" flags in LZMA2 chunk
headers, so the encoder can choose whether to reset on each call.

## Implementation plan

1. Extend `LzmaCompressor` with a `state: LzmaCompressorState`
   field holding all BitModel arrays.
2. Add a `reset_mode: ResetMode` knob:
   - `ResetAll` (current behaviour — full cold start)
   - `ResetState` (keep BitModels, reset rep offsets)
   - `Reuse` (carry everything forward)
3. LZMA2 chunk encoder respects the chosen reset mode per chunk.

## Acceptance criteria

- [ ] `LzmaCompressor` holds BitModel state across calls.
- [ ] Throughput on 100-call batches of 1 KiB inputs ≥ 3× current.
- [ ] Output may differ between reset modes — document the
  determinism contract per mode.

## Priority

P1 — LimniFS-flagged for the max-ratio tournament.
