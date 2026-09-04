//! ZSTD frame encoder — delegates to [`block::encode_frame_compressed`].
//!
//! ## Frame layout
//!
//! ```text
//! Magic_Bytes         4 bytes: 0x28 0xB5 0x2F 0xFD (LE = 0xFD2FB528)
//! Frame_Header        1-5 bytes (descriptor + optional fields)
//! Block_Header        3 bytes (LE): block_type | last_block | block_size
//! Block_Data          variable
//! [Content_Checksum]  4 bytes if descriptor.content_checksum_flag == 1
//! ```

#![forbid(unsafe_code)]

pub mod block;
pub mod cparams;
pub mod ldm;
pub mod match_finder;
pub mod opt;
pub mod sequences;

use crate::{ZstdError, ZstdLevel};

/// Compress `plaintext` into a ZSTD frame at the given level.
///
/// Uses compressed blocks (match finder + FSE-coded sequences +
/// Raw/Huffman literals). Falls back to Raw blocks when compression
/// doesn't help. The output always round-trips through any ZSTD
/// decoder.
///
/// # Errors
///
/// Returns [`ZstdError::Corrupt`] only on internal arithmetic
/// overflow (shouldn't happen for any plausible input).
pub fn encode_frame(plaintext: &[u8], level: ZstdLevel) -> Result<Vec<u8>, ZstdError> {
    block::encode_frame_compressed(plaintext, level.as_reference_level())
}

/// Multi-threaded compression: split `plaintext` into fixed-size jobs
/// (default 4 MiB, `ZSTD_MT_JOB` overrides for measurement) and encode
/// each as an independent frame across scoped worker threads,
/// concatenating in job order.
///
/// Deterministic by construction: job boundaries are a pure function
/// of input length and level — never of the thread count — and each
/// job is a self-contained frame, so the output is identical for any
/// `threads` value. `threads <= 1` and single-job inputs fall through
/// to the single-frame [`encode_frame`] path (byte-identical to
/// [`crate::compress`]).
///
/// Ratio note: matches cannot cross job boundaries, so multi-job
/// output is slightly larger than single-frame on inputs whose
/// redundancy spans more than one job; the delta is documented in
/// `TODO.remaining/19`.
///
/// # Errors
///
/// See [`encode_frame`].
pub fn encode_frames_mt(
    plaintext: &[u8],
    level: ZstdLevel,
    threads: usize,
) -> Result<Vec<u8>, ZstdError> {
    // Job size is part of the output contract (boundaries are a pure
    // function of input length + level), so the default is fixed per
    // level family, not tuned per content. Best keeps jobs large:
    // the opt tier's ratio is history-sensitive (measured on the most
    // history-dependent corpus cell, periodic CSV 17.8 MB: 4 MiB jobs
    // +28% output, 8 MiB +12%, 16 MiB +6.7% — still under the
    // single-thread reference at 16 MiB). Lower levels measured ~0 or
    // negative delta at 4 MiB jobs. ZSTD_MT_JOB overrides for
    // measurement.
    let default_job = if level == ZstdLevel::Best {
        16 * 1024 * 1024
    } else {
        4 * 1024 * 1024
    };
    let job_size: usize = std::env::var("ZSTD_MT_JOB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default_job)
        .max(1024);

    #[cfg(target_arch = "wasm32")]
    {
        let _ = threads;
        encode_frame(plaintext, level)
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        if threads <= 1 || plaintext.len() <= job_size {
            return encode_frame(plaintext, level);
        }
        let jobs: Vec<&[u8]> = plaintext.chunks(job_size).collect();
        let workers = threads.min(jobs.len()).min(8);
        let lv = level.as_reference_level();
        let mut results: Vec<Option<Result<Vec<u8>, ZstdError>>> =
            (0..jobs.len()).map(|_| None).collect();
        let per = jobs.len().div_ceil(workers);
        std::thread::scope(|scope| {
            let handles: Vec<_> = results
                .chunks_mut(per)
                .zip(jobs.chunks(per))
                .map(|(slot, job_chunk)| {
                    scope.spawn(move || {
                        for (slot, job) in slot.iter_mut().zip(job_chunk) {
                            *slot = Some(block::encode_frame_compressed(job, lv));
                        }
                    })
                })
                .collect();
            for h in handles {
                h.join().expect("zstd job encoder panicked");
            }
        });
        let mut out = Vec::with_capacity(plaintext.len() / 2 + 64 * results.len());
        for r in results {
            out.extend_from_slice(&r.expect("every job encoded")?);
        }
        Ok(out)
    }
}

/// Compress `plaintext` into a ZSTD frame primed with a dictionary.
///
/// Delegates to [`block::encode_frame_with_dict`]. See its docs for
/// the dictionary-prefix strategy.
///
/// # Errors
///
/// Returns [`ZstdError::Corrupt`] only on internal arithmetic
/// overflow.
pub fn encode_frame_with_dict(
    plaintext: &[u8],
    level: u8,
    dict: &crate::dict::ZstdDictionary,
) -> Result<Vec<u8>, ZstdError> {
    block::encode_frame_with_dict(plaintext, level, dict)
}
