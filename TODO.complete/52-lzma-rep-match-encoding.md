# Task 52: LZMA — rep-match encoding + distance encoder completeness

## Status: pending
## Priority: P1

## Problem

The LZMA encoder doesn't emit rep-matches (is_rep=1). This misses
significant compression for data with repeated patterns at the same
distance. The distance encoder also has incomplete coding modes.

## Plan

- Add `encode_rep_match` (is_rep=1, rep code 0/1/2).
- Add `encode_short_rep` (is_rep=1, length=1, rep0).
- Track rep0/rep1/rep2 history from previous matches.
- Fix distance encoder for all slot ranges.

## Files

- `omnizip-lzma/src/encoder/lzma1.rs`
- `omnizip-lzma/src/coder/distance_encoder.rs`
