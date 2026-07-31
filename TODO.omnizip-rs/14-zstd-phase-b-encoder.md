# 14 — ZSTD Phase B: encoder core (single-segment, Huffman literals)

- **Priority:** P1
- **Depends on:** [13](13-zstd-phase-a-decoder.md)
- **Estimated effort:** 2–3 weeks
- **Crate:** `omnizip-zstd`

## Goal

Port the ZSTD encoder skeleton: frame write, block write (raw / RLE /
compressed), Huffman literal encoding, match finding (basic hash chain),
sequence emission. Produces valid ZSTD frames at levels 1–3.

## Ruby → Rust module map

| Ruby source | Rust module | LOC |
|---|---|---:|
| `zstandard/encoder.rb` | `encoder.rs` | 228 |
| `zstandard/huffman_encoder.rb` | `huffman/encoder.rs` | 336 |
| `zstandard/literals_encoder.rb` | `literals/encoder.rs` | 248 |
| `zstandard/fse/encoder.rb` | `fse/encoder.rs` | 322 |

The Ruby encoder is relatively simple (228 LOC top-level) — it produces
single-segment frames with Huffman literals and basic match finding.

## Phase B scope

1. **Huffman encoder** (1 week): port `huffman_encoder.rb`. Build optimal
   Huffman trees, encode literals. Test against Ruby byte-for-byte.
2. **Literals encoder** (3 days): port `literals_encoder.rb`. Wraps
   Huffman with the literals block header.
3. **FSE encoder** (1 week): port `fse/encoder.rs`. Build FSE tables and
   encode sequences. This is the entropy-coding counterpart of the FSE
   decoder from Phase A.
4. **Match finder** (1 week): the Ruby encoder doesn't expose a separate
   match finder; it uses simple hash-chain inline. Port that, then refactor
   into a `match_finder.rs` module for Phase C's optimal parser.
5. **Top-level encoder** (3 days): port `encoder.rs`. Wire Huffman literals
   + sequences + frame header.

## Acceptance

- **Differential gate:** Ruby and Rust produce byte-identical output at
  levels 1–3 on every corpus fixture.
- **C reference gate:** Rust encoder output decompresses byte-identically
  through reference `zstd -d`.
- Ratio within 10% of reference `zstd -3` on Silesia.
- Encode throughput ≥ 30 MB/s at level 1 single core.
- Clippy clean, no `unsafe`, deterministic.

## Implementation notes

- The Ruby encoder is simpler than reference `zstd` (no multi-threading, no
  dictionary training, no long-distance matching). Phase B keeps that
  simplicity; Phase C extends.
- Frame header: single-segment, no checksum, no dictionary. Matches the
  Ruby's default.
- Block boundaries: the Ruby flushes one block per encode call; Rust should
  do the same for parity, then refactor to size-based flushing in Phase C.
