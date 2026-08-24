//! BZIP2 single-file format — port of `omnizip/formats/bzip2_file.rb`.
//! The bzip2 stream IS the container ("BZh" magic + stream); this
//! module adds multi-stream concatenation and the CRC bookkeeping the
//! codec already verifies.
#![forbid(unsafe_code)]

use crate::error::ArchiveError;

/// Compress to a single bzip2 stream at the given level (block size
/// 100k..900k).
///
/// # Errors
///
/// [`ArchiveError::InvalidArchive`] on codec failure.
pub fn compress(input: &[u8], level: u8) -> Result<Vec<u8>, ArchiveError> {
    omnizip_bzip2::compress_framed(input, level.clamp(1, 9))
        .map_err(|e| ArchiveError::InvalidArchive(format!("bzip2: {e}")))
}

/// Decompress a possibly multi-stream `.bz2` file (concatenated
/// streams are valid per the format and decoded in order).
///
/// # Errors
///
/// [`ArchiveError::InvalidArchive`] on malformed input.
pub fn decompress(input: &[u8]) -> Result<Vec<u8>, ArchiveError> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while cursor < input.len() {
        // Locate the end of this stream: find the next "BZh" that
        // starts a following member, else consume the rest.
        let rest = &input[cursor..];
        let next_member = if rest.len() > 4 {
            rest[4..]
                .windows(3)
                .position(|w| w == b"BZh")
                .map(|p| p + 4)
        } else {
            None
        };
        let member_end = next_member.unwrap_or(rest.len());
        let member = &rest[..member_end];
        if member.is_empty() {
            break;
        }
        let data = omnizip_bzip2::decompress_framed(member)
            .map_err(|e| ArchiveError::InvalidArchive(format!("bzip2: {e}")))?;
        out.extend_from_slice(&data);
        cursor += member_end;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let data = b"the quick brown fox jumps over the lazy dog".repeat(20);
        let bz = compress(&data, 9).unwrap();
        assert!(bz.starts_with(b"BZh9"));
        let back = decompress(&bz).unwrap();
        assert_eq!(back, data);
    }

    #[test]
    fn multi_stream() {
        let a = compress(b"alpha ", 1).unwrap();
        let b = compress(b"beta", 1).unwrap();
        let mut both = a;
        both.extend_from_slice(&b);
        assert_eq!(decompress(&both).unwrap(), b"alpha beta");
    }
}
