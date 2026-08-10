//! Parallel batch compression/decompression.
//!
//! LimniFS and similar batch workloads compress many independent
//! inputs in a tight loop. This module provides a default parallel
//! implementation that any [`Codec`](crate::Codec) gets for free,
//! using `std::thread::scope` (no rayon dependency, no `unsafe`).
//!
//! ## Determinism
//!
//! Each input is compressed in its own thread. No shared mutable
//! state. The same input + level produces byte-identical output
//! regardless of:
//!
//! - Number of inputs in the batch
//! - Order of inputs in the batch
//! - Thread scheduling
//!
//! See ADR-0004 for the determinism requirement.

use crate::codec::Codec;
use crate::error::OmnizipError;
use crate::level::CompressionLevel;

/// Parallel batch operations for any [`Codec`].
///
/// Default implementation uses `std::thread::scope` to spread work
/// across cores. Codecs that want finer control (e.g., shared
/// dictionary precomputed once) can override.
///
/// ## Determinism guarantee
///
/// Same inputs + same level → byte-identical outputs, regardless
/// of thread scheduling. Each input is compressed independently
/// with no shared mutable state.
pub trait ParallelBatch: Codec {
    /// Compress many inputs in parallel.
    ///
    /// Returns results in input order. If any input fails, its slot
    /// is an `Err`; other inputs are still processed.
    ///
    /// # Errors
    ///
    /// Same as [`Codec::compress`] per failing input.
    fn compress_batch(
        &self,
        inputs: &[&[u8]],
        level: CompressionLevel,
    ) -> Vec<Result<Vec<u8>, OmnizipError>> {
        if inputs.is_empty() {
            return Vec::new();
        }
        if inputs.len() == 1 {
            return vec![self.compress(inputs[0], level)];
        }

        let num_threads = inputs.len().min(num_cpus());
        let chunk_size = inputs.len().div_ceil(num_threads);
        let chunks: Vec<&[&[u8]]> = inputs.chunks(chunk_size).collect();

        // `std::thread::scope` lets spawned threads borrow non-'static
        // references. The scope ensures all threads are joined before
        // returning, so the borrowed references outlive all uses.
        // No `unsafe` needed.
        let thread_results: Vec<Vec<Result<Vec<u8>, OmnizipError>>> = std::thread::scope(|scope| {
            let handles: Vec<_> = chunks
                .into_iter()
                .map(|chunk| {
                    scope.spawn(move || -> Vec<Result<Vec<u8>, OmnizipError>> {
                        chunk
                            .iter()
                            .map(|input| self.compress(input, level))
                            .collect()
                    })
                })
                .collect();

            handles
                .into_iter()
                .map(|h| h.join().expect("compress worker thread panicked"))
                .collect()
        });

        // Flatten chunks back into input order.
        let total = inputs.len();
        let mut all_results = Vec::with_capacity(total);
        for chunk_result in thread_results {
            all_results.extend(chunk_result);
        }
        debug_assert_eq!(all_results.len(), total);
        all_results
    }

    /// Decompress many inputs in parallel.
    ///
    /// `inputs[i]` should decompress to `expected_lens[i]` bytes.
    /// Returns results in input order.
    ///
    /// # Errors
    ///
    /// Same as [`Codec::decompress`] per failing input.
    ///
    /// # Panics
    ///
    /// Panics if `inputs.len() != expected_lens.len()`.
    fn decompress_batch(
        &self,
        inputs: &[&[u8]],
        expected_lens: &[u32],
    ) -> Vec<Result<Vec<u8>, OmnizipError>> {
        assert_eq!(
            inputs.len(),
            expected_lens.len(),
            "inputs and expected_lens must have the same length"
        );
        if inputs.is_empty() {
            return Vec::new();
        }
        if inputs.len() == 1 {
            return vec![self.decompress(inputs[0], expected_lens[0])];
        }

        let num_threads = inputs.len().min(num_cpus());
        let chunk_size = inputs.len().div_ceil(num_threads);

        let input_chunks: Vec<&[&[u8]]> = inputs.chunks(chunk_size).collect();
        let len_chunks: Vec<&[u32]> = expected_lens.chunks(chunk_size).collect();

        let thread_results: Vec<Vec<Result<Vec<u8>, OmnizipError>>> = std::thread::scope(|scope| {
            let handles: Vec<_> = input_chunks
                .into_iter()
                .zip(len_chunks.into_iter())
                .map(|(input_chunk, len_chunk)| {
                    scope.spawn(move || -> Vec<Result<Vec<u8>, OmnizipError>> {
                        input_chunk
                            .iter()
                            .zip(len_chunk.iter())
                            .map(|(input, &expected)| self.decompress(input, expected))
                            .collect()
                    })
                })
                .collect();

            handles
                .into_iter()
                .map(|h| h.join().expect("decompress worker thread panicked"))
                .collect()
        });

        let mut all_results = Vec::with_capacity(inputs.len());
        for chunk_result in thread_results {
            all_results.extend(chunk_result);
        }
        debug_assert_eq!(all_results.len(), inputs.len());
        all_results
    }
}

// Blanket impl: every Codec is ParallelBatch.
impl<T: Codec + ?Sized> ParallelBatch for T {}

/// Number of CPUs to use for batch operations.
fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::CodecId;

    /// A trivial codec that doubles each byte. Used for testing
    /// without depending on real codecs.
    pub struct DoubleCodec;

    impl Codec for DoubleCodec {
        fn id(&self) -> CodecId {
            CodecId::new(0xFFFE)
        }
        fn name(&self) -> &'static str {
            "double"
        }
        fn compress(
            &self,
            plaintext: &[u8],
            _level: CompressionLevel,
        ) -> Result<Vec<u8>, OmnizipError> {
            Ok(plaintext.iter().map(|&b| b.wrapping_mul(2)).collect())
        }
        fn decompress(
            &self,
            compressed: &[u8],
            _expected_len: u32,
        ) -> Result<Vec<u8>, OmnizipError> {
            Ok(compressed.iter().map(|&b| b.wrapping_mul(2)).collect())
        }
    }

    #[test]
    fn empty_batch_returns_empty() {
        let codec = DoubleCodec;
        let results: Vec<_> = codec.compress_batch(&[], CompressionLevel::new(1));
        assert!(results.is_empty());
    }

    #[test]
    fn single_input_batch_uses_fast_path() {
        let codec = DoubleCodec;
        let input = b"hello";
        let results: Vec<_> = codec.compress_batch(&[input], CompressionLevel::new(1));
        assert_eq!(results.len(), 1);
        // h=104, e=101, l=108, l=108, o=111; each doubled
        assert_eq!(
            results[0].as_ref().unwrap(),
            &vec![104 * 2, 101 * 2, 108 * 2, 108 * 2, 111 * 2]
        );
    }

    #[test]
    fn multi_input_batch_returns_in_order() {
        let codec = DoubleCodec;
        let a = b"abc";
        let b = b"defgh";
        let c = b"i";
        let results: Vec<_> = codec.compress_batch(&[a, b, c], CompressionLevel::new(1));
        assert_eq!(results.len(), 3);
        // Each byte doubled (wrapping_mul(2))
        assert_eq!(
            results[0].as_ref().unwrap(),
            &vec![0x61 * 2, 0x62 * 2, 0x63 * 2]
        );
        assert_eq!(
            results[1].as_ref().unwrap(),
            &vec![0x64 * 2, 0x65 * 2, 0x66 * 2, 0x67 * 2, 0x68 * 2]
        );
        assert_eq!(results[2].as_ref().unwrap(), &vec![0x69 * 2]);
    }

    #[test]
    fn decompress_batch_works() {
        let codec = DoubleCodec;
        let inputs = vec![b"\xc8\xdc\xdc".as_slice(), b"\xdc"];
        let lens = vec![3u32, 1u32];
        let input_refs: Vec<&[u8]> = inputs.iter().copied().collect();
        let results: Vec<_> = codec.decompress_batch(&input_refs, &lens);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn batch_determinism_across_runs() {
        // Same inputs must produce identical outputs across multiple runs.
        let codec = DoubleCodec;
        let inputs: Vec<&[u8]> = vec![b"aaa", b"bbb", b"ccc", b"ddd", b"eee"];
        let run1: Vec<Vec<u8>> = codec
            .compress_batch(&inputs, CompressionLevel::new(1))
            .into_iter()
            .map(|r| r.expect("compress failed"))
            .collect();
        let run2: Vec<Vec<u8>> = codec
            .compress_batch(&inputs, CompressionLevel::new(1))
            .into_iter()
            .map(|r| r.expect("compress failed"))
            .collect();
        assert_eq!(run1, run2);
    }
}
