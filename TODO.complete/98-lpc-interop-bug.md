# 98 — LPC subframe interop bug: LOST_SYNC on high-order LPC

**Priority:** High (closes the remaining ~7 percentage-point ratio gap)
**Source:** TODO 97 Phase 3 (deferred from FLAC interop work)

## Symptom

When LPC subframes are enabled (`if true` in
`omnizip-flac/src/encoder/subframe.rs`), 2 of 6 libFLAC CLI parity
tests fail:

- `parity_sine_short` (192-sample sine, 8 kHz)
- `parity_sine_3_seconds` (131 072-sample sine, 44.1 kHz)

libFLAC reports `LOST_SYNC` or `OUT_OF_BOUNDS` on these inputs.

## What works

- CONSTANT, VERBATIM, FIXED orders 0-4 subframes: all 6 parity tests pass.
- LPC round-trips through OUR OWN decoder: all internal tests pass.
- LPC interop with libFLAC for SOME inputs (e.g. constant signal).

So the LPC subframe wire format is mostly correct but has a
subtle bug that triggers on specific coefficient patterns.

## Suspected root causes

1. **Coefficient quantization rounding.** Our `quantise_and_predict`
   uses `scaled.round() as i64`. Rust's `f64::round` rounds
   half-away-from-zero. libFLAC uses `(int)floor(x + 0.5)` for
   positive and `(int)ceil(x - 0.5)` for negative — also
   half-away-from-zero in principle, but float precision edge cases
   may differ.

2. **QLP shift semantics.** The shift field is 5-bit two's complement.
   Our encoder writes `shift_field = if shift < 0 { shift + 32 } else
   { shift }`. For shifts in 0..=15 this is correct. For negative
   shifts (which our search includes as 0..=12, so no negatives in
   practice), the encoding might differ from libFLAC.

3. **Residual reconstruction overflow.** libFLAC checks that decoded
   samples fit in `bps` bits. If our LPC coefficients produce
   out-of-range predictions during decode (not during encode), the
   residual + predicted can exceed i16 range. Our encoder has a
   residual-overflow check but not a reconstruction-overflow check.

## Debugging approach

1. Encode a sine via our encoder with LPC enabled. Dump the bytes.
2. Decode the same bytes via libFLAC's `--analyze` mode (needs
   decode to succeed; if not, use `--decode-through-errors`).
3. Identify which subframe causes the failure (sample number, byte
   offset).
4. Compare that subframe's bytes against what libFLAC would produce
   for the same LPC solution.
5. Find the bit-level divergence.

## Acceptance criteria

- [ ] All 6 parity tests pass with LPC enabled.
- [ ] Sine ratio on 131 072-sample benchmark drops below 25%
      (currently 28.62% with FIXED-only; libFLAC is 18.59%).
- [ ] `#![forbid(unsafe_code)]` preserved.

## Files

- `omnizip-flac/src/encoder/lpc.rs` — coefficient quantization
- `omnizip-flac/src/encoder/subframe.rs` — re-enable LPC (change
  `if false` to `if samples.len() >= 64`)
- `tests/differential/tests/flac_parity.rs` — verify parity
