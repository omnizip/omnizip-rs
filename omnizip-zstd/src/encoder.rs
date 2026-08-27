//! ZSTD frame encoder — delegates to [`block::encode_frame_compressed`].
//!
//! ## Frame layout
//!
//! ```text
//! Magic_Bytes         4 bytes: 0x28 0xB5 0x2F 0xFD (LE = 0xFD2FB528)
//! Frame_Header        1-5 bytes (descriptor + optional fields)
//! Block_Header        3 bytes (LE): block_type | last_block | block_size
//! Block_Data          variable
//! [Content_Checksum]  4 bytes if descriptor.content_checksum_flag == 1
//! ```

#![forbid(unsafe_code)]

pub mod block;
pub mod cparams;
pub mod ldm;
pub mod match_finder;
pub mod opt;
pub mod sequences;

use crate::{ZstdError, ZstdLevel};

/// Compress `plaintext` into a ZSTD frame at the given level.
///
/// Uses compressed blocks (match finder + FSE-coded sequences +
/// Raw/Huffman literals). Falls back to Raw blocks when compression
/// doesn't help. The output always round-trips through any ZSTD
/// decoder.
///
/// # Errors
///
/// Returns [`ZstdError::Corrupt`] only on internal arithmetic
/// overflow (shouldn't happen for any plausible input).
pub fn encode_frame(plaintext: &[u8], level: ZstdLevel) -> Result<Vec<u8>, ZstdError> {
    block::encode_frame_compressed(plaintext, level.as_reference_level())
}

/// Compress `plaintext` into a ZSTD frame primed with a dictionary.
///
/// Delegates to [`block::encode_frame_with_dict`]. See its docs for
/// the dictionary-prefix strategy.
///
/// # Errors
///
/// Returns [`ZstdError::Corrupt`] only on internal arithmetic
/// overflow.
pub fn encode_frame_with_dict(
    plaintext: &[u8],
    level: u8,
    dict: &crate::dict::ZstdDictionary,
) -> Result<Vec<u8>, ZstdError> {
    block::encode_frame_with_dict(plaintext, level, dict)
}
