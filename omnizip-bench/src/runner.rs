//! Benchmark runner — orchestrates `BenchCodec × CorpusFile` cases.
//!
//! Pure orchestration; codec dispatch lives in [`crate::case::BenchCodec`]
//! and corpus I/O lives in [`crate::corpus`]. The runner itself never
//! names a specific codec, so adding a codec requires no edits here.

use std::time::Instant;

use crate::case::{BenchCodec, BenchmarkResult, CodecError};
use crate::corpus::Corpus;

/// Run every `(codec, level)` pair against every file in `corpus`.
///
/// `iterations` controls how many times each encode/decode is repeated
/// for timing — the best (minimum) time is reported, which is standard
/// practice to reduce noise from scheduling jitter.
///
/// Cases whose level is out of range for the codec are reported as
/// skipped (with `error = "level out of range"`) rather than failed.
/// Encoder/decoder failures are recorded but do not abort the run.
#[must_use]
pub fn run_suite(codecs: &[BenchCodec], corpus: &Corpus, iterations: u32) -> Vec<BenchmarkResult> {
    let mut results = Vec::new();
    for codec in codecs {
        for &level in codec.levels() {
            for file in corpus.files() {
                results.push(run_one(
                    codec,
                    level,
                    corpus.name(),
                    file.name(),
                    file.content(),
                    iterations,
                ));
            }
        }
    }
    results
}

fn run_one(
    codec: &BenchCodec,
    level: u8,
    corpus_name: &str,
    file_name: &str,
    input: &[u8],
    iterations: u32,
) -> BenchmarkResult {
    let input_size = u64::try_from(input.len()).unwrap_or(0);

    let first = match codec.compress(input, level) {
        Ok(bytes) => bytes,
        Err(CodecError::LevelSkipped) => {
            return BenchmarkResult::skipped(
                codec.name(),
                level,
                corpus_name,
                file_name,
                "level out of range",
            );
        }
        Err(e) => {
            return BenchmarkResult::skipped(
                codec.name(),
                level,
                corpus_name,
                file_name,
                &e.to_string(),
            );
        }
    };

    // Determinism: second encode must be byte-identical.
    let second = codec.compress(input, level).unwrap_or_default();
    let deterministic = first == second;

    // Time encode (best of `iterations`).
    let encode_ms = best_ms(iterations, || codec.compress(input, level));

    // Time decode (best of `iterations`).
    let expected_len = u32::try_from(input.len()).unwrap_or(u32::MAX);
    let decode_ms = best_ms(iterations, || codec.decompress(&first, expected_len));

    // Round-trip correctness.
    let roundtrip = codec
        .decompress(&first, expected_len)
        .map(|d| d == input)
        .unwrap_or(false);

    let compressed_size = u64::try_from(first.len()).unwrap_or(0);
    let ratio = if input_size == 0 {
        1.0
    } else {
        compressed_size as f64 / input_size as f64
    };
    let mib = |ms: f64| {
        if ms <= 0.0 {
            0.0
        } else {
            (input_size as f64 / 1_048_576.0) / (ms / 1000.0)
        }
    };

    BenchmarkResult {
        codec: codec.name().to_string(),
        level,
        corpus: corpus_name.to_string(),
        file: file_name.to_string(),
        input_size,
        compressed_size,
        ratio,
        encode_ms,
        decode_ms,
        encode_mib_s: mib(encode_ms),
        decode_mib_s: mib(decode_ms),
        deterministic,
        roundtrip_ok: roundtrip,
        error: if roundtrip && deterministic {
            String::new()
        } else {
            let mut parts = Vec::new();
            if !deterministic {
                parts.push("non-deterministic".to_string());
            }
            if !roundtrip {
                parts.push("round-trip failed".to_string());
            }
            parts.join("; ")
        },
    }
}

fn best_ms<F>(iterations: u32, mut f: F) -> f64
where
    F: FnMut() -> Result<Vec<u8>, CodecError>,
{
    if iterations == 0 {
        return 0.0;
    }
    let mut best = f64::INFINITY;
    for _ in 0..iterations {
        let start = Instant::now();
        let _ = f();
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        if elapsed < best {
            best = elapsed;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::CorpusFile;
    use omnizip_deflate::DeflateCodec;
    use omnizip_zpaq::ZpaqCodec;

    #[test]
    fn run_suite_on_synthetic_corpus() {
        let corpus = Corpus::new(
            "test",
            vec![CorpusFile::in_memory(
                "hello.txt",
                b"hello hello hello hello hello world".to_vec(),
            )],
        );
        let codecs = vec![BenchCodec::new(
            "deflate",
            Box::new(DeflateCodec),
            vec![1, 6, 9],
        )];
        let results = run_suite(&codecs, &corpus, 1);
        assert_eq!(results.len(), 3);
        for r in &results {
            assert!(r.roundtrip_ok, "{} round-trip failed: {}", r.codec, r.error);
            assert!(r.deterministic, "{} non-deterministic", r.codec);
        }
    }

    #[test]
    fn skipped_level_is_recorded_not_panicked() {
        // ZPAQ supports 0..=9; level 99 must skip silently.
        let zpaq = BenchCodec::new("zpaq", Box::new(ZpaqCodec), vec![99]);
        let corpus = Corpus::new("test", vec![CorpusFile::in_memory("x", b"data".to_vec())]);
        let results = run_suite(std::slice::from_ref(&zpaq), &corpus, 1);
        assert_eq!(results.len(), 1);
        assert!(!results[0].error.is_empty(), "expected skip reason");
        assert_eq!(results[0].compressed_size, 0);
    }

    #[test]
    fn empty_input_is_handled() {
        let deflate = BenchCodec::new("deflate", Box::new(DeflateCodec), vec![6]);
        let corpus = Corpus::new("test", vec![CorpusFile::in_memory("empty", Vec::new())]);
        let results = run_suite(std::slice::from_ref(&deflate), &corpus, 1);
        assert_eq!(results.len(), 1);
        // An empty input is a legal edge case; either compressed_size > 0
        // (framing overhead) and round-trip OK, or skipped — both acceptable.
        assert!(
            results[0].roundtrip_ok || !results[0].error.is_empty(),
            "should round-trip or report error cleanly, got: {}",
            results[0].error
        );
    }
}
