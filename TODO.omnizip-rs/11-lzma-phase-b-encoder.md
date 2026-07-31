# 11 — LZMA Phase B: encoder core (levels 0–3)

- **Priority:** P1
- **Depends on:** [10](10-lzma-phase-a-decoder.md)
- **Estimated effort:** 3–4 weeks
- **Crate:** `omnizip-lzma`

## Goal

Port the LZMA encoder skeleton + fast optimal parser + the literal, length,
and distance encoders. Produces valid LZMA streams at levels 0–3 equivalent
to `xz -0` through `xz -3`.

## Ruby → Rust module map

| Ruby source | Rust module | LOC | Notes |
|---|---|---:|---|
| `lzma/range_encoder.rb` | `range_coder/encoder.rs` | 202 | base range encoder |
| `lzma/xz_range_encoder.rb` | `range_coder/xz_encoder.rs` | 223 | XZ variant |
| `lzma/xz_range_encoder_exact.rb` | `range_coder/xz_exact.rs` | 314 | exact-price variant |
| `lzma/xz_buffered_range_encoder.rb` | `range_coder/xz_buffered.rs` | 323 | buffered variant |
| `lzma/literal_encoder.rb` | `coder/literal_encoder.rs` | 208 | context-coded literals |
| `lzma/length_coder.rb` (encode half) | `coder/length_encoder.rs` | ~90 | length encode |
| `lzma/distance_coder.rb` (encode half) | `coder/distance_encoder.rs` | ~170 | distance encode |
| `lzma/xz_encoder.rb` | `encoder/xz.rs` | 420 | top-level XZ encoder |
| `lzma/xz_encoder_fast.rb` | `encoder/xz_fast.rs` | 640 | fast optimal parser |
| `lzma/xz_price_calculator.rb` | `price.rs` | 167 | cost model for parsing |

## Phase B scope (low-level encode)

1. **Range encoders** (1 week): port all four range encoder variants. They
   share a common trait `RangeEncoder`; the variants differ in buffering
   and exactness. Test each against the Ruby reference byte-for-byte.
2. **Coder encoders** (3 days): `literal_encoder`, `length_encoder`,
   `distance_encoder`. These produce the LZMA bitstream from coder state.
3. **Price model** (2 days): `price.rs`. The cost model that drives the
   fast optimal parser. Port the price tables verbatim.
4. **Fast optimal parser** (1.5 weeks): `encoder/xz_fast.rs` (640 LOC).
   This is the level 0–3 encoder. Ported from
   `lzma_encoder_optimum_fast.c` originally; the Ruby is already a faithful
   port, so we port Ruby → Rust.
5. **Top-level XZ encoder wiring** (3 days): `encoder/xz.rs`. Wires the
   parser + coders together.

## Acceptance

- **Differential gate:** for each fixture in `tests/corpus/`, encode at
  levels 0, 1, 2, 3 with both Ruby and Rust. Assert byte-identical output.
- **C reference gate:** Rust encoder output at each level decompresses
  byte-identically through reference `xz -d`.
- **Round-trip gate:** Rust encode + Rust decode returns original input.
- Ratio within 15% of reference `xz -3` on Silesia (the fast parser leaves
  ratio on the table; Phase C closes the gap).
- Encode throughput ≥ 10 MB/s at level 2 single core.
- Clippy clean, no `unsafe`, deterministic output.

## Implementation notes

- **Determinism is non-negotiable.** The encoder MUST produce byte-identical
  output across runs, threads, and Rust versions. The Ruby is deterministic;
  preserve that.
- The price tables are precomputed constants. Generate them at build time
  via a `build.rs` if the Ruby regenerates them, or copy verbatim if static.
- The fast parser makes locally-optimal decisions (greedy + 1-lookahead).
  Don't be tempted to "improve" it — that's Phase C territory and changes
  the byte output.
