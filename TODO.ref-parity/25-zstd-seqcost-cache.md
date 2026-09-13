# Task 25 — zstd: per-candidate table-cost caching in encode_section

Status: done (2026-09-13, shipped v0.21.87)
Board cells: words zstd L1 3.7 (worst non-accepted cell), sequence-heavy tiers

## Profile (words L1, post-v0.21.86, hot-loop 4660 samples)

words L1 decomposes differently from fits: `encode_section` 2028 (44%) +
`section_size_bits` 606 (13%) + parse 1985 (43%). The literals machinery
that owned fits is minor on text.

## Change

The table-mode decisions (`pick_table` + the FSE gates) re-measured the
full 3-stream section per candidate pair — each a complete
`sequences_bitstream_bits` walk plus three ctable builds. But the
decomposition is exact: each FSE stream's state cost is independent,
the LL/ML/OF extra bits are table-independent constants (k), and the
`ceil((bits+1)/8)` padding is computable from cached sums. Now each
stream's ≤3 candidates (fse, predefined, rle) are costed once
(`stream_payload`: one single-stream walk + one ctable build, header
length included), and every comparison is O(1) arithmetic over the
cached `StreamCost`s — bit-for-bit the same decisions.

`section_size_bits` / `sequences_bitstream_bits` (the old full-measure
path) deleted.

## Measured

- Byte-identity: 33/33 corpus cells exactly match v0.21.86; all
  round-trips via own decoder + zstd CLI.
- Time (interleaved RUNS user CPU, load 10-27): words L1 −2.6%
  (5.877→5.725 s/150; 1.57/1.32/1.30 → 1.58/1.29/1.27), csv2m L6 −6%,
  words L6 −1%, rustsrc L6 −2%. Estimation was a smaller slice of
  encode_section than the profile suggested (the final emission +
  per-sequence code building are inherent walks).
- No board cell moves (all deltas under the 0.1 I rounding); shipped
  on the strictly-less-work byte-identical precedent (the BitWriter
  word-staging). The structural value: task 21's splitter
  (`estimate_partition` → `encode_content_parts` → `encode_section`)
  pays the cached costs per partition too.
- Gates: 191+1 tests, fmt, clippy (CI invocation).

## Residual (words L1 = 3.7, the worst non-accepted cell)

Post-change shape: parse (`compress_block_fast4`) ~43%, encode_section
~53% (final emission + code building are the inherent walks), literals
~10%. No single component >45% — words L1 is at the per-op floor of
this parser shape; further movement needs the task-21-class structural
changes or per-op constants (SIMD table encoding).
