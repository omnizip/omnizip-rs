# Task 31 — measurement-integrity audit: the harness enum-mapping fallthrough

Status: closed (2026-09-14, verified — no board correction needed)

## The scare

The scratch harnesses (`zw_tmp`/`zprof_tmp`) map numeric levels through
`match { 1=>Fastest, 3=>Fast, 6=>Default, 12=>Better, _ => Best }` —
any unmatched level falls through to `Best` (=22). Two observations
forced the audit: "fits L2 output == fits L19 output exactly" (both
mapped to 22), and the realization that every board "zstd 19" cell was
measured through the 22 entry point against `zstd -19` on the
reference side.

## The verification

- The public trait path is identity-mapped (codec.rs clamps numeric
  1–22 and runs `encode_frame_compressed(level)` directly) — a raw-19
  harness (`zlvl_tmp`, explicit `Codec::compress`) exists now for
  future sweeps.
- **ours@19 == ours@22 byte-identical** (fits 1,957,692): levels 19–22
  differ only in window/chain logs that cannot bind at these file
  sizes (window ≥ file ⇒ same candidate space).
- **ref@19 == ref@22 too** (fits 1,966,084 both): the reference
  converges identically at this scale. The board column labeled 19 was
  ours@19-equivalent vs ref@19 all along — honest as measured.
- The "L2" reading was purely a harness artifact (mapped to 22); no
  board exposure. For the record, true L2 on fits is a healthy cell:
  ours 3,558,740 vs ref 3,574,780 (−0.45%), round-trip verified.

## Standing

No board cell moves; v30 remains current. The corrected raw-level
harness (`omnizip-zstd/examples/zlvl_tmp.rs`) is the canon tool for
any future per-level sweep — the enum-mapped harnesses must not be
used for levels outside {1,3,6,12,22}.
