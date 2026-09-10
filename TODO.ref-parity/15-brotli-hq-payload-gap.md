# 15 — plists-class dense text at q11: 8.4% PAYLOAD gap in the hq port

- **Priority:** P1 (the largest unmapped size gap on the v17 board)
- **Score evidence (v17):** plists brotli q11 **S=1.0929**
  (114,479 vs 104,745) — 6x rfc's decomposed residual and never
  itself decomposed until 2026-09-10.
- **Status:** FIXED 2026-09-10 (v0.21.80). The suspected
  cost-model gap was WRONG — the model already had the UTF8 variant
  and both suspects measured out. The actual root cause (found by
  the dict_hits decoder probe): the sparse path disabled dictionary
  candidates in the BASE hq parse; ref emitted 2,189 dict matches /
  14,512 bytes, we emitted zero.

## Decomposition (BROTLI_STRUCT_DUMP, both streams, 2026-09-10)

| section | ours | ref |
|---|---|---|
| mlen / nbl l/c/d | 910,398 / 29/24/24 | 910,398 / 32/24/25 |
| ntrees l / d | 64 / 25 | 55 / 27 |
| littrees bits | 13,520 | 6,201 |
| cmdtrees bits | 9,456 | 6,676 |
| disttrees bits | 4,060 | 3,287 |
| **payload bits** | **883,854** | **815,708 (+8.4%)** |

Total gap 9,773 B = headers +1,322 B (tree excess, the known root-2
shape) **+ payload +8,451 B**. The payload dominates: this is a
PARSE/encoding quality gap, not wire format.

Our payload split: lit 364,911 + cmd_sym 178,251 + cmd_extra 41,429
+ dist_sym 114,951 + dist_extra 185,436.

## Facts established

1. plists (910 KB) takes the SPARSE contest path (n > 256 KiB) —
   hq + a/b + treecap only, no bt/dict/iter candidates. Ref's own
   parse is the same hq class (H10 zopfli) — the gap is OUR hq
   port's quality, not candidate routing.
2. **Measured out**: sliding-window literal costs on text
   (BROTLI_SW_LIT_TEXT probe, new): byte-identical output — the
   file's literal entropy is stationary.
3. **Measured out**: distance parameters — the q10+ cost search
   runs (ndirect 0 vs ref's 1 is per-parse optimal, not a gap).

## Primary suspect (next session)

The hq cost model's LITERAL COSTS: upstream's ZopfliCostModel
computes literal costs per CONTEXT (the context histogram feeds the
DP); ours uses a single global (or per-metablock) literal cost
table for text ("Text keeps the global table (their UTF8 variant
differs; measured later)" — never measured until now). Dense JSON
with mixed contexts (quotes/braces/string-vs-number literals) is
exactly where context-aware costs change parse decisions. Compare
`zopfli_hq.rs`'s cost model against upstream's ComputeLiteralsCosts
(backward_references_hq.rs) — if ours is context-blind, port the
context-aware costs. Expected payoff: plists q11 up to −8% payload;
possibly the whole q9 S-cluster (1.02-1.07) shares the root (the
q9 zopfli-iterative path).

## Acceptance

- plists q11 S <= 1.02; q9-cluster S re-measured; output changes
  only through the contest shields (parse candidate quality, not
  routing).

## Resolution (v0.21.80)

The port-session suspects were all wrong — the hq cost model already
ports the UTF8 variant (is_mostly_utf8 branch present), SW-on-text
was byte-identical, MLEN_CAP neutral, distance params searched. The
decoder-side dict_hits comparison (BROTLI_DICT_COUNT) found it
directly: **the reference's stream uses the static dictionary 2,189
times; ours used it zero times** — the n > 256 KiB sparse path ran
the base hq parse with dict disabled, and plists-class dense text
lives exactly there.

Fix: base-parse dictionary candidates for the sparse class, gated by
the density screen. Small files keep the shielded hq_d candidate
(the unshielded replacement regressed csv_100k +7%, caught by the
regression gate pre-merge — the catch also exposed that a premature
baseline refresh can mask a regression; refresh only AFTER the gate
passes on the intended code).

plists q11 S 1.093 -> 1.024; the changed cells measure T 0.4-0.5x
(faster than ref — shorter command streams); plists q11 is now a
net-win cell (I ~0.5). Remaining S=1.024: dict hits 1,799 vs ref
2,189 — the dict-match finder's recall gap, a smaller follow-up.
