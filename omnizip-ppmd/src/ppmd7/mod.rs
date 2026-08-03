//! PPMd7 (PPMdH) — pure-Rust port of omnizip's Ruby PPMd7 reference.
//!
//! Ported from the Ruby reference at
//! `omnizip/lib/omnizip/algorithms/ppmd7/`.
//!
//! PPM (Prediction by Partial Matching) with PPM*C-style escape.
//! For each byte: walk context orders from `max_order` down to 1;
//! at each order look up the byte in the context's symbol table.
//! On hit, encode the byte's cumulative-frequency slot. On miss,
//! encode the escape symbol and drop to the next shorter order.
//! The order-(-1) fallback emits 8 equiprobable bits.
//!
//! See [`model`] for the prediction model and [`codec`] for the
//! `Codec` trait implementation.

#![allow(clippy::doc_markdown)]

pub mod codec;
pub mod context_tree;
pub mod model;

/// Container magic for PPMd7 streams: `b"PPMD\0"`.
///
/// (PPMd8 uses `b"PPD8\0"` — different magic to keep the two
/// formats distinguishable on the wire.)
pub const PPMD7_MAGIC: &[u8; 5] = b"PPMD\0";

/// Codec id for PPMd7.
pub const PPMD7_CODEC_ID: omnizip_codecs::CodecId = omnizip_codecs::CodecId::PPMD7;

/// Errors specific to the PPMd7 codec.
#[derive(Debug)]
pub enum Ppmd7Error {
    /// `max_order` is outside `[1, 16]`.
    InvalidOrder(u8),
    /// The compressed stream's magic prefix is wrong.
    BadMagic,
    /// The compressed stream is truncated or malformed.
    Corrupt(String),
    /// The input is too large to store in the container (u32 limit).
    TooLarge(usize),
}

impl std::fmt::Display for Ppmd7Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidOrder(o) => write!(f, "invalid max_order {o} (must be 1..=16)"),
            Self::BadMagic => write!(f, "bad magic (expected b\"PPMD\\0\")"),
            Self::Corrupt(r) => write!(f, "corrupt: {r}"),
            Self::TooLarge(n) => write!(f, "input too large: {n} bytes (max u32)"),
        }
    }
}

impl std::error::Error for Ppmd7Error {}

pub use codec::{
    compress, compress_default, compress_with_budget, decompress, decompress_with_budget,
    Ppmd7Codec, DEFAULT_MAX_ORDER, DEFAULT_MEMORY_BUDGET_BYTES, MAX_ORDER, MIN_ORDER,
};
pub use model::PpmModel;
// Re-export the shared binary arithmetic coder so callers that
// build their own PpmModel pipelines don't need to reach into
// omnizip_codecs::arith directly.
pub use omnizip_codecs::arith::{ArithDecoder, ArithEncoder};
