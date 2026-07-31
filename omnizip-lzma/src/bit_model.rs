//! Adaptive probability model for range coding.
//!
//! Ported from `omnizip/lib/omnizip/algorithms/lzma/bit_model.rb` (MIT,
//! Ribose Inc.), which is itself a port of XZ Utils
//! `range_encoder.c`.
//!
//! Each bit position in the LZMA bitstream has an associated
//! [`BitModel`] that tracks the probability of encoding a 0. The range
//! coder uses this probability to narrow the interval; after each bit,
//! the model adapts toward the observed outcome.
//!
//! ## Adaptation formula
//!
//! Matches XZ Utils `RC_BIT_*` macros:
//!
//! - Observed 0: `prob += (TOTAL - prob) >> MOVE_BITS`
//! - Observed 1: `prob -= prob >> MOVE_BITS`
//!
//! With `TOTAL = 2048` and `MOVE_BITS = 5`, this gives a gentle
//! adaptation: each update moves ~6% of the way toward the observed
//! outcome.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use crate::constants::{BIT_MODEL_TOTAL, INIT_PROBS, MOVE_BITS};

/// An adaptive bit probability model.
///
/// The probability is stored as a `u16` in `[1, TOTAL - 1]`. It
/// represents the probability of encoding a 0 bit. A value of
/// `TOTAL / 2` (1024) means 50% probability.
#[derive(Clone, Debug)]
pub struct BitModel {
    probability: u16,
}

impl BitModel {
    /// Create a new model at the initial probability (50% — balanced).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            probability: INIT_PROBS,
        }
    }

    /// Create a model at a specific probability. Intended for testing
    /// and for restoring saved state.
    #[must_use]
    pub const fn at(probability: u16) -> Self {
        Self { probability }
    }

    /// The current probability of a 0 bit, in `[1, TOTAL - 1]`.
    #[must_use]
    pub fn probability(&self) -> u16 {
        self.probability
    }

    /// The probability of a 1 bit: `TOTAL - probability`.
    #[must_use]
    pub fn prob_1(&self) -> u16 {
        BIT_MODEL_TOTAL - self.probability
    }

    /// Adapt toward the observed bit value.
    ///
    /// - If `bit == 0`: probability increases (shifts toward 0).
    /// - If `bit == 1`: probability decreases (shifts toward 1).
    pub fn update(&mut self, bit: u32) {
        if bit == 0 {
            // prob += (TOTAL - prob) >> MOVE_BITS
            let total = BIT_MODEL_TOTAL;
            self.probability = self.probability + ((total - self.probability) >> MOVE_BITS);
        } else {
            // prob -= prob >> MOVE_BITS
            self.probability = self.probability - (self.probability >> MOVE_BITS);
        }
    }

    /// Reset to the initial probability (50%).
    pub fn reset(&mut self) {
        self.probability = INIT_PROBS;
    }
}

impl Default for BitModel {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for BitModel {
    fn eq(&self, other: &Self) -> bool {
        self.probability == other.probability
    }
}

impl Eq for BitModel {}

/// A contiguous array of [`BitModel`] values. This is the primary
/// allocation unit for probability tables (literal contexts, length
/// coders, distance coders, etc.).
#[derive(Clone, Debug)]
pub struct BitModelArray {
    models: Vec<BitModel>,
}

impl BitModelArray {
    /// Create an array of `len` models, each at the initial probability.
    #[must_use]
    pub fn new(len: usize) -> Self {
        Self {
            models: (0..len).map(|_| BitModel::new()).collect(),
        }
    }

    /// Access a model by index (mutable, for adaptation during decode).
    ///
    /// # Panics
    ///
    /// Panics if `index >= len`.
    pub fn get(&mut self, index: usize) -> &mut BitModel {
        &mut self.models[index]
    }

    /// Reset all models to the initial probability.
    pub fn reset(&mut self) {
        for m in &mut self.models {
            m.reset();
        }
    }

    /// The number of models in the array.
    #[must_use]
    pub fn len(&self) -> usize {
        self.models.len()
    }

    /// Whether the array is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_model_starts_at_half() {
        let m = BitModel::new();
        assert_eq!(m.probability(), INIT_PROBS);
        assert_eq!(m.prob_1(), INIT_PROBS);
        assert_eq!(m.probability() + m.prob_1(), BIT_MODEL_TOTAL);
    }

    #[test]
    fn update_toward_zero_increases_prob() {
        let mut m = BitModel::new();
        let p_before = m.probability();
        m.update(0);
        assert!(
            m.probability() > p_before,
            "updating with bit=0 should increase probability of 0"
        );
    }

    #[test]
    fn update_toward_one_decreases_prob() {
        let mut m = BitModel::new();
        let p_before = m.probability();
        m.update(1);
        assert!(
            m.probability() < p_before,
            "updating with bit=1 should decrease probability of 0"
        );
    }

    #[test]
    fn prob_sum_is_constant_after_update() {
        let mut m = BitModel::new();
        m.update(0);
        m.update(0);
        m.update(1);
        m.update(1);
        assert_eq!(
            m.probability() + m.prob_1(),
            BIT_MODEL_TOTAL,
            "prob + prob_1 must always equal TOTAL"
        );
    }

    #[test]
    fn reset_returns_to_half() {
        let mut m = BitModel::new();
        m.update(1);
        m.update(1);
        m.update(1);
        m.reset();
        assert_eq!(m.probability(), INIT_PROBS);
    }

    #[test]
    fn array_allocates_and_indexes() {
        let mut arr = BitModelArray::new(100);
        assert_eq!(arr.len(), 100);
        assert!(!arr.is_empty());
        arr.get(0).update(0);
        arr.get(50).update(1);
        assert_ne!(arr.get(0).probability(), arr.get(50).probability());
    }

    #[test]
    fn array_reset_restores_all_models() {
        let mut arr = BitModelArray::new(10);
        for i in 0..10 {
            arr.get(i).update(1);
        }
        arr.reset();
        for i in 0..10 {
            assert_eq!(arr.get(i).probability(), INIT_PROBS);
        }
    }

    #[test]
    fn repeated_updates_converge() {
        // After many updates toward 0, prob approaches TOTAL but never
        // reaches it: when the gap < 32, (gap >> 5) = 0 and adaptation
        // stops. This matches the C reference behaviour.
        let mut m = BitModel::new();
        for _ in 0..1000 {
            m.update(0);
        }
        let total = BIT_MODEL_TOTAL;
        assert!(
            m.probability() > total - 35,
            "after 1000 zeros, prob should be near TOTAL (within 35), got {}",
            m.probability()
        );
    }
}
