//! omnizip-bench — benchmark suite for omnizip-rs codecs.
//!
//! Runs every codec in the workspace against standard compression
//! corpora (Calgary, Canterbury, Silesia, Enwik8) and reports ratio,
//! encode/decode throughput, and determinism.
//!
//! ## Architecture
//!
//! Three MECE layers, dependencies always downward:
//!
//! 1. **Models** ([`case`]) — `BenchCodec`, `BenchmarkCase`, `BenchmarkResult`.
//!    Pure data, no I/O.
//! 2. **Corpus** ([`corpus`]) — `Corpus`, `CorpusFile`, downloader/cache.
//! 3. **Runner** ([`runner`]) — orchestrates cases → results.
//! 4. **Reporters** ([`report`]) — `Reporter` trait with CSV / JSON /
//!    Markdown impls (open/closed: new formats = new impl, no runner edits).
//!
//! Adding a codec = one entry in [`default_codecs`]. Adding a corpus =
//! one entry in [`corpus::known_corpora`]. Adding a reporter = one
//! `impl Reporter`. The runner never changes. This is OCP applied to
//! the benchmark harness.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

pub mod case;
pub mod corpus;
pub mod report;
pub mod runner;
pub mod synthetic;

pub use case::{BenchCodec, BenchmarkResult, CodecError};
pub use corpus::{known_corpora, Corpus, CorpusError, CorpusFile};
pub use report::{CsvReporter, JsonReporter, MarkdownReporter, Reporter};
pub use runner::run_suite;

use omnizip_brotli::BrotliCodec;
use omnizip_bzip2::Bzip2Codec;
use omnizip_deflate::DeflateCodec;
use omnizip_deflate64::Deflate64Codec;
use omnizip_glza::GlzaCodec;
use omnizip_lz4::{Lz4FastCodec, Lz4HcCodec};
use omnizip_lzma::LzmaCodec;
use omnizip_ppmd::{Ppmd7Codec, Ppmd8Codec};
use omnizip_snappy::SnappyCodec;
use omnizip_zpaq::ZpaqCodec;
use omnizip_zstd::ZstdCodec;

/// Return the default set of codecs the benchmark knows about.
///
/// Adding a codec = add one entry here. Each entry picks a sensible
/// spread of compression levels; out-of-range levels are silently
/// skipped by the runner (the codec returns `LevelOutOfRange`).
///
/// This is the single place that enumerates codecs — the runner and
/// reporters are codec-agnostic.
#[must_use]
pub fn default_codecs() -> Vec<BenchCodec> {
    vec![
        BenchCodec::new("lzma", Box::new(LzmaCodec), vec![0, 3, 6, 9]),
        BenchCodec::new("zstd", Box::new(ZstdCodec), vec![1, 3, 6, 9, 12, 15, 19, 22]),
        BenchCodec::new("deflate", Box::new(DeflateCodec), vec![1, 6, 9]),
        BenchCodec::new("deflate64", Box::new(Deflate64Codec), vec![1, 6, 9]),
        BenchCodec::new("brotli", Box::new(BrotliCodec), vec![1, 6, 9, 11]),
        BenchCodec::new("bzip2", Box::new(Bzip2Codec), vec![1, 6, 9]),
        BenchCodec::new("glza", Box::new(GlzaCodec), vec![1]),
        BenchCodec::new("lz4", Box::new(Lz4FastCodec), vec![1, 9, 12]),
        BenchCodec::new("lz4hc", Box::new(Lz4HcCodec), vec![3, 9, 12]),
        BenchCodec::new("snappy", Box::new(SnappyCodec), vec![1]),
        BenchCodec::new("ppmd7", Box::new(Ppmd7Codec), vec![1, 6, 9]),
        BenchCodec::new("ppmd8", Box::new(Ppmd8Codec), vec![1, 6, 9]),
        BenchCodec::new("zpaq", Box::new(ZpaqCodec), vec![1, 3, 5]),
    ]
}
