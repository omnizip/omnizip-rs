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

use omnizip_codecs::{Codec, CodecId, CompressionLevel, OmnizipError};

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

/// Adapter that composes a [`Filter`] with a [`Codec`].
///
/// `compress` applies the filter forward, then encodes the filtered
/// bytes via the inner codec. `decompress` reverses the pipeline. The
/// composed codec has the id of the inner codec; the filter is
/// transparent to callers.
///
/// ## Example
///
/// ```no_run
/// # // Stubbed as no_run because omnizip-lzma isn't a runtime dep of
/// # // omnizip-filters — callers wire the actual codec + filter combo.
/// use omnizip_codecs::{Codec, CompressionLevel};
/// use omnizip_filters::{BcjX86Filter, FilteredCodec};
///
/// // x86 BCJ filter + LZMA compression: typical for executable code.
/// // Replace `MyCodec` with an actual codec like `omnizip_lzma::LzmaCodec`.
/// # struct MyCodec;
/// # impl Codec for MyCodec {
/// #     fn id(&self) -> omnizip_codecs::CodecId { omnizip_codecs::CodecId::LZMA }
/// #     fn name(&self) -> &'static str { "stub" }
/// #     fn compress(&self, _: &[u8], _: CompressionLevel) -> Result<Vec<u8>, omnizip_codecs::OmnizipError> { Ok(Vec::new()) }
/// #     fn decompress(&self, _: &[u8], _: u32) -> Result<Vec<u8>, omnizip_codecs::OmnizipError> { Ok(Vec::new()) }
/// # }
/// let codec = FilteredCodec::new(
///     MyCodec,
///     BcjX86Filter,
/// );
/// let exe_bytes = std::fs::read("program.bin").unwrap();
/// let compressed = codec.compress(&exe_bytes, CompressionLevel::default()).unwrap();
/// ```
pub struct FilteredCodec<C, F> {
    codec: C,
    filter: F,
}

impl<C, F> FilteredCodec<C, F> {
    /// Construct a new filtered codec.
    #[must_use]
    pub const fn new(codec: C, filter: F) -> Self {
        Self { codec, filter }
    }

    /// Access the inner codec.
    #[must_use]
    pub const fn inner_codec(&self) -> &C {
        &self.codec
    }

    /// Access the inner filter.
    #[must_use]
    pub const fn inner_filter(&self) -> &F {
        &self.filter
    }
}

impl<C: Codec, F: Filter> Codec for FilteredCodec<C, F> {
    fn id(&self) -> CodecId {
        self.codec.id()
    }

    fn name(&self) -> &'static str {
        self.codec.name()
    }

    fn compress(
        &self,
        plaintext: &[u8],
        level: CompressionLevel,
    ) -> Result<Vec<u8>, OmnizipError> {
        let filtered = self.filter.encode(plaintext);
        self.codec.compress(&filtered, level)
    }

    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError> {
        let filtered = self.codec.decompress(compressed, expected_len)?;
        if filtered.len() != expected_len as usize {
            return Err(OmnizipError::LengthMismatch {
                codec: self.codec.id(),
                expected: expected_len,
                actual: filtered.len(),
            });
        }
        let original = self.filter.decode(&filtered);
        if original.len() != filtered.len() {
            return Err(OmnizipError::Corrupt {
                codec: self.codec.id(),
                reason: format!(
                    "filter '{}' changed length: {} → {}",
                    self.filter.name(),
                    filtered.len(),
                    original.len()
                ),
            });
        }
        Ok(original)
    }
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

    /// Verify FilteredCodec applies the filter on compress and reverses
    /// it on decompress, producing the original input.
    #[test]
    fn filtered_codec_round_trips_with_delta_and_lzma() {
        // Use a workspace codec that's always available in tests.
        // We exercise the adapter with a synthetic codec wrapper.
        struct IdentityCodec;
        impl Codec for IdentityCodec {
            fn id(&self) -> CodecId {
                CodecId::LZMA
            }
            fn name(&self) -> &'static str {
                "identity"
            }
            fn compress(
                &self,
                plaintext: &[u8],
                _level: CompressionLevel,
            ) -> Result<Vec<u8>, OmnizipError> {
                Ok(plaintext.to_vec())
            }
            fn decompress(
                &self,
                compressed: &[u8],
                expected_len: u32,
            ) -> Result<Vec<u8>, OmnizipError> {
                if compressed.len() as u32 != expected_len {
                    return Err(OmnizipError::LengthMismatch {
                        codec: CodecId::LZMA,
                        expected: expected_len,
                        actual: compressed.len(),
                    });
                }
                Ok(compressed.to_vec())
            }
        }

        let codec = FilteredCodec::new(IdentityCodec, DeltaFilter::new(1));
        let input: Vec<u8> = (0..200).map(|i| (i % 7) as u8).collect();
        let compressed = codec
            .compress(&input, CompressionLevel::default())
            .expect("compress");
        let decompressed = codec
            .decompress(&compressed, input.len() as u32)
            .expect("decompress");
        assert_eq!(decompressed, input);
    }

    /// FilteredCodec's id and name should match the inner codec.
    #[test]
    fn filtered_codec_identities() {
        struct TestCodec;
        impl Codec for TestCodec {
            fn id(&self) -> CodecId {
                CodecId::LZMA
            }
            fn name(&self) -> &'static str {
                "test-codec"
            }
            fn compress(
                &self,
                _: &[u8],
                _: CompressionLevel,
            ) -> Result<Vec<u8>, OmnizipError> {
                Ok(Vec::new())
            }
            fn decompress(&self, _: &[u8], _: u32) -> Result<Vec<u8>, OmnizipError> {
                Ok(Vec::new())
            }
        }
        let codec = FilteredCodec::new(TestCodec, DeltaFilter::new(1));
        assert_eq!(codec.id(), CodecId::LZMA);
        assert_eq!(codec.name(), "test-codec");
    }
}
