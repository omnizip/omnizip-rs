//! Preprocessing filters for compression pipelines.
//!
//! Filters are reversible transforms applied to the input **before** a
//! codec (LZMA, ZSTD, etc.) and **after** decode. They convert data into
//! a more compressible form:
//!
//! - **Delta filter** — replaces each byte with its difference from the
//!   byte N positions earlier. Excellent for PCM audio, floating-point
//!   arrays, and other regularly-spaced data.
//! - **BCJ filters** (Branch / Call / Jump) — convert relative branch
//!   instructions in executable code to a form that compresses much
//!   better. One filter per instruction set (x86, ARM, ARM64, ...).
//!
//! ## Determinism
//!
//! Every filter is fully deterministic: same input + same parameters ⇒
//! byte-identical output across runs and machines. This is a hard
//! requirement for content-addressed storage.
//!
//! ## Reversibility
//!
//! Every filter's `decode` is the exact inverse of its `encode`:
//! `filter.decode(filter.encode(data)) == data` for every input.
//!
//! ## Filters are not codecs
//!
//! Filters do not compress on their own; they transform data to make a
//! downstream codec more effective. The codec registry and the filter
//! registry are separate concerns. A typical pipeline is:
//!
//! ```text
//! plaintext → delta_filter.encode → lzma.compress → compressed
//! compressed → lzma.decompress → delta_filter.decode → plaintext
//! ```

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

pub mod bcj_x86;
pub mod delta;

pub use bcj_x86::BcjX86Filter;
pub use delta::DeltaFilter;

/// Reversible preprocessing transform. `encode` and `decode` are exact
/// inverses.
pub trait Filter: Send + Sync {
    /// Human-readable name.
    fn name(&self) -> &'static str;
    /// Apply the forward transform.
    fn encode(&self, input: &[u8]) -> Vec<u8>;
    /// Apply the inverse transform. MUST recover the original input
    /// exactly when given the output of [`encode`](Self::encode).
    fn decode(&self, input: &[u8]) -> Vec<u8>;
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod round_trip_tests {
    use super::*;

    fn round_trip<F: Filter>(filter: &F, data: &[u8]) {
        let encoded = filter.encode(data);
        let decoded = filter.decode(&encoded);
        assert_eq!(
            decoded.as_slice(),
            data,
            "round-trip mismatch for {}",
            filter.name()
        );
    }

    #[test]
    fn delta_round_trips_on_various_inputs() {
        let filter = DeltaFilter::new(1);
        round_trip(&filter, b"");
        round_trip(&filter, b"a");
        round_trip(&filter, b"hello world");
        round_trip(&filter, &(0..200u32).map(|i| i as u8).collect::<Vec<_>>());
    }

    #[test]
    fn bcj_x86_round_trips_on_various_inputs() {
        let filter = BcjX86Filter;
        round_trip(&filter, b"");
        round_trip(&filter, b"\x90\x90\x90\x90");
        round_trip(&filter, b"\xe8\x10\x00\x00\x00\xe8\x20\x00\x00\x00");
    }
}
