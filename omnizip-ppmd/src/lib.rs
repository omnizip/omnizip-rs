//! omnizip-ppmd — Pure-Rust PPMd (Prediction by Partial Matching) codecs.
//!
//! Two PPMd variants live in this crate, kept strictly separate so
//! users never confuse them:
//!
//! - **[`ppmd7`]** — PPMd7 (PPMdH). Codec id [`CodecId::PPMD7`] = `0x0008`.
//!   Container magic `b"PPMD\0"`. Default order 4. Implemented by
//!   [`Ppmd7Codec`].
//! - **[`ppmd8`]** — PPMd8 (PPMdI). Codec id [`CodecId::PPMD8`] = `0x0009`.
//!   Container magic `b"PPD8\0"`. Default order 6. Implemented by
//!   [`Ppmd8Codec`].
//!
//! Both codecs share the same ZPAQ-style binary arithmetic coder, but
//! the prediction models differ:
//!
//! - PPMd7 uses byte-level PPM with PPM*C-style escape and a flat
//!   hash-table of contexts.
//! - PPMd8 adds RESTART/CUT_OFF restoration methods, glue counting
//!   for context pruning, and run-length encoding support.
//!
//! ## Quick start
//!
//! ```no_run
//! use omnizip_ppmd::{Ppmd7Codec, Ppmd8Codec};
//! use omnizip_codecs::{Codec, CompressionLevel};
//!
//! let input = b"the quick brown fox jumps over the lazy dog".repeat(100);
//!
//! // PPMd7 (default codec exported from crate root)
//! let codec7 = Ppmd7Codec::new();
//! let c7 = codec7.compress(&input, CompressionLevel::default()).unwrap();
//! let d7 = codec7.decompress(&c7, input.len() as u32).unwrap();
//! assert_eq!(d7, input);
//!
//! // PPMd8
//! let codec8 = Ppmd8Codec::new();
//! let c8 = codec8.compress(&input, CompressionLevel::default()).unwrap();
//! let d8 = codec8.decompress(&c8, input.len() as u32).unwrap();
//! assert_eq!(d8, input);
//! ```
//!
//! ## Determinism
//!
//! Both codecs are deterministic: same input + same `max_order` ⇒
//! byte-identical output, across runs, machines, and Rust versions.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

pub mod ppmd7;
pub mod ppmd8;

// Convenience re-exports at crate root — both codecs equally
// discoverable, with the PPMd version baked into the name to avoid
// any ambiguity.
pub use ppmd7::{Ppmd7Codec, PPMD7_CODEC_ID, PPMD7_MAGIC};
pub use ppmd8::{Ppmd8Codec, PPMD8_CODEC_ID, PPMD8_MAGIC};
