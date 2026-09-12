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

## Follow-up: the dict recall gap (2026-09-11)

The 1,799-vs-2,189 recall gap decomposed cleanly with decoder-side
per-match logs (BROTLI_DICT_LOG) joined against a per-position match
table dump (BROTLI_HQ_MATCH_DUMP in `collect_matches`):

1. **The self-gate is NOT the blocker.** Only 111/529 missing
   (pos,len) pairs were gated out by `minlen = max(4, best_len+1)`
   (upstream's own FindAllMatchesH10 gate — verified the same shape,
   including its `best_len <= 2` short-scan early exit). The other
   418 were admitted at their length, many at positions where our
   tree found no match at all.
2. **The finder is NOT the blocker.** A scratch example calling
   `find_all_static_dictionary_matches` directly at all 418 missing
   positions found every word (mostly transform-0 4-letter words:
   "vice", "tour", "plus", "plan", ...).
3. **The blocker is DP rejection — and a pricing bug was found and
   deliberately NOT fixed.** `long_dist_symbol`'s `odd` term can
   never fire (the compare re-tests the condition the loop just
   broke on), so both DP tiers priced every first-half-bucket
   distance one extra bit high, exactly the regime dict words live
   in. Upstream-exact pricing (PrefixEncodeCopyDistance, verified
   symbol+extra over 25k distances) recovered dict recall only
   1,799→1,902 (plists q11 107,235→107,770) and cost the corpus
   q10+q11 net **+1.23%**, csv2m q11 alone **+41%** (119,038→
   168,169). The +1-bit bias steers the DP onto rep codes and near
   matches, and that accidental parse is *better* than both the
   reference's (csv2m q11 S=0.796) and our upstream-priced variant.
   Reverted byte-identical to v0.21.80; the divergence is documented
   in the function's comment.

**Disposition: ACCEPT the residual.** plists q11 stays S≈1.024 with
the remaining 418-dict-recall gap attributed to a second DP
divergence that the pricing bias masks. Known leads if reopened:
our `store_and_find_capped` drops sub-4-length tree matches
(upstream's flat list carries len 2-3 matches at any distance); the
pass-2 `from_commands` feedback loop; missing `backward >
max_distance` guard on rep candidates (windowed chunks only). The
diagnostics (DICT_LOG, DICT_WORD, HQ_MATCH_DUMP) ship env-gated for
the next session.

## Addendum (2026-09-12): f32-vs-f64 cost-model hypothesis — negative result

Hypothesis (candidate root for the "masked second DP divergence" and the
task-04 tree-shape S residuals): the reference's cost stack runs in f32 —
the Rust reference port defaults `floatX = f32`
(`~/src/external/brotli/src/enc/util.rs`, `float64` feature off) with
`FastLog2(v≥256) = (v as f32).log2()` — while our ports are f64
end-to-end, systematically flipping near-tie DP decisions.

Checked against the **actual reference binary** (homebrew brotli 1.2.0, C,
v1.2.0 tag, `c/enc/fast_log.{h,c}`): `FastLog2` is
`v < 256 ? kBrotliLog2Table[v] : log2((double)v)` and the table entries are
**exact log2 doubles** (e.g. 1.5849625007211563 = log2(3)); accumulation is
`double` throughout. Our model (exact f64 log2, index-ordered) matches C at
the value level. The only f32 semantics live in the Rust *port's* default
build, which is not our oracle. **Dead end — precision divergence is
eliminated as a root cause.** The near-tie divergence must come from
algorithmic ordering, not arithmetic.
