# BUGREPORT: zstd decoder still fails own-encoder frames post-#316 (minimal 163-byte repro)

## RESOLVED (2026-09-20, verified 0.21.103)

The residual does not reproduce on the current codebase: the pinned 163-byte
blob round-trips at ALL five levels, and a 180-case differential sweep (15
corpora x {6 reference-encode levels incl. --long, 5 our-encode levels},
both directions) passes clean — our decoder matches system zstd on every
reference frame and system zstd accepts every frame we emit byte-exactly.
Cleared by the intervening decoder work (OF_BASE/OF_BITS, literals, frame
machinery); never re-verified until now. Gates landed:
`omnizip-zstd/tests/z315_regression.rs` (pinned blob, shape class,
system-CLI differential, codec-tier leg). The report body below is the
historical record.

## omnizip version
0.16.78 (post PR #316 OF_BASE/OF_BITS fix). Also live in 0.16.76/0.16.77.

## Affected crate
`omnizip-zstd` (decoder)

## Summary
After the offset-table fix, the decoder STILL mis-decodes frames its
own encoder produces for specific content shapes. Levels affected:
Fastest / Fast / Default / Better. Best is correct. The frames are
VALID — the system zstd CLI decodes every failing frame byte-exactly.

## Minimal repro (163 bytes, delta-debugged from issue #315's 318-byte blob)
base64:
```
LwjOGAEAAAAEpQAAAGR1cGxpY2F0ZSBpbmxpbmUgY29udGVuaGUgc2FtZSAyMDAtaXNoIGJ5dGVzIGluIHRocmVlIGZpbGVzLCBzbyB0aGUgd3JpdGVyJ2VzIG9uIGV2ZXJ5IHJlYWxpc3RpYyB0cmVlLiBQYWQAAAAF6UCBLwjOAQAAAADSf+9PzA2Fv8RqcmiN5Gtx/fn2pu5LCCNiKcneAQ==
```
- Fastest/Fast/Default/Better: ALL FOUR emit the IDENTICAL 172-byte
  frame (tiny input collapses the level parameterizations) and fail
  identically: `stored 0xD0E6C092, computed 0x1DF30DF2`
- Best: emits a different 175-byte frame, round-trips OK
- `zstd -d` (system CLI) decodes the Default frame (172 B) back to the
  exact 163 input bytes — frame valid, decoder wrong.

## Diagnostics
- Shortest failing prefix of the original blob: 288 bytes; greedy
  middle-cut shrink → 163 bytes (input shape: binary header + long
  literal text + binary tail — the regime where matches/reps fire
  mid-literal, not covered by Raw/RLE goldens).
- The residual suspect, given #316 fixed the tables: state
  reconstruction for interleaved literal + offset FSE streams at
  small block sizes / few sequences (baseline or extra-bit
  accounting, or stream-end handling).
- Self-round-trip fuzz at Fastest..Better over mixed text+binary
  inputs would catch this class; size-sweep corpora do not
  (downstream LimniFS passes 0–65536 random inputs at all levels).

## Key clue
On this input the four affected levels produce byte-identical frames,
so the defect is in the ONE shared encoder parameterization used at
tiny inputs — and its frame is valid (system zstd decodes it) but the
decoder reconstructs different bytes. Bisect the decoder's sequence
reconstruction (offset/literal FSE interleaving, few-sequences /
small-block edge cases) against the reference on this exact frame.

## Suggested fix
Add self-round-trip tests at the four affected levels over
mixed-content corpora (this 163-byte input included), then bisect
decoder state vs. reference for the first divergent sequence.

## Impact on LimniFS
Guarded since v0.2.53: every zstd encode is decompress-verified; a
frame that does not round-trip is refused (tournament falls through).
Pure overhead once fixed. Pinned as
`codec::zstd::tests::omnizip_315_blob_is_refused_not_emitted`.
