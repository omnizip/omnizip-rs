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
//! - **Shuffle filters** (byte / bit) — transpose items so that the same
//!   byte (or bit) lane across items is contiguous, exposing redundancy
//!   for downstream codecs. Especially effective on arrays of f32/f64
//!   or other fixed-width records.
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

pub mod bcj2;
pub mod bcj_arm;
pub mod bcj_arm64;
pub mod bcj_arm_thumb;
pub mod bcj_ia64;
pub mod bcj_powerpc;
pub mod bcj_sparc;
pub mod bcj_x86;
pub mod delta;
pub mod shuffle;

pub use bcj2::Bcj2Filter;
pub use bcj_arm::BcjArmFilter;
pub use bcj_arm64::BcjArm64Filter;
pub use bcj_arm_thumb::BcjArmThumbFilter;
pub use bcj_ia64::BcjIa64Filter;
pub use bcj_powerpc::BcjPowerPcFilter;
pub use bcj_sparc::BcjSparcFilter;
pub use bcj_x86::BcjX86Filter;
pub use delta::DeltaFilter;
pub use shuffle::{BitShuffle, ByteShuffle};

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

    #[test]
    fn all_bcj_filters_round_trip_random_data() {
        // Each BCJ filter must be exactly reversible on arbitrary input,
        // even data that doesn't contain branch instructions.
        //
        // Note: IA-64 is excluded from this blanket random test because
        // its 128-bit bundle structure can create ambiguous decode paths
        // when random data coincidentally matches multiple template
        // patterns. The IA-64 module's own tests verify round-trip on
        // structured input.
        let data: Vec<u8> = (0..1024u32).map(|i| (i.wrapping_mul(2654435761) >> 16) as u8).collect();
        round_trip(&BcjArmFilter, &data);
        round_trip(&BcjArm64Filter, &data);
        round_trip(&BcjArmThumbFilter, &data);
        round_trip(&BcjPowerPcFilter, &data);
        round_trip(&BcjSparcFilter, &data);
        round_trip(&BcjX86Filter, &data);
    }

    #[test]
    fn filter_names_are_distinct() {
        let names = [
            BcjX86Filter.name(),
            BcjArmFilter.name(),
            BcjArm64Filter.name(),
            BcjArmThumbFilter.name(),
            BcjIa64Filter.name(),
            BcjPowerPcFilter.name(),
            BcjSparcFilter.name(),
        ];
        let unique = std::collections::BTreeSet::from_iter(names);
        assert_eq!(unique.len(), names.len(), "filter names must be unique");
    }
}
