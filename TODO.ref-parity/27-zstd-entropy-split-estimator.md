# Task 27 — zstd: entropy-only split estimation (measured out)

Status: measured-out (2026-09-13, not shipped)
Parent: task 21 dependency #2 (incremental split estimates)

## The experiment

Replaced `derive_splits`' per-candidate trial emissions (3 full
`encode_content_parts` per recursion level — 77% of split-encode time)
with pure entropy arithmetic: literal Shannon entropy + LL/ML/OF
symbol entropy + extra bits, plus header amortization constants
(env-tunable). Final emission and the keep-only-when-smaller shield
unchanged. L1–L12 byte-identical (splitter inactive there).

## The result — entropy is blind to the real gains

- csv2m L19: **152,843 → 159,803 (+4.6%)** — the estimator returns NO
  splits at any header constant (verified invariant from 968 to
  99,999,999 bits): the exact splitter's 7 KB win on csv2m comes from
  repcode structure and FSE-table specialization that symbol entropy
  cannot see. The periodic structure has uniform per-partition
  entropy while its encodings differ sharply.
- words +29B, plists +901B, noto +93B, icons +12B, dbdump +36B,
  sqlite +26B; fits/rustsrc/rfc/install identical (no splits either
  way at L19 on those).
- The T side would have delivered (~15% off the L19 column,
  derive_splits 21% of csv2m L19 frame encode), and board I would
  have moved (csv2m 2.25→~2.03) — but it trades S for T.

## Verdict

Rejected on the value order: LimniFS stores S forever; T is paid once.
A +4.6% size regression to save 15% of a 2.4x cell inverts that. The
exact trial emissions are the deliberate price of the splitter's
csv2m-class wins. Task 21's chain therefore has NO cheap estimator
path: reopening it needs either an estimator that models repcode +
table specialization (a research item, not a port), or a ~3x cheaper
exact emission. Reverted byte-identical to v0.21.88 (csv2m L19
152,843 verified).

## Note on the reference

zstd's own ZSTD_deriveBlockSplits uses entropy estimates too — on the
reference's parse and table stack the gains are entropy-visible; on
ours they are not (its seq distributions differ). Do not assume the
ref's estimator transfers: measure.
