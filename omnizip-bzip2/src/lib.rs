//! omnizip-bzip2 — pure-Rust `BZip2` compression codec.
//!
//! Port of omnizip's Ruby `BZip2` implementation
//! (`omnizip/lib/omnizip/algorithms/bzip2/`). The classic 4-stage pipeline:
//!
//! 1. **RLE1** — collapse runs of 4+ identical bytes.
//! 2. **BWT** — Burrows-Wheeler Transform (block sorting).
//! 3. **MTF** — Move-to-Front transform.
//! 4. **Huffman** — canonical Huffman coding of the MTF output.
//!
//! The container format mirrors the Ruby reference: each block carries a
//! 4-byte CRC32, the BWT primary index, the original length, the RLE1 length,
//! a Huffman code-length table, and the Huffman bit stream. See
//! [`codec::Bzip2Codec`] for the [`Codec`] implementation.
//!
//! ## Determinism
//!
//! Same input + level always produces byte-identical output (no RNG, no
//! `HashSet` iteration in encode paths). Required by `LimniFS` content
//! addressing.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
// Casts in this crate are always on values bounded by block sizes (<= 900 KB)
// or alphabet sizes (<= 256). They cannot overflow in practice; the pedantic
// cast lints would just add noise.
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]

mod bwt;
mod bz2;
mod codec;
mod crc32;
mod huffman;
mod mtf;
mod rle;

pub use codec::Bzip2Codec;

// Re-export the pipeline stages so downstream crates/tests can exercise them
// in isolation (matching the Ruby class layout).
pub use bwt::{bwt_decode, bwt_encode};
pub use huffman::{huffman_decode, huffman_encode};
pub use mtf::{mtf_decode, mtf_encode};
pub use rle::{rle_decode, rle_encode};
pub use bz2::compress as compress_framed;
