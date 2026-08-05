# TODO 166: FLAC remainder — finish the 10× gap

## Problem

TODO 112 closed half the FLAC 10× gap (FFT autocorrelation).
TODO 111 closed another big chunk (block-size pruning).

Remaining gap vs libFLAC / DwarFS:
- LPC order selection tuning (current shortlist may miss optimum).
- Block-size sweep at high quality levels.
- SIMD residual computation wider (currently i32x8, could go i32x16
  with AVX-512 detection).

## Scope

1. **LPC order selection**: current `ORDER_SHORTLIST = [16, 12, 8,
   6, 4, 2]` is fixed. Make it adaptive: shortlist orders whose
   prediction-error energy drops >X% from the previous.
2. **Block-size sweep at quality**: add a `FlacEncoderOptions::try_
   all_block_sizes` flag (default false) that restores the libFLAC
   `--best` sweep.
3. **Wider SIMD**: detect AVX-512 at runtime, use `i32x16` for the
   residual loop.

## Acceptance criteria

- [ ] Adaptive LPC order selection lands.
- [ ] Block-size sweep knob exposed.
- [ ] Bench shows ≥ 2× additional speedup at quality 8.
- [ ] Output stays byte-identical to current default.

## Priority

P2 — second-half of a gap that's already half-closed.
