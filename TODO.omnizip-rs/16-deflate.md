# 16 — DEFLATE / DEFLATE64

- **Priority:** P1 (universal compatibility codec)
- **Depends on:** [01](01-codec-trait-registry.md), [02](02-cross-language-differential-harness.md)
- **Estimated effort:** 1 week (DEFLATE) + 1 week (DEFLATE64)
- **Crate:** `omnizip-deflate`

## Goal

Port DEFLATE (RFC 1951) and DEFLATE64 (the Microsoft extended variant).
Decode is universal (every gzip / zlib / PNG file). Encode gives LimniFS
interoperability with the gzip ecosystem.

## Ruby → Rust module map

### DEFLATE (110 LOC)

| Ruby source | Rust module | LOC |
|---|---|---:|
| `deflate/decoder.rb` | `decoder.rs` | ~60 |
| `deflate/encoder.rb` | `encoder.rs` | ~50 |

The Ruby DEFLATE is minimal. Augment with `miniz_oxide` (pure Rust, MIT) as
a reference for the encoder's match finder and Huffman code construction.

### DEFLATE64 (783 LOC)

| Ruby source | Rust module | LOC |
|---|---|---:|
| `deflate64/decoder.rb` | `deflate64/decoder.rs` | ~400 |
| `deflate64/encoder.rb` | `deflate64/encoder.rs` | ~380 |

DEFLATE64 extends DEFLATE with a 64 KB window (vs 32 KB) and longer match
lengths (up to 65538 vs 258). Microsoft proprietary; the Ruby is a clean-
room port.

## Acceptance

- **DEFLATE decode:** every `.gz`, `.zlib`, and `.zip` (deflate-compressed)
  fixture decompresses byte-identically to Ruby and to `gzip -d`.
- **DEFLATE encode:** Rust encoder output decompresses byte-identically
  through `gzip -d`.
- **DEFLATE64 decode:** every `.zip` fixture with deflate64 entries
  decompresses byte-identically to Ruby.
- Ratio within 5% of `gzip -9` on Silesia for DEFLATE.
- Encode throughput ≥ 50 MB/s at level 6.
- Clippy clean, no `unsafe`, deterministic.

## Implementation notes

- DEFLATE's Huffman code construction is well-specified; the Ruby is a
  faithful implementation. Port directly.
- The match finder is the encoder's hot path. `miniz_oxide`'s match finder
  is a good reference for performance; the Ruby's is simpler (slower).
- DEFLATE64 is decode-only in many ecosystems (no mainstream encoder).
  Implement decode first; encode is lower priority.
