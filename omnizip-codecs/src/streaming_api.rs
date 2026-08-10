//! Streaming compression/decompression traits.
//!
//! Lets callers feed input in chunks without buffering the entire
//! input in memory. Each codec that supports streaming implements
//! [`CompressStream`] / [`DecompressStream`].
//!
//! ## Determinism
//!
//! Same input (in any chunk size) + same level → byte-identical
//! output. Chunk boundaries in the INPUT must not affect the OUTPUT.
//! Each codec's streamer achieves this by buffering internally
//! until a complete unit (LZ4 block, ZSTD block, brotli metablock)
//! is available, then emitting the unit.

use crate::error::OmnizipError;
use crate::level::CompressionLevel;

/// Streaming compressor. Feed input in chunks; call `finish` to
/// flush the trailer.
pub trait CompressStream: Send {
    /// Feed input bytes. Returns compressed output produced so far.
    /// May return empty Vec if internal state needs more input before
    /// emitting output.
    ///
    /// # Errors
    ///
    /// Returns [`OmnizipError::EncodeFailed`] on encoder failure.
    fn feed(&mut self, input: &[u8]) -> Result<Vec<u8>, OmnizipError>;

    /// Signal end of input. Returns final compressed bytes (trailer,
    /// checksums, last block, etc.).
    ///
    /// # Errors
    ///
    /// Returns [`OmnizipError::EncodeFailed`] on flush failure.
    fn finish(&mut self) -> Result<Vec<u8>, OmnizipError>;

    /// Total uncompressed bytes consumed so far.
    fn input_consumed(&self) -> u64;

    /// Total compressed bytes produced so far.
    fn output_produced(&self) -> u64;

    /// Peak memory used by this streamer (input buffer + output
    /// buffer + intermediate state).
    fn memory_usage(&self) -> usize;
}

/// Streaming decompressor.
pub trait DecompressStream: Send {
    /// Feed compressed bytes. Returns decompressed output produced
    /// so far.
    ///
    /// # Errors
    ///
    /// Returns [`OmnizipError::DecodeFailed`] on decoder failure.
    fn feed(&mut self, input: &[u8]) -> Result<Vec<u8>, OmnizipError>;

    /// Returns true once the stream is complete (footer parsed).
    fn is_finished(&self) -> bool;

    /// Total uncompressed bytes produced.
    fn output_produced(&self) -> u64;
}

/// Factory trait for creating streamers. Optional capability on top
/// of [`Codec`](crate::Codec).
pub trait Streamable: crate::Codec {
    /// Start a streaming compression session.
    ///
    /// # Errors
    ///
    /// Returns [`OmnizipError::Unsupported`] if this codec doesn't
    /// support streaming.
    fn compress_stream(
        &self,
        level: CompressionLevel,
    ) -> Result<Box<dyn CompressStream>, OmnizipError>;

    /// Start a streaming decompression session.
    ///
    /// # Errors
    ///
    /// Returns [`OmnizipError::Unsupported`] if this codec doesn't
    /// support streaming.
    fn decompress_stream(&self) -> Result<Box<dyn DecompressStream>, OmnizipError>;
}
