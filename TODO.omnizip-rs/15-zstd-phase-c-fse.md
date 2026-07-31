# 15 — ZSTD Phase C: FSE entropy tuning + sequences + multi-block

- **Priority:** P1
- **Depends on:** [14](14-zstd-phase-b-encoder.md)
- **Estimated effort:** 3–4 weeks
- **Crate:** `omnizip-zstd`

## Goal

Extend the Phase B encoder to full ZSTD ratio: optimal FSE table
selection, sequence-mode encoding, multi-block frames with size-based
flushing, and higher levels (4–22 equivalent).

## Phase C scope

1. **Optimal FSE table selection** (2 weeks): for each sequence symbol
   alphabet (literal lengths, offsets, match lengths), search the FSE
   table parameter space to find the lowest-bit-cost table. The C
   reference calls this `ZSTD_optimal_t` — consult `lib/compress/zstd_opt.c`
   for the algorithm; port to Rust.
2. **Sequence-mode literals** (1 week): encode literals as a single
   Huffman + FSE-compressed block (more efficient than per-literal Huffman).
3. **Multi-block frames** (3 days): flush blocks based on size thresholds
   (default 128 KB) and ratio feedback. Reset entropy tables on block
   boundaries.
4. **Levels 4–22** (1 week): map LimniFS levels 4–22 to ZSTD encoder
   parameters (window size, hash log, chain log, search log, search depth).
   Match the reference `zstd` preset table.
5. **Optional: long-distance matching (LDM)** (1 week, post-v1): for
   ultra levels (19+). Consult `lib/compress/zstd_ldm.c`.

## Acceptance

- **Differential gate:** Ruby and Rust produce byte-identical output at
  every level 4–9 on every corpus fixture. (Levels 10–22 may diverge from
  Ruby if the Ruby doesn't implement them — document any divergence.)
- **C reference gate:** Rust encoder output at every level decompresses
  byte-identically through reference `zstd -d`.
- Ratio within 5% of reference `zstd -9` on Silesia.
- Ratio within 3% of reference `zstd -19` on Silesia.
- Encode throughput ≥ 20 MB/s at level 6 single core.
- Clippy clean, no `unsafe`, deterministic.

## Implementation notes

- FSE table selection is the main ratio driver. The C reference uses
  heuristics + brute-force search over a small parameter space; port the
  heuristics, not the brute force.
- LDM is gated behind a feature flag `ldm` because it increases memory
  usage significantly.
- Multi-threaded encoding (task [33](33-multi-threaded-encoding.md)) is a
  separate concern, NOT in Phase C. Phase C is single-threaded.
