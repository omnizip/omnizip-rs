# 01 — brotli q9 text-DP tier: 29–104× slower for ≤10% size

- **Priority:** P0 (worst inequality on the board)
- **Score evidence (2026-09-07):** dbdump q9 **I=94** (T=104×, S=0.908);
  rustsrc q9 I=55 (T=55.5×, S=0.983); words q9 I=26 (T=29×, S=0.894).
- **Status:** done 2026-09-07 (I 20-85 -> ~0.8-2)

## Root cause (confirmed by the full 75-cell table)

TWO gates routed sub-1MiB and text inputs at q4-9 into the
iterative-zopfli DP: the `input.len() >= 1 MiB` greedy bar (every
small file: rfc/dbdump/plists/sqlite/noto q5+q9 at I=25-85) and the
`!is_text_like` exception (words/rustsrc q9 at I=26-55). The DP buys
6-12% size at 40-90x time — the exact inequality this board exists to
whack.

## Fix (shipped): greedy for ALL q2-9 inputs

Routing changed to `quality >= 2 && quality < 10` with the size bar
dropped to 4 KiB. Measured trade (q9 representative cells):

- rfc q9: T 90x -> <1x, S 0.940 -> 1.071, **I 84.6 -> ~1**
- dbdump q9: T 80x -> ~1.7x, S 0.908 -> 1.101, **I 72.6 -> ~1.9**
- plists q9: T 52x -> ~1x, S 1.035 -> 1.258, **I 54.2 -> ~1.3**
- words q9: T 29x -> ~1x, S 0.894 -> 0.991, **I 26.0 -> ~1**
- rustsrc q9: T 55x -> ~1.5x, S 0.983 -> 1.041, **I 54.6 -> ~1.6**
- csv2m q5/q9 (already greedy >=1MiB): csv2m q9 T 26.5x -> ~1x,
  S 0.761 -> 0.797, **I 20.2 -> 0.8**

The q4-9 tier now ships 6-26% larger on sub-1MiB text (plists worst)
— a deliberate one-time output change under the board's I principle.
Regression baseline refreshed (36 fixture rows). The old DP output
remains reachable at q10+; BROTLI_NO_GREEDY_TIER restores the DP for
q4-9.

## Follow-up (the size give-back)

The greedy-seed + one-refine hybrid (q9-shape at greedy cost,
documented in the 0.16.71 notes) is the lever to recover the 6-26%
without the DP's 40-90x — open as a separate task when the board
re-scores.

## Acceptance

- q9 cells: I ≤ 3 on the 9-file corpus; brotli test suite green;
  regression gate (q9 fixtures refresh if the tier's output changes —
  it will, once, in the release that ships this).
