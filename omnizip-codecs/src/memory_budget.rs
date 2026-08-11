//! Memory budget estimation (TODO 264).
//!
//! Per-codec trait that estimates peak memory usage for a given input
//! size + compression level. Lets callers choose codecs adaptively
//! based on available memory.

use crate::level::CompressionLevel;

/// Estimator for peak memory usage of a compression/decompression
/// operation. Codecs override with accurate per-codec models.
///
/// ## Determinism
///
/// Pure function of `(input_len, level)`. No time-seeded RNG.
pub trait MemoryBudget {
    /// Estimated peak memory in bytes for compressing `input_len`
    /// bytes at `level`.
    fn estimated_compress_memory(&self, input_len: usize, _level: CompressionLevel) -> usize {
        // Default: input + output buffers + small overhead.
        input_len + input_len / 2 + 4096
    }

    /// Estimated peak memory in bytes for decompressing data that was
    /// originally `original_len` bytes.
    fn estimated_decompress_memory(&self, original_len: usize) -> usize {
        // Default: output buffer + small overhead.
        original_len + 4096
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyCodec;

    impl MemoryBudget for DummyCodec {
        fn estimated_compress_memory(&self, input_len: usize, level: CompressionLevel) -> usize {
            input_len * 2 + usize::from(level.as_u8()) * 1024
        }
    }

    #[test]
    fn default_estimates_are_reasonable() {
        // A type without explicit impl uses the default.
        struct Defaulted;
        impl MemoryBudget for Defaulted {}
        let d = Defaulted;
        let m = d.estimated_compress_memory(1000, CompressionLevel::new(5));
        assert!(m >= 1000);
    }

    #[test]
    fn override_takes_effect() {
        let d = DummyCodec;
        let m = d.estimated_compress_memory(1000, CompressionLevel::new(5));
        assert_eq!(m, 1000 * 2 + 5 * 1024);
    }
}
