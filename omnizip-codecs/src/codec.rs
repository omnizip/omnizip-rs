//! Codec identifiers and the Codec trait.
//!
//! Codec ids are u16 to accommodate the full omnizip-rs portfolio (>256
//! entries once every filter variant and newer algorithm is registered).
//! `LimniFS`'s wire-format u8 codec byte maps to a [`CodecId`] at the
//! integration boundary.

use crate::error::OmnizipError;
use crate::level::CompressionLevel;

/// Strongly-typed codec identifier. The inner u16 is opaque; construct
/// via the `CODEC_*` constants on this struct.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct CodecId(u16);

impl CodecId {
    /// Construct a `CodecId` from a raw u16. Intended for codec crates
    /// registering their id; callers should use the named constants below.
    #[must_use]
    pub const fn new(raw: u16) -> Self {
        Self(raw)
    }

    /// Raw u16 value, for serialisation at integration boundaries.
    #[must_use]
    pub const fn as_u16(self) -> u16 {
        self.0
    }

    // --- Assigned codec ids (allocate in task README order) ---

    /// 0x0000: store (no compression).
    pub const STORE: CodecId = Self::new(0x0000);
    /// 0x0001: LZ4 fast.
    pub const LZ4: CodecId = Self::new(0x0001);
    /// 0x0002: Zstandard.
    pub const ZSTD: CodecId = Self::new(0x0002);
    /// 0x0003: LZMA / LZMA2 / XZ.
    pub const LZMA: CodecId = Self::new(0x0003);
    /// 0x0004: Brotli.
    pub const BROTLI: CodecId = Self::new(0x0004);
    /// 0x0005: DEFLATE (RFC 1951).
    pub const DEFLATE: CodecId = Self::new(0x0005);
    /// 0x0006: DEFLATE64 (Microsoft extended).
    pub const DEFLATE64: CodecId = Self::new(0x0006);
    /// 0x0007: bzip2.
    pub const BZIP2: CodecId = Self::new(0x0007);
    /// 0x0008: `PPMd7`.
    pub const PPMD7: CodecId = Self::new(0x0008);
    /// 0x0009: `PPMd8`.
    pub const PPMD8: CodecId = Self::new(0x0009);
    /// 0x000A: Snappy.
    pub const SNAPPY: CodecId = Self::new(0x000A);
    /// 0x000B: libdeflate (DEFLATE-compatible, faster).
    pub const LIBDEFLATE: CodecId = Self::new(0x000B);
    /// 0x000C: LZ4 HC (high-compression variant).
    pub const LZ4_HC: CodecId = Self::new(0x000C);
    /// 0x000D: Reserved (was previously used by GLZA and ZPAQ — collision fixed).
    pub const RESERVED_0D: CodecId = Self::new(0x000D);
    /// 0x000E: Reserved.
    pub const RESERVED_0E: CodecId = Self::new(0x000E);
    /// 0x000F: Reserved.
    pub const RESERVED_0F: CodecId = Self::new(0x000F);
    /// 0x0010: FSST (Fast Static Symbol Table).
    pub const FSST: CodecId = Self::new(0x0010);
    /// 0x0011: Rice++ (`DwarFS` ricepp).
    pub const RICEPP: CodecId = Self::new(0x0011);
    /// 0x0012: FLAC audio.
    pub const FLAC: CodecId = Self::new(0x0012);
    /// 0x0013: BLOSC2 container.
    pub const BLOSC: CodecId = Self::new(0x0013);
    /// 0x0014: GLZA (grammar-based LZ).
    pub const GLZA: CodecId = Self::new(0x0014);
    /// 0x0015: ZPAQ (context-mixing archival).
    pub const ZPAQ: CodecId = Self::new(0x0015);
}

impl std::fmt::Display for CodecId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "0x{:04X}", self.0)
    }
}

/// The behaviour every compression codec implements.
///
/// Codec implementations MUST be deterministic (see crate-level docs).
/// They MUST be `Send + Sync` so the registry can be shared across rayon
/// worker threads.
pub trait Codec: Send + Sync {
    /// The codec id used for dispatch.
    fn id(&self) -> CodecId;

    /// Human-readable name for diagnostics.
    fn name(&self) -> &'static str;

    /// Compress `plaintext` at `level`.
    ///
    /// # Errors
    ///
    /// Returns [`OmnizipError::LevelOutOfRange`] if `level` is outside this
    /// codec's supported range, [`OmnizipError::Unsupported`] if the codec
    /// is decode-only, or [`OmnizipError::EncodeFailed`] on encoder
    /// failure.
    fn compress(&self, plaintext: &[u8], level: CompressionLevel) -> Result<Vec<u8>, OmnizipError>;

    /// Decompress `compressed`, verifying the output length matches
    /// `expected_len` exactly.
    ///
    /// # Errors
    ///
    /// Returns [`OmnizipError::DecodeFailed`] on decoder failure or
    /// [`OmnizipError::LengthMismatch`] if the output length differs from
    /// `expected_len`.
    fn decompress(&self, compressed: &[u8], expected_len: u32) -> Result<Vec<u8>, OmnizipError>;
}
