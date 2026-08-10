//! Per-codec structured error sub-types.
//!
//! These give callers pattern-matching detail beyond the top-level
//! [`OmnizipError`](crate::OmnizipError) variants. Each codec may
//! have its own error enum; `OmnizipError` continues to be the
//! unified type via `From` conversions.

/// Brotli-specific structured errors.
#[derive(Debug)]
pub enum BrotliError {
    /// Metablock header failed to parse or had inconsistent values.
    InvalidMetablockHeader(&'static str),
    /// Literal context ID out of range or inconsistent with mode.
    InvalidLiteralContext(u8),
    /// Distance code exceeded `max_backward_distance` or
    /// referenced a position before output start.
    InvalidDistance(u32),
    /// Static dictionary lookup failed (unknown word/transform).
    DictionaryLookupFailed,
    /// Huffman tree violates Kraft inequality or other invariant.
    InvalidHuffmanTree(&'static str),
    /// Wire-format divergence from RFC 7932.
    WireFormat(String),
}

impl std::fmt::Display for BrotliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidMetablockHeader(r) => {
                write!(f, "invalid metablock header: {r}")
            }
            Self::InvalidLiteralContext(c) => write!(f, "invalid literal context: {c}"),
            Self::InvalidDistance(d) => write!(f, "invalid distance: {d}"),
            Self::DictionaryLookupFailed => write!(f, "dictionary lookup failed"),
            Self::InvalidHuffmanTree(r) => write!(f, "invalid Huffman tree: {r}"),
            Self::WireFormat(r) => write!(f, "wire-format: {r}"),
        }
    }
}

impl std::error::Error for BrotliError {}

/// LZMA-specific structured errors.
#[derive(Debug)]
pub enum LzmaError {
    /// Range coder state inconsistent (carry / renorm).
    RangeCoder(&'static str),
    /// Probability model out of range or uninitialized.
    ProbabilityModel(&'static str),
    /// Match finder hit an invalid state (e.g., distance > window).
    MatchFinder(&'static str),
    /// XZ stream header/footer CRC or magic mismatch.
    ContainerCorrupt(&'static str),
    /// LZMA2 chunk size or property byte out of range.
    Lzma2Chunk(&'static str),
}

impl std::fmt::Display for LzmaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RangeCoder(r) => write!(f, "range coder: {r}"),
            Self::ProbabilityModel(r) => write!(f, "probability model: {r}"),
            Self::MatchFinder(r) => write!(f, "match finder: {r}"),
            Self::ContainerCorrupt(r) => write!(f, "container corrupt: {r}"),
            Self::Lzma2Chunk(r) => write!(f, "LZMA2 chunk: {r}"),
        }
    }
}

impl std::error::Error for LzmaError {}

/// ZSTD-specific structured errors.
#[derive(Debug)]
pub enum ZstdError {
    /// Frame header magic or field invalid.
    FrameHeader(&'static str),
    /// Block header invalid or block size exceeds frame window.
    BlockHeader(&'static str),
    /// FSE table malformed (incorrect probabilities, max accuracy).
    FseTable(&'static str),
    /// Huffman tree violates Kraft or has too many/long codes.
    HuffmanTree(&'static str),
    /// XXHash64 frame checksum mismatch.
    ChecksumMismatch { expected: u64, actual: u64 },
    /// Sequence section malformed.
    Sequences(&'static str),
}

impl std::fmt::Display for ZstdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FrameHeader(r) => write!(f, "frame header: {r}"),
            Self::BlockHeader(r) => write!(f, "block header: {r}"),
            Self::FseTable(r) => write!(f, "FSE table: {r}"),
            Self::HuffmanTree(r) => write!(f, "Huffman tree: {r}"),
            Self::ChecksumMismatch { expected, actual } => {
                write!(
                    f,
                    "checksum mismatch: expected {expected:016x}, got {actual:016x}"
                )
            }
            Self::Sequences(r) => write!(f, "sequences: {r}"),
        }
    }
}

impl std::error::Error for ZstdError {}
