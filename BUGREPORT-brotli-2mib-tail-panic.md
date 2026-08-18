# omnizip-brotli: panic + silent corruption at chunk 8+ (file offset > 16 MiB)

## Summary

**RESOLVED.** Dictionary/LZ77 classification in every encoder walk used the
unclamped output position (`distance > mlen_offset + pos`) instead of the
decoder's rule (`distance > min(pos, MAX_BACKWARD_DISTANCE)`). Once a
metablock's base offset exceeds the 16 MiB backward window
(2 MiB chunking → chunk 8+, i.e. any input > ~16.8 MiB using dictionary
matches), dictionary distances — computed against the *clamped* threshold —
fall below the unclamped position and get misclassified as LZ77:

- **Panic** (the reported crash): the walk advances by word-length instead
  of transform-length, drifts, and slices past the chunk
  (`score_commands: range start index 2097156 out of range for slice of
  length 2097152`).
- **Silent corruption** (worse, same root): the emitted command decodes as
  a garbage-but-valid LZ77 copy at the misclassified distance — wrong
  bytes, no crash.

## Repro (exact)

`omnizip-brotli/examples/fits_repro.rs` mirrors limnifs-bench's
fits-synthetic generator (2880-byte ASCII FITS header + 25M big-endian
16-bit pixels, 8-pixel runs):

```
cargo run --release -p omnizip-brotli --example fits_repro -- 47700000 5
# pre-fix: panic; post-fix: OK 47700000 -> 28675617, reference-decode OK
```

4–8 MB inputs pass both ways (fewer than 8 chunks).

## Root cause (verified by instrumentation)

Parser/walk join (`BROTLI_CHAINTRACE`) names the first divergence:

```
ctx[29708] parser=(192668,192668,192673) ins=0 cpy=4 dist=16810215
```

Chunk 8 → `mlen_offset = 16,777,216 > MAX_BACKWARD_DISTANCE = 16,777,200`.
The dict distance `16,810,215 = 16,777,201 + address` exceeds the true
threshold (16,777,200) but is below the unclamped position (16,969,884) →
walk classifies LZ77 → advance 4 (word length) instead of 5 (transform
length) → drift −1, then amplifies (behind-drift misclassifies more
commands as dict; ahead-drift decodes wrong dictionary words) until the
walk crosses the chunk end.

The decoder (decoder.rs ~1262) uses exactly
`max_distance = min(pos, max_backward_distance)` — the fix clamps all 14
encoder-walk classification sites to match.

## Fix

`omnizip-brotli/src/from_spec_encoder.rs`: every
`distance > mlen_offset + <pos>` classification becomes
`distance > (mlen_offset + <pos>).min(MAX_BACKWARD_DISTANCE as usize)`
(14 sites: score_commands, score_commands_adaptive, build_symbol_stream,
emission simulation, extract_literals, rep_hint, DistCostModel,
rewrite_for_rep_codes, exact_emission_bits, is_literal_pos builder,
greedy/two_pass walks).

## Validation

- FITS 47.7MB q5: OK → 28,656,617B, `brotli -d` byte-identical
- FITS 47.7MB q11 + q9: (in flight at time of writing)
- CSV corpus + unit tests + ratio regression gate: see PR
