# 27 — rfc.txt brotli q11 size cell (the last >1.05x cell)

- **Priority:** MEDIUM (only >1.05x cell on the standing board)
- **Status:** in_progress 2026-09-06

## The cell

`rfc.txt` (25,037 B, RFC text) brotli **q11: ours 7,205 vs ref 6,548 =
1.1003x**. Anomaly: our q11 is WORSE than our own q5 (7,107) and q9
(7,111) — the q11 contest ships a parse that loses ~100 B to our own
lower tiers on this file.

## New diagnostic capability

Since the PREFIX_SUFFIX fix (task 26, 0.21.60) the reference q11
stream decodes through our decoder — reference transform usage is now
diffable via `dec_ref` + DEC_STATS. Every earlier parse-shape
diagnosis of this cell predates that capability and is suspect.

## Candidate levers

1. **q11 contest third candidate** — the exact-acceptance contest only
   compares hq-vs-btopt DP parses; if both lose to the greedy-tier
   parse + q11 emission, add that as a third candidate.
2. **Transform-aware dict candidates (tl != wl)** — RFC text is the
   static dictionary's target domain; reference q11 may ride
   transformed words (" of the " affixes) that our tl==wl-scoped
   candidate paths never emit. Task 18's env-gated CODE_DICT work is
   the template.

## Findings (2026-09-06 session) — root cause FOUND, one bug layer left

**The cell is the static dictionary.** Post-PREFIX_SUFFIX-fix decode of
the reference stream showed 637 dictionary references (30% of its
commands) vs our 473 — the old DEC_STATS bucket hardcoded `d > 65536`
and read 48 (fixed this session: `decoder_full.rs` now buckets by the
decoder's own max-distance rule).

**Ported**: `BrotliFindAllStaticDictionaryMatches` verbatim into
`encoder/static_dict.rs` + mechanically-generated bucket/word tables
(`static_dict_lut.rs`, 32,768 buckets + 31,705 packed words from
google/brotli, BSD-3). Full transform family per position: raw,
omit-1..9, "ing ", affix continuations, uppercase + caps variants,
leading " "/"."/"e "/"s "/" NBSP/" the "/".com/" sub-blocks. Verified
against upstream: every C multiplier present; "Comments" (id 9238) and
" Working " (id 30760) resolve exactly as the reference's commands.

**Wired** into zopfli_hq's DP: per-position candidate list relaxed like
LZ matches (len=produced tl, len_code=word wl, CODE_DICT_SHORT ring
safety). Measured on rfc.txt q11: **hq 64,197 → 55,835 bits; hq now
BEATS btopt in the contest (57,634); shipped would be ~6,980 vs 7,205.**

**THE REMAINING BUG (feature gated OFF via `BROTLI_HQ_DICT`, default
off — default output byte-identical to 0.21.60):** with AFFIX
(lengthening, tl>wl) candidates active the stream is bit-corrupt.
Bisected: identity-only VALID, shrink-only VALID, grow-only CORRUPT.
The corruption: decoder reads the same tree LENGTHS as the writer
(verified by tree dumps) and the same command symbols, but the BIT
POSITIONS diverge inside the FIRST Huffman table's wire form
(writer/reader bit-trace aligned at writer-run event ~18, right where
the complex-form header ends) — the "space break" mirroring in
`write_huffman_table` (emission.rs, the `num_codes != 1 &&
space.wrapping_sub(1) >= 32` early-break) vs the decoder's read loop
break is the prime suspect. The command misread: sym 356's extras
decode (37,27) where (38,26) was written — self-correcting bit totals
kept later commands aligned, which is why command-level diffs pointed
everywhere at once.

**SOLVED (2026-09-06, second session): the Huffman-table theory was WRONG — the real bug was in the port itself.** The caps sub-walk
(`walk_bucket(..., sub_t=85, caps=true)`) leaked TRANSFORM-0 bucket
words into the 18/7/13 sub-block arm, which multiplies by `sub_t` —
fabricating bogus " " + ALL-CAPS(w) + " " candidates for inputs that
merely matched the raw word plus a ' ' two bytes later (repro: input
" clearly\n   " at pos 989 produced a " CLEARLY " candidate). The DP
took these phantom candidates (they price well — caps transforms are
short ids), the walk emitted them, and the produced bytes diverged from
the input — desyncing every downstream command (the "Huffman table
divergence", the ring misreads, the self-correcting bit patterns: all
downstream symptoms). Fix: `if caps && transform == 0 { continue; }` —
the caps walk handles only transformed entries (upstream's is_space
else-branch); raw words belong to the 6/32 walk.

**Honest numbers after the fix** (all streams byte-exact, ref streams
unaffected): hq-with-dict 64,197 -> **59,863 bits** on rfc q11 — but
btopt still wins the contest (57,634), so shipped output stays 7,205
and the feature stays gated (`BROTLI_HQ_DICT`). The corrupt era's
55,835 was the phantom candidates measuring small.

**LEVER (c) SHIPPED (third session): the iterative-zopfli third contest
candidate.** The q11 contest now also measures the in-house iterative
zopfli (the parse our sub-1MiB q5 tier ships) emitted with its OWN
q5-tier emission, for inputs <= 256 KiB. Key measurement: re-emitting
the iterative commands under the q11 emission measures WORSE (59,042
bits on rfc) than its own q5 emission (56,852) — the q11 literal
assignment overshoots on small inputs; the candidate must ship its own
writer. Contest-shielded: ships only when strictly smaller than both
DP candidates.

**RESULTS (all byte-identical + C-decodable):**
- rfc.txt q11: 7,205 -> **7,107** (cell 1.1003x -> **1.0854x**)
- photo.jpg q11: 13,591 -> 13,450 (1.0587x -> 1.0478x)
- every other corpus cell byte-identical (synthetic + real)
- regression gate green (no baseline refresh needed)

**FOURTH candidate shipped (fourth session): hq+dict as a SHIELDED
contest candidate — rfc q11 7,107 -> 7,007 (cell 1.0701x).** Two
discoveries: (1) the dict relaxation was inside the MATCHES loop, so
at positions with NO LZ candidates (exactly the dictionary-word
positions) it never ran at all — relocated to the k-loop body,
hq+dict went 59,863 -> 56,050 bits on rfc; (2) dict density is NOT
defaultable globally (csv2m +2,008B, dbdump +76B, words +314B when
replacing the plain hq candidate) but IS a pure improvement as an
ADDITIONAL candidate: the contest min() over {hq, hq+dict, bt, iter}
(q11, n <= 256 KiB) can never regress. Corpus verified: rfc the only
change, every other cell byte-identical, regression gate green.

**N-BOUND LIFTED via a density screen (fifth session):** the fourth
candidate now runs at ANY size when a 512-position dictionary-density
sample clears 0.08 — measured classes separate cleanly (text
0.10-0.20: rfc/rustsrc/words/dbdump/plists/install.log; periodic and
binary 0.00-0.03: csv2m/fits/arial/rand). Small inputs (<= 256 KiB)
stay unconditional. **rustsrc q11 379,830 -> 377,904 (-1,926 B)**;
every other cell byte-identical; the time-sensitive binary cells
(fits q11 at 0.93x reference time) skip the extra DP pass entirely.

**Still open (7,007 vs ref 6,548, ~575 B):** the reference's emission
beats all four candidates on rfc — its literal-steering (positional
cost model x dict density) remains the named lever.

## Acceptance

Gated feature: PR with port + fix (no release — default output
unchanged). Un-gating happens in the follow-up that fixes the
Huffman-table break asymmetry, with the full corpus sweep + regression
baseline refresh.
