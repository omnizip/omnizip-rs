//! DP work meter (issues #388/#408 class).
//!
//! The pathological-input incidents were both WORK regressions: a
//! raised match-length cap multiplied per-position candidate/sweep
//! work on repetitive content, showing up as a CI hang only on slow
//! machines. Timing-based tests can't catch that deterministically;
//! counting the loop iterations that scale with the knobs can. The
//! tests assert a fixed budget (calibrated with headroom) for the
//! pathological content classes, so any change that inflates DP work
//! — cap bumps, sweep rewrites, candidate-count growth — fails a
//! unit assertion instantly, on any machine.
//!
//! Counted loops (per iteration, or batched by length after compare
//! loops): the rep-code compare and l2 sweeps and the H10 relaxation
//! sweep in [`crate::encoder::zopfli_hq`], and the rep-probe compares
//! and copy relaxations in [`crate::encoder::btopt`].
//!
//! Thread-local on purpose: encoders are single-threaded per call,
//! isolation keeps parallel tests from polluting each other's
//! readings, and a `Cell` add is free next to the loop bodies.

#![forbid(unsafe_code)]

use std::cell::Cell;

thread_local! {
    static WORK_UNITS: Cell<u64> = const { Cell::new(0) };
}

/// Record `n` units of DP work (one call per loop iteration, or the
/// batched length after a loop).
#[inline]
pub(crate) fn add(n: u64) {
    WORK_UNITS.with(|w| w.set(w.get().wrapping_add(n)));
}

/// Total units since the last [`reset`] on this thread.
#[must_use]
pub fn units() -> u64 {
    WORK_UNITS.with(Cell::get)
}

/// Reset the meter on this thread.
pub fn reset() {
    WORK_UNITS.with(|w| w.set(0));
}
