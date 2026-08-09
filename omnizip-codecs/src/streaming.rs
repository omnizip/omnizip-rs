//! Streaming encode/decode traits for incremental compression.
//!
//! Extends the one-shot [`Codec`] trait with incremental APIs for
//! processing data that doesn't fit in memory. OCP: existing `Codec`
//! implementations are unchanged; codecs opt into streaming by
//! implementing these traits.
//!
//! ## Design
//!
//! ```ignore
//! let mut enc = LzmaStreamingEncoder::new(level);
//! enc.write(chunk1)?;
//! enc.write(chunk2)?;
//! let compressed = enc.finish()?;
//!
//! let mut dec = LzmaStreamingDecoder::new();
//! let partial = dec.write(&compressed[..100])?;
//! let rest = dec.write(&compressed[100..])?;
//! let final_bytes = dec.finish()?;
//! ```
//!
//! ## Determinism
//!
//! Streaming encode MUST produce byte-identical output to the one-shot
//! `compress` for the same input + level (when all data is written
//! before `finish`). This is a hard requirement for `LimniFS`.

#![forbid(unsafe_code)]

use crate::OmnizipError;

/// Incremental encoder. Write data in chunks, then call [`finish`](Self::finish)
/// to get the complete compressed output.
pub trait StreamingEncoder {
    /// Write a chunk of plaintext. May buffer internally.
    ///
    /// # Errors
    ///
    /// Returns an error on encode failure.
    fn write(&mut self, input: &[u8]) -> Result<(), OmnizipError>;

    /// Finish encoding and return the complete compressed output.
    ///
    /// # Errors
    ///
    /// Returns an error on encode failure or if no data was written.
    fn finish(self) -> Result<Vec<u8>, OmnizipError>;
}

/// Incremental decoder. Write compressed data in chunks; each call may
/// return zero or more decoded plaintext bytes.
pub trait StreamingDecoder {
    /// Write a chunk of compressed data. Returns any plaintext that
    /// could be fully decoded from the data received so far.
    ///
    /// # Errors
    ///
    /// Returns an error on decode failure or corruption.
    fn write(&mut self, input: &[u8]) -> Result<Vec<u8>, OmnizipError>;

    /// Finish decoding and return any remaining plaintext.
    ///
    /// # Errors
    ///
    /// Returns an error if the stream is truncated or corrupt.
    fn finish(self) -> Result<Vec<u8>, OmnizipError>;
}
