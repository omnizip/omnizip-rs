# 173: Brotli — Q≥2 Encoder (Combined Insert+Copy Commands)

## Priority: P4 (deferred — current q=0..1 encoder is correct + small)

## Status: pending

## Context

The pure-Rust brotli encoder (`fast_encoder.rs`) is a verbatim port of
upstream's `compress_fragment_two_pass`, the q=0/q=1 fast path. It uses
the 3-tuple `INSERT + DISTANCE + COPY-LD` pattern, with separate
Huffman codes for each.

The q≥2 encoder (`compress_fragment.rs` and the optimal parser in
`backward_references_hq.rs`) emits combined INSERT+COPY commands via
`combine_length_codes(inscode, copycode, use_last_distance)`, producing
a single code in the 704-symbol alphabet. This achieves ~10–20% better
ratio at 10–100× the CPU cost.

## Why deferred

Our `compress_fragment_two_pass` encoder already produces valid brotli
that any conformant decoder (including ours, `brotli -d`, browsers)
accepts. The compression ratio is competitive with upstream's q=1.
LimniFS cares about determinism + round-trip integrity, not max ratio.

A q≥2 path would add ~5K LOC and significant complexity for a ratio
bump that doesn't unblock any consumer.

## If/when prioritised

1. Port upstream `compress_fragment.rs` (single-pass, q=2..6).
2. Port upstream `backward_references_hq.rs` (Zopfli-style optimal
   parser, q=7..11).
3. Wire `BrotliOptions::quality` through to dispatch on the three
   encoder tiers.
4. Keep `compress_fragment_two_pass` as the default for quality ≤ 1.

## Acceptance Criteria

- Round-trip via own decoder + `brotli -d` at every quality 0..11.
- Ratio on `enwik8` within 5% of upstream `brotli -q N`.
- No nondeterminism: same input + quality always produces identical
  bytes across runs, machines, and Rust versions.
