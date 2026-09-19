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

/// Single-input parallel compression for ANY codec — the
/// generalized form of zstd's `compress_mt` job split (task 58).
///
/// `plaintext` is split into fixed `job_size` chunks (an explicit
/// parameter: the caller owns the output contract — job boundaries
/// are part of it); each job is compressed independently and the
/// results concatenate in job order.
///
/// ## Determinism
///
/// Output is byte-identical for any `threads` (jobs are assigned to
/// workers in fixed strided groups, results indexed by job) and for
/// `threads <= 1` or single-job inputs equals the codec's one-shot
/// output exactly.
///
/// ## Concatenation contract
///
/// The codec's output must be independently stream-concatenatable
/// (zstd multi-frame, gzip multi-member, bzip2 and xz multistream
/// qualify; raw single-stream deflate does not). Callers pick codecs
/// with this property — `parallel_compress` cannot verify it.
///
/// # Errors
///
/// The first job error (in job order) propagates; worker panics
/// surface as errors via `join`.
pub fn parallel_compress(
    codec: &dyn crate::Codec,
    plaintext: &[u8],
    level: crate::CompressionLevel,
    threads: usize,
    job_size: usize,
) -> Result<Vec<u8>, crate::OmnizipError> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (threads, job_size);
        return codec.compress(plaintext, level);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let job_size = job_size.max(1);
        if threads <= 1 || plaintext.len() <= job_size {
            return codec.compress(plaintext, level);
        }
        let jobs: Vec<&[u8]> = plaintext.chunks(job_size).collect();
        let workers = threads.min(jobs.len());
        let per = jobs.len().div_ceil(workers);
        let mut results: Vec<Option<Result<Vec<u8>, crate::OmnizipError>>> =
            (0..jobs.len()).map(|_| None).collect();
        let mut worker_panicked = false;
        std::thread::scope(|scope| {
            let handles: Vec<_> = results
                .chunks_mut(per)
                .zip(jobs.chunks(per))
                .map(|(slot, job_chunk)| {
                    scope.spawn(move || {
                        for (slot, job) in slot.iter_mut().zip(job_chunk) {
                            *slot = Some(codec.compress(job, level));
                        }
                    })
                })
                .collect();
            for h in handles {
                if h.join().is_err() {
                    worker_panicked = true;
                }
            }
        });
        if worker_panicked {
            // Surface as job errors rather than unwinding further.
            for slot in &mut results {
                if slot.is_none() {
                    *slot = Some(Err(crate::OmnizipError::EncodeFailed {
                        codec: codec.id(),
                        reason: "parallel worker panicked".to_string(),
                    }));
                }
            }
        }
        let mut out = Vec::with_capacity(plaintext.len() / 2 + 64 * jobs.len());
        for (i, r) in results.into_iter().enumerate() {
            out.extend_from_slice(&r.unwrap_or_else(|| {
                Err(crate::OmnizipError::EncodeFailed {
                    codec: codec.id(),
                    reason: format!("job {i} produced no result"),
                })
            })?);
        }
        Ok(out)
    }
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

#[cfg(test)]
mod parallel_compress_tests {
    use super::parallel_compress;
    use crate::codec::CodecId;
    use crate::level::CompressionLevel;
    use crate::{Codec, OmnizipError};

    /// Identity codec that tags each job with its length prefix, so
    /// job splitting and ordering are observable in the output.
    struct TaggedCodec;

    impl Codec for TaggedCodec {
        fn id(&self) -> CodecId {
            CodecId::new(0xFF01)
        }
        fn name(&self) -> &'static str {
            "tagged"
        }
        fn compress(
            &self,
            plaintext: &[u8],
            _level: CompressionLevel,
        ) -> Result<Vec<u8>, OmnizipError> {
            let mut out = u32::try_from(plaintext.len())
                .unwrap()
                .to_le_bytes()
                .to_vec();
            out.extend_from_slice(plaintext);
            Ok(out)
        }
        fn decompress(
            &self,
            _compressed: &[u8],
            _expected_len: u32,
        ) -> Result<Vec<u8>, OmnizipError> {
            unimplemented!("test codec: compress-only")
        }
    }

    /// Fails every job past the first — error propagation check.
    struct FlakyCodec;

    impl Codec for FlakyCodec {
        fn id(&self) -> CodecId {
            CodecId::new(0xFF02)
        }
        fn name(&self) -> &'static str {
            "flaky"
        }
        fn compress(&self, _p: &[u8], _l: CompressionLevel) -> Result<Vec<u8>, OmnizipError> {
            Err(OmnizipError::EncodeFailed {
                codec: self.id(),
                reason: "planned failure".into(),
            })
        }
        fn decompress(&self, _c: &[u8], _e: u32) -> Result<Vec<u8>, OmnizipError> {
            unimplemented!("test codec: compress-only")
        }
    }

    fn data(len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| u8::try_from(i % 251).expect("< 251"))
            .collect()
    }

    #[test]
    fn thread_count_invariant_output() {
        // Every config that SPLITS (threads >= 2) must produce the
        // same bytes regardless of worker count. (threads <= 1 is
        // the documented one-shot shortcut — covered by
        // single_job_equals_one_shot.)
        let input = data(10_000);
        let level = CompressionLevel::default();
        let t2 = parallel_compress(&TaggedCodec, &input, level, 2, 1024).unwrap();
        let t8 = parallel_compress(&TaggedCodec, &input, level, 8, 1024).unwrap();
        let t16 = parallel_compress(&TaggedCodec, &input, level, 16, 1024).unwrap();
        assert_eq!(t2, t8);
        assert_eq!(t8, t16);
    }

    #[test]
    fn single_job_equals_one_shot() {
        let input = data(500);
        let level = CompressionLevel::default();
        let one_shot = TaggedCodec.compress(&input, level).unwrap();
        let parallel = parallel_compress(&TaggedCodec, &input, level, 8, 4096).unwrap();
        assert_eq!(one_shot, parallel);
    }

    #[test]
    fn jobs_split_and_ordered() {
        // 3000 bytes / 1024-job = 3 jobs (1024, 1024, 952): each
        // output segment = LE length tag + payload, concatenated in
        // job order.
        let input = data(3000);
        let out =
            parallel_compress(&TaggedCodec, &input, CompressionLevel::default(), 4, 1024).unwrap();
        let mut expect = Vec::new();
        for chunk in input.chunks(1024) {
            expect.extend_from_slice(&u32::try_from(chunk.len()).unwrap().to_le_bytes());
            expect.extend_from_slice(chunk);
        }
        assert_eq!(out, expect);
    }

    #[test]
    fn job_error_propagates() {
        let input = data(3000);
        let err = match parallel_compress(&FlakyCodec, &input, CompressionLevel::default(), 4, 1024)
        {
            Err(e) => e,
            Ok(_) => panic!("codec failure must propagate"),
        };
        assert!(err.to_string().contains("planned failure"), "{err}");
    }

    #[test]
    fn empty_and_tiny_inputs() {
        let level = CompressionLevel::default();
        assert!(parallel_compress(&TaggedCodec, &[], level, 4, 1024).is_ok());
        let tiny = parallel_compress(&TaggedCodec, b"xy", level, 4, 1024).unwrap();
        assert_eq!(tiny, TaggedCodec.compress(b"xy", level).unwrap());
    }
}
