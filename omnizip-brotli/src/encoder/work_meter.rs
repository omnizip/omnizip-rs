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

/// Instrumented sites (order fixed; see `site_name`).
pub const SITES: usize = 5;
const NAMES: [&str; SITES] = [
    "hq_rep_compare",
    "hq_rep_sweep",
    "hq_h10_sweep",
    "bt_probe_compare",
    "bt_relax",
];

thread_local! {
    static WORK_UNITS: [Cell<u64>; SITES] = const { [const { Cell::new(0) }; SITES] };
}

/// Record `n` units of DP work at `site` (one call per loop
/// iteration, or the batched length after a loop).
#[inline]
pub(crate) fn add(site: usize, n: u64) {
    WORK_UNITS.with(|w| w[site].set(w[site].get().wrapping_add(n)));
}

/// Total units across sites since the last [`reset`] on this thread.
#[must_use]
pub fn units() -> u64 {
    WORK_UNITS.with(|w| w.iter().map(Cell::get).sum())
}

/// Per-site totals since the last [`reset`].
#[must_use]
pub fn breakdown() -> [(&'static str, u64); SITES] {
    WORK_UNITS.with(|w| {
        let mut out = [("", 0); SITES];
        for (i, slot) in w.iter().enumerate() {
            out[i] = (NAMES[i], slot.get());
        }
        out
    })
}

/// Reset the meter on this thread.
pub fn reset() {
    WORK_UNITS.with(|w| {
        for slot in w.iter() {
            slot.set(0);
        }
    });
}
