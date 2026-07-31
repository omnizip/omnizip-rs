//! Delta filter — byte-wise differencing.
//!
//! Each output byte is `input[i] - input[i - distance]` (wrapping on
//! subtraction). Decode is the inverse: `output[i] = encoded[i] +
//! state[i - distance]`.
//!
//! Ported from `omnizip/lib/omnizip/filters/delta.rb` (MIT, Ribose Inc.).
//! The default distance is 1, suitable for raw PCM audio and byte
//! streams. For wider sample widths (16-bit, 32-bit), use distance 2 or
//! 4 respectively.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use crate::Filter;

/// Maximum configurable distance. Matches the XZ delta filter.
const MAX_DISTANCE: usize = 256;

/// Delta filter with configurable byte distance.
pub struct DeltaFilter {
    distance: usize,
}

impl DeltaFilter {
    /// Construct a delta filter with the given byte distance (1–256).
    ///
    /// # Panics
    ///
    /// Panics if `distance` is 0 or greater than 256.
    #[must_use]
    pub fn new(distance: usize) -> Self {
        assert!(
            (1..=MAX_DISTANCE).contains(&distance),
            "delta distance must be 1..={MAX_DISTANCE}, got {distance}",
        );
        Self { distance }
    }

    /// The standard delta-1 filter (suitable for raw byte streams).
    #[must_use]
    pub fn new_default() -> Self {
        Self::new(1)
    }

    /// The configured byte distance.
    #[must_use]
    pub fn distance(&self) -> usize {
        self.distance
    }
}

impl Default for DeltaFilter {
    fn default() -> Self {
        Self::new(1)
    }
}

impl Filter for DeltaFilter {
    fn name(&self) -> &'static str {
        "delta"
    }

    fn encode(&self, input: &[u8]) -> Vec<u8> {
        let dist = self.distance;
        let mut output = Vec::with_capacity(input.len());
        for (i, &byte) in input.iter().enumerate() {
            if i >= dist {
                output.push(byte.wrapping_sub(input[i - dist]));
            } else {
                output.push(byte);
            }
        }
        output
    }

    fn decode(&self, input: &[u8]) -> Vec<u8> {
        let dist = self.distance;
        let mut output = Vec::with_capacity(input.len());
        for (i, &byte) in input.iter().enumerate() {
            if i >= dist {
                output.push(byte.wrapping_add(output[i - dist]));
            } else {
                output.push(byte);
            }
        }
        output
    }
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod tests {
    use super::*;

    #[test]
    fn distance_1_inverts_cleanly() {
        let filter = DeltaFilter::new(1);
        let data: Vec<u8> = (0..200u32).map(|i| (i * 7) as u8).collect();
        let encoded = filter.encode(&data);
        assert_eq!(filter.decode(&encoded), data);
    }

    #[test]
    fn smooth_signal_compresses_better_after_delta() {
        // A smooth ramp should produce small deltas that compress better
        // when chained with a downstream codec.
        let filter = DeltaFilter::new(1);
        let data: Vec<u8> = (0..200u32).map(|i| (i / 3) as u8).collect();
        let encoded = filter.encode(&data);
        // Smooth input → many repeated delta values; the encoded stream
        // has lower entropy than the ramp input.
        let unique_input: std::collections::HashSet<u8> = data.iter().copied().collect();
        let unique_encoded: std::collections::HashSet<u8> = encoded.iter().copied().collect();
        assert!(
            unique_encoded.len() <= unique_input.len(),
            "delta encoding should not increase symbol variety"
        );
    }

    #[test]
    #[should_panic(expected = "delta distance must be 1..=256")]
    fn rejects_zero_distance() {
        let _ = DeltaFilter::new(0);
    }

    #[test]
    #[should_panic(expected = "delta distance must be 1..=256")]
    fn rejects_oversize_distance() {
        let _ = DeltaFilter::new(257);
    }

    #[test]
    fn handles_empty_input() {
        let filter = DeltaFilter::default();
        assert_eq!(filter.encode(b""), b"");
        assert_eq!(filter.decode(b""), b"");
    }

    #[test]
    fn preserves_short_input_below_distance() {
        let filter = DeltaFilter::new(8);
        let data = b"short";
        // When input is shorter than the distance, encode is identity.
        assert_eq!(filter.encode(data), data);
        assert_eq!(filter.decode(data), data);
    }
}
