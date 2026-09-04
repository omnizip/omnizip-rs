# 13 — Encode speed: fresh measurement + deterministic multi-threading

- **Priority:** HIGH (the largest remaining practical gap for LimniFS)
- **Depends on:** nothing open; extends [TODO.omnizip-rs 33](../TODO.omnizip-rs/33-multi-threaded-encoding.md)
- **Estimated effort:** 1 week
- **Status:** in_progress 2026-09-04

## Fresh time table (2026-09-04, shared box, load 8-14, best-of-3
user-time ratios — re-measure quietly before citing)

- xz `-6` csv21m (17.8 MB): **0.97x ref** (MT block encoding already shipped)
- brotli q5: words 5.6x, rustsrc 4.7x, csv2m 6.1x, csv21m 5.1x
- brotli q9 (text = DP tier): words 30x, rustsrc 56x, csv2m 28x
- brotli q11: words 3.2x, rustsrc 3.8x; fits4m 19.2s vs ref 0.06s
  (pathological DP cell on synthetic counter data — the real-FITS
  question lands with task 15)
- zstd L6: words 20x, csv21m 27x; L19: words 2.5x, csv21m 1.6x

## Design decisions (verified in code, 2026-09-04)

- **brotli q10-11 byte-identical MT is feasible**: the encoder resets
  the rep ring at every chunk start (forcing 4 explicit long-form
  copies first), so emission carries no cross-chunk state; and
  `HashChainMatchFinder`'s chain walk BREAKS at the first
  beyond-window node (`walk_chain_ladder` / `find_match`:
  `dist > max_distance → break`), so a per-chunk MF primed by
  store-only `advance()` through the preceding window produces walks
  identical to the sequential shared MF. Prime window =
  `MAX_BACKWARD_DISTANCE`; construct each per-chunk MF over the FULL
  input (identical `prev` sizing ⇒ identical aliasing ⇒ identical
  candidates).
- **brotli q4-9 (bank tier) deferred**: `BankMatchFinder` state
  threading needs its own priming analysis.
- **zstd**: `compress_mt(input, level, threads)` — fixed job size
  (a pure function of input length, never of thread count), each job
  an independent frame via a per-thread `MatchState`, concatenated in
  job order. The decoder already handles multi-frame streams.
  Measure the ratio delta vs the single-frame path before shipping;
  opt-in API, `compress` unchanged.

## Goal

Bring worst-case encode wall time to ≤1.3x the reference CLI on
multi-core machines via deterministic multi-threading, without changing
single-threaded output bytes for the same input + level where that is
achievable, and where not, via an explicit opt-in API.

## Current state (corrected 2026-09-04)

The "12x slower q11" figure floating around is STALE — it predates the
0.16.58–0.16.63 DP-speed campaign and the 0.21.4–0.21.7 bank-scan/emit
passes. Last recorded standings (CPU time, loaded box — re-measure
before designing):

- brotli: CSV q5 ~2.3x ref, CSV q9 ~1.6x, CSV q11 ~2.5x,
  FITS q5 ~2.2x, FITS q9 ~1.14x ✓, FITS q11 0.93x ✓, q1 ✓
- zstd: opt tiers ~1.5x
- xz: **already MT** — XZ block payloads encode on scoped worker
  threads (≤4), byte-identical to sequential, sequential on wasm
  (shipped 0.16.91). This is the established repo pattern for
  deterministic MT.

The remaining single-thread gap is per-op constants under
`#![forbid(unsafe_code)]` (bounds-checked scans vs C raw loads) — each
remaining ST lever measured ≤2–5%. Parallelism is the one big lever
left.

## Determinism rules (hard requirements)

1. Chunk/job boundaries are a pure function of input length + level
   (fixed size, e.g. 4 MiB) — NEVER of thread count, load, or timing.
2. Results assemble by chunk index, not completion order.
3. Same input + level ⇒ identical bytes across runs, machines, thread
   counts. (CLAUDE.md invariant 3.)

## Approach

### zstd (task 33 phase 1)

Multi-frame: split input into fixed ~4 MiB jobs, encode each as an
independent frame, concatenate. The decoder already handles
multi-frame streams. Wire as an opt-in API surface (e.g.
`compress_with_threads`) or a Cargo feature — NOT a silent default
change, because output bytes differ from the ST path (cross-job
matches lost; ratio cost expected <0.5%, must be measured on the
10-file corpus + pathological inputs per invariant 1).

### brotli

Investigate byte-identical parallel chunk emission. Chunk state that
crosses boundaries today:

- **Shared MF / history window** (PR #280 threading): a per-chunk MF
  pre-primed by store-only inserts over the preceding window replay
  gives the identical MF state (inserts depend only on input bytes) ⇒
  identical parse. Feasibility: verify no parse-dependent inserts.
- **rep-cache carry across metablocks**: if the encoder threads reps
  from chunk N−1's emission into chunk N's parse, byte-identical
  parallelism is blocked; options: (a) reset at boundaries in the MT
  path (measure the ratio delta; 512KB-chunk "rep warmth" data
  suggests it is small at ≥2 MiB chunks), (b) sequential pre-pass
  that cheaply approximates. Decide by measurement.
- p1/p2 context carry is input-position-determined — parallel-safe.

If byte-identical is impractical, ship opt-in MT with its own
deterministic layout and a measured ratio delta, like zstd.

### xz

Already done — nothing to do unless re-validation fails.

## Acceptance

- Fresh time table (best-of-5, `/usr/bin/time` user time, quiet
  machine) for brotli q1/5/9/11 + zstd L1/6/19 + xz -6 on the 10-file
  corpus, ours-ST vs ours-MT vs ref, recorded in this file.
- MT paths: byte-identical across 1/2/4/8 threads (or documented
  bounded ratio delta for the opt-in API).
- MT wall time ≤1.3x ref on a 4-core-class machine for every cell
  where ST was >1.3x.
- `cargo test --workspace` green; regression gate green; no new deps
  beyond what task 33 allows (prefer `std::thread::scope` over rayon
  to keep the dep set unchanged).
- Worst-case analysis on pathological content (all-zeros, periodic,
  small repetitive structured text) — bounded work per thread, no
  unbounded queueing.

## Notes

- Do not re-open decode-speed work here — closed with three measured
  negatives.
- The repo's CI box and this dev machine are shared; use best-of-N
  interleaved runs and CPU time, never single wall readings.
