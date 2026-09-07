# 03 — zstd fast/intermediate tiers: ~22× slower for 13% smaller

- **Priority:** P0
- **Score evidence:** words L6 **I=19.4** (T=22.3×, S=0.870). L19:
  I=2.5 (T=2.4×, S=1.001) — the opt tier is fine; the problem is
  L1–L6 running DP-shaped parses where the reference runs dfast/lazy.
- **Status:** pending

## Root cause

Our from-spec encoder's intermediate tiers reuse the greedy/bank or
single-pass parse machinery. The reference's `ZSTD_compress_fast` /
`ZSTD_compressDoubleFast` (levels 1–3) and `lazy` (4–6) are dedicated
O(n)-with-small-constant matchers: dfast = two 4/8-byte hash tables,
no chains; lazy = one chain with depth 4–16 and early exit. Our L6
beats `zstd -6` by 13% because our parse is doing fundamentally more
work — that's the wrong tier for the level number.

## Plan (port from `~/src/external/zstd` — the reference source)

1. **Port `ZSTD_compressDoubleFast`** as a new parse path in the
   zstd encoder (match finder + rep handling; the emission/block
   machinery stays ours). Route L1–L3.
2. **Port `ZSTD_compress_lazy`** (fast/lazy/lazy2/btlazy segments per
   clevels.h — we already ported the four cparams tables in 0.21.59;
   the parse shapes are the missing half). Route L4–L6.
3. Keep the current parse as the L7+ or as a contest candidate
   (size-shielded) — our L6's 13% size win over ref -6 should become
   the L7-ish tier's win instead.
4. Re-validate against `zstd` CLI decode + round-trip + the
   cparams-tier unit tests (src/encoder/cparams.rs).

## Acceptance

- L1–L6 cells: T ≤ 3×, S ≤ 1.02 vs the reference at the same level.
- Differential/conformance gates green; byte-determinism tests green.
