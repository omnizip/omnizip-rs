# Task 17 — zstd: L1 literal-encoding time (arena package-merge, lazy min-heap, batched flushes)

Status: done (2026-09-12, shipped v0.21.83)
Board cells: zstd L1 T-cluster (fits 4.4, csv2m 3.8, rustsrc 3.7, words 3.6, plists 3.1)

## Profile (fits4m L1, hot-loop `sample`, RUNS=200)

Both sides are sub-100 ms per single encode (ours ~45 ms, ref ~10 ms), so
the A/B uses RUNS-40/60 loops. Profile: 43% of encode in
`encode_literals_internal`:

| component | samples | share | cause |
|---|---|---|---|
| `build_weights` | 957 | 25% | O(n²) min-heap computed unconditionally and **discarded** when package-merge succeeds; package-merge's per-coin `Vec<usize>` sets = thousands of small allocations per block (leaf `vec![i]` ×256×11 levels + `clone`+`extend` per package) — measured realloc/free storm |
| `encode_huffman_stream` | 752 | 20% | one `flush()` call per literal |
| `compress_block_fast4_with_prefix` | 347 | 9% | the match finder itself |

## Change (output-preserving by construction)

`omnizip-zstd/src/huffman/package_merge.rs` — coins are
(weight, arena-range) into one shared `Vec<u32>` instead of per-coin
`Vec<usize>`. Merge order, sort key (`sort_unstable_by_key(weight)` on the
identical key sequence), and truncation sequence unchanged → identical
lengths.

`omnizip-zstd/src/huffman/encoder.rs` —
1. `huffman_lengths` computed only on the fallback path (invalid Kraft sum;
  never triggers on the corpus).
2. `encode_huffman_stream`: flush only when `bit_pos() > 52` (cannot take
  another 11-bit code); `flush()` drains whole bytes either way → identical
  byte stream.

`omnizip-zstd/src/fse/encoder.rs` — `BitCStream::bit_pos()` accessor.

Debug note: the first arena draft extended the arena with the source
*range values* (indices) instead of the contents — caught immediately by
the identity sweep (panic: `lengths[4547]` on an 89-symbol table), fixed
with explicit `copy_range`/`push_val` helpers. The lesson stands: the
byte-identity sweep is the gate that catches arena/pointer rewrites.

## Verification

- 33 corpus cells (11 files × L1/L6/L19): byte-identical sizes.
- Timing (RUNS-40 serial, shared box): fits L1 5.38→4.29s (**−20%**),
  words 7.45→6.19 (−17%), csv2m 2.40→2.07 (−14%), rustsrc 2.70→2.25
  (−16%), plists 1.14→0.57 (**−50%**), sqlite 0.24→0.12 (−49%).
  L6: fits −21%, words −30%; csv2m L6 unresolvable under box noise
  (same-binary run-to-run swings 2.7–4.0 s) — carried.
- Gates: 191+1 zstd tests, fmt, clippy (CI invocation), full CI on PR #565.

## Board impact (v20)

L1 T cells (S unchanged, byte-identical): fits 4.2→~3.35, csv2m
3.8→~3.25, rustsrc 3.7→~3.1, words 3.6→~3.0, plists 3.0→~1.5,
sqlite 2.2→~1.1. Expected worst cells after: fits brotli q11 3.4 /
fits zstd L1 ~3.7 (S=1.052 becomes the binding side).

## Residual

- fits zstd L1 S=1.052 is now the binding constraint on that cell (task 14
  inspected the S leads: Vec+copy ~5%, literals share sub-threshold).
- `encode_huffman_stream` per-symbol loop remains ~15% post-fix; the
  reference's 4-symbol batched state machine is the known next form.
- csv2m L6 T unresolved (box noise) — re-measure on a quiet box.
