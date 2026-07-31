# 12 — LZMA Phase C: optimal parser + LZMA2 + XZ container (levels 4–9)

- **Priority:** P1
- **Depends on:** [11](11-lzma-phase-b-encoder.md)
- **Estimated effort:** 4–6 weeks
- **Crate:** `omnizip-lzma`

## Goal

Port the DP-based optimal parser (where LZMA's signature ratio comes from),
the LZMA2 chunked container, and the full XZ container with CRC64. Produces
streams at levels 4–9 equivalent to `xz -4` through `xz -9`.

## Ruby → Rust module map

### Optimal parser

| Ruby source | Rust module | LOC |
|---|---|---:|
| `lzma/optimal_encoder.rb` (mode: normal) | `encoder/optimal.rs` | ~400 (est.) |

The Ruby file currently has a `:fast` mode (Phase B); the `:normal` mode is
the DP parser and needs adding to both Ruby and Rust in lockstep.

### LZMA2 chunked container (906 LOC)

| Ruby source | Rust module | LOC |
|---|---|---:|
| `lzma2/constants.rb` | `lzma2/constants.rs` | 38 |
| `lzma2/properties.rs` | `lzma2/properties.rs` | 179 |
| `lzma2/lzma2_chunk.rb` | `lzma2/chunk.rs` | 161 |
| `lzma2/chunk_manager.rb` | `lzma2/chunk_manager.rs` | 180 |
| `lzma2/encoder.rb` | `lzma2/encoder.rs` | 143 |
| `lzma2/simple_lzma2_encoder.rb` | `lzma2/simple.rs` | 122 |
| `lzma2/xz_encoder_adapter.rb` | `lzma2/xz_adapter.rs` | 83 |

### XZ container (CRC64 + index)

| C reference | Rust module | Notes |
|---|---|---|
| `xz/liblzma/check/crc64.c` | `xz_container/crc64.rs` | 0BSD; port the table-based fast path |
| `xz/liblzma/common/stream_encoder.c` | `xz_container/stream.rs` | stream header + footer |
| `xz/liblzma/common/index_encoder.c` | `xz_container/index.rs` | block index |
| `xz/liblzma/common/block_header_encoder.c` | `xz_container/block_header.rs` | per-block header |

## Phase C scope

1. **Optimal parser** (3 weeks): port `optimal_encoder.rb` normal mode. This
   is dynamic programming over the input: at each position, compute the
   cost of literal-vs-match for every reachable future state, choose the
   minimum. The Ruby implementation is ~400 LOC of intricate DP; port
   carefully and verify byte-identical output against Ruby.
2. **LZMA2 container** (1 week): port the 7 Ruby files. LZMA2 chunks the
   input, resets the dictionary on chunk boundaries, and emits copy/reset
   chunk types. The chunk manager decides chunk boundaries based on ratio
   feedback.
3. **XZ container** (1.5 weeks): port CRC64, stream header/footer, block
   header, and index from the C reference (the Ruby doesn't implement the
   XZ container; omnizip Ruby's XZ encoder produces LZMA-alone inside an
   XZ wrapper via the adapter).
4. **Level 4–9 tuning** (1 week): map LimniFS levels 4–9 to LZMA preset
   parameters (dictionary size, nice_len, depth, num_fast_bytes). Match
   the reference `xz` preset table exactly.

## Acceptance

- **Differential gate:** Ruby and Rust produce byte-identical output at
  every level 4–9 on every corpus fixture.
- **C reference gate:** Rust encoder output at level 9 decompresses
  byte-identically through reference `xz -d`.
- **`xz -t` validation:** every Rust-produced `.xz` file passes `xz -t`
  (test integrity).
- Ratio within 5% of reference `xz -9` on Silesia.
- Ratio within 3% of reference `xz -6` on Silesia.
- Encode throughput ≥ 3 MB/s at level 6 single core.
- Clippy clean, no `unsafe`, deterministic output.

## Implementation notes

- The optimal parser is O(n·depth) in time and O(n) in memory. The Ruby uses
  a `MatchFinder` probe per position; Rust should reuse the match finder
  state across positions to avoid recomputation.
- LZMA2's chunk manager is heuristic-driven: it monitors compression ratio
  per chunk and resets the dictionary when ratio degrades. Port the
  heuristics verbatim; they are tuned by the original 7-Zip authors.
- CRC64: use the ECMA-182 polynomial (same as XZ). The fast path is a
  256-entry table; populate at compile time via `const fn`.
- The XZ stream header includes a magic number (`FD 37 7A 58 5A 00`),
  flags, and a CRC32. Don't forget the CRC32 over the flags — common bug.

## Why this completes LZMA

After Phase C, `omnizip-lzma` is a complete, production-grade pure-Rust
LZMA / LZMA2 / XZ implementation. LimniFS can produce and consume `.xz`
files that interoperate with the reference `xz` tool at every level, with
no C dependencies.
