//! PPMd8 (PPMdI) — PPM with RESTART restoration, glue counting, and RLE.
//!
//! Ported from the Ruby reference at
//! `omnizip/lib/omnizip/algorithms/ppmd8/`.
//!
//! See [`model`] for the prediction model and [`codec`] for the
//! `Codec` trait implementation.

#![allow(clippy::doc_markdown)]

pub mod codec;
pub mod model;

/// Container magic for PPMd8 streams: `b"PPD8\0"`.
pub const PPMD8_MAGIC: &[u8; 5] = b"PPD8\0";

/// Codec id for PPMd8.
pub const PPMD8_CODEC_ID: omnizip_codecs::CodecId = omnizip_codecs::CodecId::PPMD8;

/// Errors specific to the PPMd8 codec.
#[derive(Debug)]
pub enum Ppmd8Error {
    /// `max_order` is outside `[2, 16]`.
    InvalidOrder(u8),
    /// The compressed stream's magic prefix is wrong.
    BadMagic,
    /// The compressed stream is truncated or malformed.
    Corrupt(String),
    /// The input is too large to store in the container (u32 limit).
    TooLarge(usize),
}

impl std::fmt::Display for Ppmd8Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidOrder(o) => write!(f, "invalid max_order {o} (must be 2..=16)"),
            Self::BadMagic => write!(f, "bad magic (expected b\"PPD8\\0\")"),
            Self::Corrupt(r) => write!(f, "corrupt: {r}"),
            Self::TooLarge(n) => write!(f, "input too large: {n} bytes (max u32)"),
        }
    }
}

impl std::error::Error for Ppmd8Error {}

pub use codec::{
    compress, compress_with_budget, decompress, decompress_with_budget, Ppmd8Codec,
    DEFAULT_MEMORY_BUDGET_BYTES, DEFAULT_ORDER, MAX_ORDER, MIN_ORDER,
};
pub use model::Ppmd8Model;
// Re-export the shared binary arithmetic coder for callers building
// custom Ppmd8Model pipelines.
pub use omnizip_codecs::arith::{ArithDecoder, ArithEncoder};
