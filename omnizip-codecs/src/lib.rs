//! omnizip-codecs — shared `Codec` trait + `CodecRegistry`.
//!
//! Every codec crate in the omnizip-rs workspace implements [`Codec`] and
//! registers it with a [`CodecRegistry`]. consumers (`LimniFS`, others)
//! either use [`CodecRegistry::default_pure_rust`] or construct a custom
//! registry with a subset of codecs.
//!
//! Adding a codec = one new file + one `register()` call. Dispatch code
//! never changes. This is the open/closed principle applied to codecs.
//!
//! ## Determinism
//!
//! Every codec MUST be deterministic: same input + same level ⇒ byte-
//! identical output across runs, machines, and Rust versions. A codec
//! that produces non-deterministic output breaks content-addressed
//! storage (e.g., `LimniFS`'s `DropId = BLAKE3(plaintext)`).

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

pub mod arith;
pub mod bitstream;
pub mod checksum;
mod codec;
mod error;
pub mod hash;
mod level;
pub mod matchfinder;
mod registry;
pub mod xxhash;

pub use arith::{scaled_prob, ArithDecoder, ArithEncoder, PROB_SCALE as ARITH_PROB_SCALE};
pub use bitstream::{BitReaderBE, BitReaderLE, BitWriterBE, BitWriterLE};
pub use checksum::{crc32_iso_hdlc, crc32_iso_hdlc_raw, crc32_iso_hdlc_update};
pub use codec::{Codec, CodecId};
pub use error::OmnizipError;
pub use hash::{djb2_32, djb2_32_tagged, fnv1a_32, fnv1a_32_tagged};
pub use level::CompressionLevel;
pub use matchfinder::{HashChainConfig, HashChainMatchFinder, Lz77Match};
pub use registry::CodecRegistry;
