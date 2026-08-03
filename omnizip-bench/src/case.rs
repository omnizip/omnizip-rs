//! Benchmark case + result models. Pure data, no I/O.

use omnizip_codecs::{Codec, CodecId, CompressionLevel};
use serde::{Deserialize, Serialize};

/// A codec wrapper that knows the levels to benchmark.
///
/// Holds a `Box<dyn Codec>` so the runner is codec-agnostic (OCP: new
/// codecs are added by constructing more `BenchCodec` values, not by
/// editing runner branches).
pub struct BenchCodec {
    name: &'static str,
    codec: Box<dyn Codec>,
    levels: Vec<u8>,
}

impl BenchCodec {
    #[must_use]
    pub fn new(name: &'static str, codec: Box<dyn Codec>, levels: Vec<u8>) -> Self {
        Self { name, codec, levels }
    }

    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }

    #[must_use]
    pub fn id(&self) -> CodecId {
        self.codec.id()
    }

    #[must_use]
    pub fn levels(&self) -> &[u8] {
        &self.levels
    }

    /// Consumer-rebuild with a different level set. Used by the CLI to
    /// honour `--level` overrides without exposing a mutable accessor.
    #[must_use]
    pub fn with_levels(mut self, levels: Vec<u8>) -> Self {
        self.levels = levels;
        self
    }

    /// Compress at `level`, returning the compressed bytes.
    ///
    /// # Errors
    ///
    /// - [`CodecError::LevelSkipped`] when `level` is outside the codec's
    ///   supported range — the runner treats this as "skip this case",
    ///   not as a failure.
    /// - [`CodecError::Encode`] on actual encoder failure.
    pub fn compress(&self, input: &[u8], level: u8) -> Result<Vec<u8>, CodecError> {
        match self.codec.compress(input, CompressionLevel::new(level)) {
            Ok(bytes) => Ok(bytes),
            Err(omnizip_codecs::OmnizipError::LevelOutOfRange { .. }) => {
                Err(CodecError::LevelSkipped)
            }
            Err(e) => Err(CodecError::Encode(e.to_string())),
        }
    }

    /// Decompress, verifying the round-trip equals `expected`.
    ///
    /// # Errors
    ///
    /// See [`CodecError`].
    pub fn decompress(
        &self,
        compressed: &[u8],
        expected_len: u32,
    ) -> Result<Vec<u8>, CodecError> {
        match self.codec.decompress(compressed, expected_len) {
            Ok(bytes) => Ok(bytes),
            Err(e) => Err(CodecError::Decode(e.to_string())),
        }
    }
}

/// Codec-level outcome categories the runner distinguishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError {
    /// Level outside the codec's supported range — skip silently.
    LevelSkipped,
    /// Encoder returned an error.
    Encode(String),
    /// Decoder returned an error.
    Decode(String),
}

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LevelSkipped => write!(f, "level out of range (skipped)"),
            Self::Encode(s) => write!(f, "encode error: {s}"),
            Self::Decode(s) => write!(f, "decode error: {s}"),
        }
    }
}

impl std::error::Error for CodecError {}

/// Outcome of one `(codec, level, file)` benchmark case.
///
/// Serialized by reporters — all fields are `pub` and `Serialize`.
/// Ratio is `compressed_size / input_size` (smaller = better).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkResult {
    pub codec: String,
    pub level: u8,
    pub corpus: String,
    pub file: String,
    pub input_size: u64,
    pub compressed_size: u64,
    pub ratio: f64,
    pub encode_ms: f64,
    pub decode_ms: f64,
    pub encode_mib_s: f64,
    pub decode_mib_s: f64,
    pub deterministic: bool,
    pub roundtrip_ok: bool,
    /// Empty on success; otherwise a short error category string.
    pub error: String,
}

impl BenchmarkResult {
    /// Mark this case as skipped (level out of range, etc.).
    pub(crate) fn skipped(
        codec: &str,
        level: u8,
        corpus: &str,
        file: &str,
        reason: &str,
    ) -> Self {
        Self {
            codec: codec.to_string(),
            level,
            corpus: corpus.to_string(),
            file: file.to_string(),
            input_size: 0,
            compressed_size: 0,
            ratio: f64::NAN,
            encode_ms: 0.0,
            decode_ms: 0.0,
            encode_mib_s: 0.0,
            decode_mib_s: 0.0,
            deterministic: false,
            roundtrip_ok: false,
            error: reason.to_string(),
        }
    }
}
