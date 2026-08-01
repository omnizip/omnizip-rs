//! Decoder module — LZMA1 packet decode engine + container decoders.
//!
//! Phase A scope (this module):
//! - [`lzma1::Lzma1Decoder`] — the core LZMA1 packet decode loop. Ported
//!   from the decode-side of `omnizip/lib/omnizip/algorithms/lzma/xz_utils_decoder.rb`,
//!   simplified to single-stream use (no LZMA2 chunk preservation).
//! - [`alone::lzma_alone_decompress`] — the legacy `.lzma` container
//!   (13-byte header: 1 prop byte + 4 dict-size bytes + 8 uncompressed-size bytes).
//!
//! Deferred to Phase A continuation:
//! - XZ container (`decoder/xz.rs`) — block / stream / CRC32 / CRC64
//! - LZMA2 multi-chunk (`decoder/lzma2.rs`) — chunk manager + state preservation
//! - Lzip container (`decoder/lzip.rs`) — `.lz` format with trailing CRC

pub mod alone;
pub mod lzma1;

pub use alone::lzma_alone_decompress;
pub use lzma1::Lzma1Decoder;
