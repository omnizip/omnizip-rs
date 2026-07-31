//! Compression level newtype.
//!
//! Each codec clamps to its own range (LZMA 0–9, ZSTD 1–22, Brotli 0–11,
//! etc.). A codec receiving an out-of-range level returns
//! [`crate::OmnizipError::LevelOutOfRange`].

/// Compression level. Semantic newtype wrapping u8.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub struct CompressionLevel(u8);

impl CompressionLevel {
    /// Construct a level from a raw u8. The codec will validate the range
    /// at dispatch time; this constructor does not clamp.
    #[must_use]
    pub const fn new(level: u8) -> Self {
        Self(level)
    }

    /// Level 0 (fastest). Equivalent semantics to `xz -0` / `zstd -1` /
    /// `brotli -0` depending on codec.
    #[must_use]
    pub const fn fastest() -> Self {
        Self(0)
    }

    /// Level 6. A common "default" across codecs.
    #[must_use]
    pub const fn default() -> Self {
        Self(6)
    }

    /// Highest numeric level supported across the workspace (individual
    /// codecs may clamp lower — e.g., LZMA caps at 9).
    #[must_use]
    pub const fn best() -> Self {
        Self(22)
    }

    /// Raw u8 value, for serialisation at integration boundaries.
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        self.0
    }
}

impl std::fmt::Display for CompressionLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "level-{}", self.0)
    }
}

impl From<u8> for CompressionLevel {
    fn from(value: u8) -> Self {
        Self::new(value)
    }
}
