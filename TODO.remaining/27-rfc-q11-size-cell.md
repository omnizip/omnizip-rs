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

**Next session entry point:** instrument `write_huffman_table`'s
code-length header loop vs `read_complex_form`'s reading loop bit-by-bit
on the failing stream (`BROTLI_HQ_DICT=1 OUT=... rfc.txt brotli 11`),
find the break-condition asymmetry, fix, then re-run the corpus sweep +
un-gate. Expected: rfc q11 ~6,980–7,050 (cell closes to ~1.066-1.075x;
full ref parity 6,548 needs the DP-level literal steering beyond dict).

## Acceptance

Gated feature: PR with port + fix (no release — default output
unchanged). Un-gating happens in the follow-up that fixes the
Huffman-table break asymmetry, with the full corpus sweep + regression
baseline refresh.
