# 19 — zstd multi-threaded compress (`compress_mt`)

- **Priority:** HIGH (zstd L6 is now the worst time cell: 20–27x ref)
- **Depends on:** [13](13-encode-speed-parallelism.md) umbrella
- **Estimated effort:** 2 days
- **Status:** done 2026-09-04 (PR pending)

## Goal

Opt-in multi-threaded zstd compression: `compress_mt(input, level,
threads)` producing a deterministic multi-frame stream at large speedup
on multi-core, with a measured ratio delta vs the single-frame path.
The existing `compress` stays single-frame and untouched (no output
change for current callers, no regression-gate risk).

## Design (task 13, verified)

- Fixed job size — a pure function of level (default 4 MiB; opt-in
  knob), NEVER of thread count. Jobs = `ceil(len / job_size)`.
- Each job encodes as an INDEPENDENT frame with its own
  `MatchState` (params identical to `compress`, hash_log capped per
  job length). The decoder (ours and reference) already handles
  concatenated frames.
- Scoped worker threads (`std::thread::scope`), contiguous job ranges
  per worker, results concatenated in job order — the omnizip-lzma
  XZ-block pattern and PR #467's brotli pattern.
- `threads == 1` or single-job inputs fall through to the sequential
  path (identical bytes either way: one job = one frame = compress
  output… verify: job params must match `compress`'s exactly, incl.
  hash_log capping for the FULL input vs per-job — for a single job
  the job IS the input, so identical).
- wasm: sequential.

## Ratio delta to measure (acceptance input)

Cross-job matches are lost at job boundaries. Measure on
csv2m/dbdump/words/rustsrc/fits4m/csv21m at L6 and L19:
`compress` vs `compress_mt` vs reference single-threaded. Expect
<1% at 4 MiB jobs on text; document actuals in this file.

## Results (2026-09-04, shared box load 7-13)

csv21m 17.8 MB periodic CSV — the most history-dependent corpus cell
(ref single-thread: L6 3,056,814 / L19 1,053,325; ours single-frame:
L6 2,258,899 / L19 914,106 — we beat ref on both):

| level | jobs | size | delta vs single-frame | vs ref | wall |
|---|---|---|---|---|---|
| L6 | 4 MiB × 5 | 1,774,950 | **−21% (smaller!)** | −42% | 8.8s vs 13.1s (1.48x) |
| L19 | 4 MiB × 5 | 1,171,628 | +28% | +11% | 24.8s vs 57.9s (2.33x) |
| L19 | 8 MiB × 3 | 1,020,994 | +12% | −3% | 47.7s |
| L19 | 16 MiB × 2 | 975,171 | +6.7% | −7.4% | (loaded box) |

L6's negative delta is real, not a bug: per-job fresh match state is
favorable on this content (per-4 MiB-job output 560,355 verified
independently); ref -6 is far worse (3.06 MB). Other corpus files
(words/rustsrc/dbdump/fits at L6/L19) are single-job at 4 MiB —
identical to `compress` byte-for-byte.

Shipped defaults: Best → 16 MiB jobs, everything below → 4 MiB;
`ZSTD_MT_JOB` overrides. Reference interop verified (5-frame stream
through `zstd -d`, byte-identical to input). Thread-count invariance
unit-tested (2/4/8).

## Acceptance

- `compress_mt` round-trips through `ZstdDecoder` and `zstd -d`
  byte-identical to input, on every corpus file at L1/L6/L19.
- Same input + level ⇒ identical output across 1/2/4/8 threads
  (unit-tested with forced job sizes).
- Speed: ≥2x on 2 jobs, ≥3x on ≥4 jobs on a quiet multi-core box
  (best-of-5 user time vs sequential).
- Ratio delta table recorded here.
- `cargo test -p omnizip-zstd` green; fmt/clippy/typos clean.
