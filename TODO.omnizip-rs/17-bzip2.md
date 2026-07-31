# 17 — bzip2

- **Priority:** P1 (legacy high-ratio codec, still common)
- **Depends on:** [01](01-codec-trait-registry.md), [02](02-cross-language-differential-harness.md)
- **Estimated effort:** 2 weeks
- **Crate:** `omnizip-bzip2`

## Goal

Port bzip2 (BWT + MTF + RLE + Huffman). Decode is universal (every `.bz2`
file). Encode gives LimniFS access to the legacy high-ratio tier.

## Ruby → Rust module map (1,101 LOC)

| Ruby source | Rust module | LOC |
|---|---|---:|
| `bzip2/constants.rb` | `constants.rs` | ~80 |
| `bzip2/burrows_wheeler_transform.rb` | `bwt.rs` | ~250 |
| `bzip2/move_to_front.rb` | `mtf.rs` | ~80 |
| `bzip2/rle.rs` | `rle.rs` | ~100 |
| `bzip2/huffman.rb` | `huffman.rs` | ~200 |
| `bzip2/encoder.rb` | `encoder.rs` | ~200 |
| `bzip2/decoder.rs` | `decoder.rs` | ~190 |

## Phase scope

1. **Decoder** (1 week): port the decoder side. BWT inverse, MTF inverse,
   RLE inverse, Huffman decode. Read every `.bz2` fixture.
2. **Encoder** (1 week): port the encoder. Forward BWT (the hard part —
   needs suffix-array sort), MTF, RLE, Huffman encode.

## Acceptance

- **Differential gate:** Ruby and Rust produce byte-identical output at
  every level (1–9) on every corpus fixture.
- **C reference gate:** Rust output decompresses through `bzip2 -d`.
- Ratio within 5% of `bzip2 -9` on Silesia.
- Decode throughput ≥ 30 MB/s; encode throughput ≥ 5 MB/s.
- Clippy clean, no `unsafe`, deterministic.

## Implementation notes

- The Burrows-Wheeler Transform is the encoder's bottleneck. The forward
  transform needs a suffix-array sort; use the SA-IS algorithm (linear
  time). The Ruby likely uses a simpler O(n log² n) sort; Rust can do
  better but must match the Ruby's BWT output for differential parity.
- For differential parity, the SORT ORDER must match. If Ruby uses one
  suffix-array algorithm and Rust uses another, BWT outputs differ even
  though both are "correct" BWTs. Pin the algorithm in both.
- bzip2 levels 1–9 differ in block size (100 KB – 900 KB). Larger blocks
  give better ratio but slower encode.
