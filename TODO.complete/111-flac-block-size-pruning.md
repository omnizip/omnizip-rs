# TODO 111: FLAC block-size sweep pruning

## Problem

`omnizip-flac/src/encoder/mod.rs::encode_stream` currently calls
`encode_stream_with_block_size` for every candidate in
`CANDIDATE_BLOCK_SIZES = [192, 256, 512, 1024, 2048, 4096, 4608, 8192, 16384]`.

For inputs larger than the smallest candidate this means up to 9
full encodings of the same audio. Each encoding is ~10-50 ms for a
typical 4 MiB block on commodity hardware, so frame selection alone
costs 100-500 ms — the dominant slice of FLAC encode time on real
audio.

## Root cause

The "try every candidate, pick the smallest" strategy mirrors
libFLAC's `--best` stream-level sweep. But libFLAC's per-block LPC
selection is already exhaustive at the block level; the stream-level
sweep is overkill for most inputs.

## Proposed fix

Pick the block size from a simple heuristic based on sample rate and
total sample count:

| Condition | Block size | Rationale |
|-----------|-----------|-----------|
| `total_samples < 256` | `total_samples.max(16)` | Tiny input — match |
| `total_samples < 4096` | `256` | Small — keep frame overhead low |
| `total_samples < 65_536` | `4608` | libFLAC default for 44.1 kHz |
| `total_samples ≥ 65_536` AND sample_rate ≥ 44_100 | `4608` | Standard audio |
| `total_samples ≥ 65_536` AND sample_rate < 44_100 | `4096` | Power-of-two wins |

Provide a `FlacEncoderOptions::try_all_block_sizes: bool` flag (default
`false`) for callers that explicitly want libFLAC `--best` semantics.

## Acceptance criteria

- [ ] Default encode picks a single block size via the heuristic table.
- [ ] `try_all_block_sizes = true` restores current behavior.
- [ ] Round-trip tests still pass on sine, DC, random, stereo inputs.
- [ ] Bench shows ≥ 5× faster FLAC encode on 32 KiB+ inputs.

## Priority

P0 — half of the FLAC 10× gap closes here.
