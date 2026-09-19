//! Progress / ETA reporting — port of the Ruby gem's `progress/` +
//! `eta/` subsystems (`TODO.ref-parity/59`).
//!
//! [`ProgressReporter`] is the single seam long-running operations
//! (streaming in task 57, the parallel engine in task 58, archive
//! create/extract) call into. Reporting is best-effort by design:
//! reporters never fail the operation, and [`Silent`] (the default)
//! compiles to nothing on the hot path.
//!
//! ## Determinism
//!
//! Reporting sits OUTSIDE the data path — outputs are byte-identical
//! with any reporter, including none. [`ProgressTracker`]'s clock
//! (`Instant`) feeds reporting only, never output decisions.

#![forbid(unsafe_code)]
// f64 rate math: u64 byte counts lose mantissa bits past 2^53 —
// irrelevant for a rate ESTIMATE (reporting only).
#![allow(clippy::cast_precision_loss)]

use std::time::Instant;

/// The operation being reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Compress,
    Decompress,
    Verify,
    Extract,
    Create,
    Convert,
}

impl Operation {
    /// Lowercase name for display.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Compress => "compress",
            Self::Decompress => "decompress",
            Self::Verify => "verify",
            Self::Extract => "extract",
            Self::Create => "create",
            Self::Convert => "convert",
        }
    }
}

/// Progress sink. `done`/`total` are bytes for stream operations,
/// entries for archive operations.
///
/// Best-effort: implementations must not fail the operation; errors
/// are the reporter's problem.
pub trait ProgressReporter: Send + Sync {
    /// Operation started (`total` may be 0 when unknown).
    fn on_start(&self, _op: Operation, _subject: &str, _total: u64) {}
    /// Work progressed. Callers guarantee `done <= total` when total
    /// is known; implementations must tolerate anything.
    fn on_progress(&self, _op: Operation, _subject: &str, _done: u64, _total: u64) {}
    /// Operation completed successfully.
    fn on_finish(&self, _op: Operation, _subject: &str) {}
    /// Operation failed with `error`.
    fn on_error(&self, _op: Operation, _subject: &str, _error: &str) {}
}

/// The no-op default — zero cost on the data path.
#[derive(Debug, Clone, Copy, Default)]
pub struct Silent;

impl ProgressReporter for Silent {}

/// Closure-based reporter — what CLIs and the Ruby bridge build.
pub struct CallbackReporter<F>
where
    F: Fn(ProgressEvent<'_>) + Send + Sync,
{
    f: F,
}

impl<F> CallbackReporter<F>
where
    F: Fn(ProgressEvent<'_>) + Send + Sync,
{
    #[must_use]
    pub fn new(f: F) -> Self {
        Self { f }
    }
}

/// One progress event, passed to [`CallbackReporter`].
#[derive(Debug, Clone, Copy)]
pub struct ProgressEvent<'a> {
    pub op: Operation,
    pub subject: &'a str,
    pub done: u64,
    pub total: u64,
}

impl<F> ProgressReporter for CallbackReporter<F>
where
    F: Fn(ProgressEvent<'_>) + Send + Sync,
{
    fn on_start(&self, op: Operation, subject: &str, total: u64) {
        (self.f)(ProgressEvent {
            op,
            subject,
            done: 0,
            total,
        });
    }
    fn on_progress(&self, op: Operation, subject: &str, done: u64, total: u64) {
        (self.f)(ProgressEvent {
            op,
            subject,
            done,
            total,
        });
    }
    fn on_finish(&self, op: Operation, subject: &str) {
        (self.f)(ProgressEvent {
            op,
            subject,
            done: u64::MAX,
            total: u64::MAX,
        });
    }
    fn on_error(&self, op: Operation, subject: &str, error: &str) {
        (self.f)(ProgressEvent {
            op,
            subject,
            done: u64::MAX,
            total: error.len() as u64,
        });
    }
}

/// Rate tracker with an exponential moving average — the `eta/`
/// port. `Instant` feeds reporting only (never output decisions).
#[derive(Debug)]
pub struct ProgressTracker {
    started: Instant,
    last: Instant,
    total: u64,
    done: u64,
    /// EMA of bytes-per-second (alpha 0.2, seeded with the first
    /// instantaneous rate).
    rate_ema: Option<f64>,
}

const EMA_ALPHA: f64 = 0.2;

impl ProgressTracker {
    #[must_use]
    pub fn new(total: u64) -> Self {
        let now = Instant::now();
        Self {
            started: now,
            last: now,
            total,
            done: 0,
            rate_ema: None,
        }
    }

    /// Record `done` units completed so far; updates the rate EMA.
    pub fn update(&mut self, done: u64) {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f64();
        if dt > 0.0 && done > self.done {
            let inst = (done - self.done) as f64 / dt;
            self.rate_ema = Some(match self.rate_ema {
                Some(prev) => EMA_ALPHA * inst + (1.0 - EMA_ALPHA) * prev,
                None => inst,
            });
        }
        self.last = now;
        self.done = done;
    }

    /// `(done, total, rate_units_per_sec, eta_seconds)` — eta is
    /// `None` until the rate estimate exists.
    #[must_use]
    pub fn snapshot(&self) -> (u64, u64, Option<f64>, Option<f64>) {
        match self.rate_ema {
            Some(rate) if rate > 0.0 => {
                let remaining = self.total.saturating_sub(self.done);
                (
                    self.done,
                    self.total,
                    Some(rate),
                    Some(remaining as f64 / rate),
                )
            }
            _ => (self.done, self.total, None, None),
        }
    }

    /// Seconds since tracking started.
    #[must_use]
    pub fn elapsed(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn silent_is_the_zero_cost_default() {
        Silent.on_start(Operation::Compress, "x", 10);
        Silent.on_progress(Operation::Compress, "x", 5, 10);
        Silent.on_finish(Operation::Compress, "x");
        Silent.on_error(Operation::Compress, "x", "boom");
    }

    #[test]
    fn callback_reporter_sees_the_sequence() {
        let events: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());
        {
            let reporter = CallbackReporter::new(|e| {
                events.lock().unwrap().push((e.done, e.total));
            });
            reporter.on_start(Operation::Extract, "a.zip", 100);
            reporter.on_progress(Operation::Extract, "a.zip", 50, 100);
            reporter.on_finish(Operation::Extract, "a.zip");
        }
        let seq = events.into_inner().unwrap();
        assert_eq!(seq, vec![(0, 100), (50, 100), (u64::MAX, u64::MAX)]);
    }

    #[test]
    fn tracker_rates_and_eta() {
        let mut t = ProgressTracker::new(1000);
        t.update(100);
        let (done, total, rate, eta) = t.snapshot();
        assert_eq!((done, total), (100, 1000));
        // First update has an instantaneous rate — eta derives from it.
        assert!(rate.is_some_and(|r| r > 0.0));
        assert!(eta.is_some_and(f64::is_finite));

        // No new work: snapshot stable, no divide-by-zero.
        std::thread::sleep(std::time::Duration::from_millis(2));
        t.update(100);
        let (done, _, _, _) = t.snapshot();
        assert_eq!(done, 100);

        // Overflow-tolerant: done > total saturates.
        t.update(u64::MAX);
        let (done, total, _, eta) = t.snapshot();
        assert_eq!((done, total, eta), (u64::MAX, 1000, Some(0.0)));
    }
}
