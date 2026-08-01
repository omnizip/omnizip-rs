//! ZSTD frame header parser — ported from
//! `omnizip/lib/omnizip/algorithms/zstandard/frame/header.rb`
//! (220 LOC, MIT, Ribose Inc.). The format is specified by RFC 8878
//! §3.1.1.1.
//!
//! ## Header layout
//!
//! ```text
//! offset  size      field                notes
//! 0       1         Frame_Header_Descriptor
//! 1       0 or 1    Window_Descriptor    omitted when single_segment = 1
//! …       0/1/2/4   Dictionary_ID        size = fcs_table[dictionary_id_flag]
//! …       0/1/2/4/8 Frame_Content_Size   size depends on fcs_flag + single_segment
//! ```
//!
//! The descriptor byte bit layout:
//!
//! ```text
//! bit  7  6  5  4  3  2  1  0
//!      └─┬─┘  │        │  └─┬─┘
//!        │   │        │    │
//!        │   │        │    └── Dictionary_ID flag (0/1/2/3)
//!        │   │        └────── Content checksum flag
//!        │   └────────────── Single segment flag
//!        └──────────────── FCS field size flag (0/1/2/3)
//! ```

#![forbid(unsafe_code)]

use crate::constants::{MAGIC_BYTES, SKIPPABLE_MAGIC_BASE, SKIPPABLE_MAGIC_MASK};
use crate::ZstdError;

/// Magic byte length for the ZSTD frame preamble.
pub const MAGIC_LEN: usize = 4;

/// Parsed ZSTD frame header. Fields that are optional per the descriptor
/// are wrapped in `Option`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameHeader {
    pub content_size_flag: u8,
    pub single_segment: bool,
    pub checksum_flag: bool,
    pub dictionary_id_flag: u8,
    pub window_log: Option<u8>,
    pub window_size: Option<u64>,
    pub dictionary_id: Option<u32>,
    pub content_size: Option<u64>,
    pub header_size: usize,
}

impl FrameHeader {
    /// Parse the header starting just past the magic number.
    /// Returns the parsed header and the slice that follows it.
    ///
    /// # Errors
    ///
    /// Returns [`ZstdError::Corrupt`] if `input` is shorter than the
    /// declared header size or if any field has an illegal value.
    pub fn parse(input: &[u8]) -> Result<(Self, &[u8]), ZstdError> {
        if input.is_empty() {
            return Err(ZstdError::Corrupt {
                reason: "frame header is empty".into(),
            });
        }
        let descriptor = input[0];
        let mut pos = 1usize;

        let content_size_flag = (descriptor >> 6) & 0x03;
        let single_segment = (descriptor & 0x20) != 0;
        let checksum_flag = (descriptor & 0x04) != 0;
        let dictionary_id_flag = descriptor & 0x03;

        let mut window_log: Option<u8> = None;
        let mut window_size: Option<u64> = None;
        if !single_segment {
            // Window descriptor present.
            if pos >= input.len() {
                return Err(ZstdError::Corrupt {
                    reason: "truncated frame header (window descriptor)".into(),
                });
            }
            let (wlog, wsize) = parse_window_descriptor(input[pos]);
            window_log = Some(wlog);
            window_size = Some(wsize);
            pos += 1;
        }

        let mut dictionary_id: Option<u32> = None;
        let did_size = dictionary_id_size(dictionary_id_flag);
        if did_size > 0 {
            if pos + did_size > input.len() {
                return Err(ZstdError::Corrupt {
                    reason: "truncated frame header (dictionary id)".into(),
                });
            }
            let raw = le_uint(&input[pos..pos + did_size]);
            dictionary_id = Some(raw as u32);
            pos += did_size;
        }

        let mut content_size: Option<u64> = None;
        let fcs_size = content_size_size(content_size_flag, single_segment);
        if fcs_size > 0 {
            if pos + fcs_size > input.len() {
                return Err(ZstdError::Corrupt {
                    reason: "truncated frame header (content size)".into(),
                });
            }
            // When FCS is 1 byte, it's stored directly; 2/4/8 bytes add
            // a 256-byte implied offset for the 1/2/4-byte variants.
            let raw = le_uint(&input[pos..pos + fcs_size]);
            let adjusted = match fcs_size {
                2 | 4 => raw.wrapping_add(256),
                _ => raw,
            };
            content_size = Some(adjusted);
            pos += fcs_size;
        }

        Ok((
            Self {
                content_size_flag,
                single_segment,
                checksum_flag,
                dictionary_id_flag,
                window_log,
                window_size,
                dictionary_id,
                content_size,
                header_size: pos,
            },
            &input[pos..],
        ))
    }

    /// Whether the frame carries a trailing 4-byte content checksum.
    #[must_use]
    pub const fn has_checksum(&self) -> bool {
        self.checksum_flag
    }
}

/// Window-descriptor decode — RFC 8878 §3.1.1.1.2.
///
/// Returns `(window_log, window_size)`. `window_log = 10 + Exponent`,
/// `window_size = (1 << window_log) + (Mantissa << (window_log - 2))`.
fn parse_window_descriptor(byte: u8) -> (u8, u64) {
    let exponent = (byte >> 3) & 0x1F;
    let mantissa = u64::from(byte & 0x07);
    let window_log = 10 + exponent;
    let window_base = 1u64 << window_log;
    // `window_base / 4 * mantissa` == `mantissa << (window_log - 2)`.
    let window_add = (window_base / 4) * mantissa;
    (window_log, window_base + window_add)
}

/// Field size of the `Dictionary_ID` for a given flag value.
const fn dictionary_id_size(flag: u8) -> usize {
    match flag {
        1 => 1,
        2 => 2,
        3 => 4,
        // 0 and any out-of-range value contribute no bytes. (The flag
        // is masked to 2 bits upstream, so the wildcard is unreachable.)
        _ => 0,
    }
}

/// Field size of `Frame_Content_Size`, depending on the FCS flag and the
/// single-segment bit (RFC 8878 §3.1.1.1.5).
const fn content_size_size(fcs_flag: u8, single_segment: bool) -> usize {
    if single_segment {
        match fcs_flag {
            0 => 1,
            1 => 2,
            2 => 4,
            3 => 8,
            _ => 0,
        }
    } else {
        match fcs_flag {
            1 => 2,
            2 => 4,
            3 => 8,
            // 0 and any out-of-range value contribute no bytes.
            _ => 0,
        }
    }
}

/// Read a little-endian unsigned integer of `bytes.len()` width. The
/// caller is responsible for ensuring `bytes.len() ≤ 8`.
fn le_uint(bytes: &[u8]) -> u64 {
    let mut v = 0u64;
    for (i, &b) in bytes.iter().enumerate() {
        v |= u64::from(b) << (8 * i);
    }
    v
}

/// Identify whether `bytes` starts with a ZSTD frame magic.
///
/// Returns `Some(true)` for a regular frame, `Some(false)` for a
/// skippable frame, or `None` if `bytes` is too short to tell.
#[must_use]
pub fn detect_frame_kind(bytes: &[u8]) -> Option<bool> {
    if bytes.len() < MAGIC_LEN {
        return None;
    }
    let magic = le_uint(&bytes[..MAGIC_LEN]) as u32;
    if magic == crate::constants::MAGIC_NUMBER {
        Some(true)
    } else if (magic & SKIPPABLE_MAGIC_MASK) == SKIPPABLE_MAGIC_BASE {
        Some(false)
    } else {
        None
    }
}

/// Strip the magic number from `bytes`, returning the slice that begins
/// at the frame header descriptor. Returns `None` if the magic doesn't
/// match a ZSTD frame.
#[must_use]
pub fn strip_magic(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.len() < MAGIC_LEN {
        return None;
    }
    if bytes[..MAGIC_LEN] != MAGIC_BYTES {
        return None;
    }
    Some(&bytes[MAGIC_LEN..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_header_errors() {
        assert!(FrameHeader::parse(&[]).is_err());
    }

    #[test]
    fn minimal_header_descriptor_zero_no_optional_fields() {
        // descriptor = 0: no FCS, no single-segment, no checksum,
        // no dictionary. Still requires a window descriptor.
        let header_bytes = [0x00u8, 0x10]; // window descriptor 0x10
        let (h, rest) = FrameHeader::parse(&header_bytes).expect("parse");
        assert_eq!(h.header_size, 2);
        assert!(!h.single_segment);
        assert!(!h.checksum_flag);
        assert_eq!(h.dictionary_id_flag, 0);
        assert_eq!(h.content_size_flag, 0);
        assert_eq!(h.window_log, Some(10 + 2)); // exponent = (0x10 >> 3) = 2
        assert!(rest.is_empty());
    }

    #[test]
    fn window_descriptor_decodes_correctly() {
        // exponent=4, mantissa=5 → window_log=14, window_base=16384,
        // window_add = 16384/4 * 5 = 4096 * 5 = 20480; size = 36864.
        let (wlog, wsize) = parse_window_descriptor((4 << 3) | 5);
        assert_eq!(wlog, 14);
        assert_eq!(wsize, 16384 + (16384 / 4) * 5);
    }

    #[test]
    fn dictionary_id_size_table() {
        assert_eq!(dictionary_id_size(0), 0);
        assert_eq!(dictionary_id_size(1), 1);
        assert_eq!(dictionary_id_size(2), 2);
        assert_eq!(dictionary_id_size(3), 4);
    }

    #[test]
    fn content_size_size_single_segment_table() {
        assert_eq!(content_size_size(0, true), 1);
        assert_eq!(content_size_size(1, true), 2);
        assert_eq!(content_size_size(2, true), 4);
        assert_eq!(content_size_size(3, true), 8);
    }

    #[test]
    fn content_size_size_multi_segment_table() {
        assert_eq!(content_size_size(0, false), 0);
        assert_eq!(content_size_size(1, false), 2);
        assert_eq!(content_size_size(2, false), 4);
        assert_eq!(content_size_size(3, false), 8);
    }

    #[test]
    fn magic_detection() {
        // Real ZSTD frame magic.
        assert_eq!(detect_frame_kind(&MAGIC_BYTES), Some(true));
        // Skippable.
        let skip: [u8; 4] = [
            SKIPPABLE_MAGIC_BASE as u8,
            (SKIPPABLE_MAGIC_BASE >> 8) as u8,
            (SKIPPABLE_MAGIC_BASE >> 16) as u8,
            (SKIPPABLE_MAGIC_BASE >> 24) as u8,
        ];
        assert_eq!(detect_frame_kind(&skip), Some(false));
        // Unknown magic.
        assert_eq!(detect_frame_kind(&[0x00, 0x00, 0x00, 0x00]), None);
        // Too short.
        assert_eq!(detect_frame_kind(&[0x28, 0xB5]), None);
    }

    #[test]
    fn strip_magic_rejects_bad_input() {
        assert!(strip_magic(&[0x00, 0x00, 0x00, 0x00]).is_none());
        assert!(strip_magic(&MAGIC_BYTES).is_some());
    }

    #[test]
    fn truncated_window_descriptor_errors() {
        // descriptor=0 requires window descriptor; we don't supply one.
        let err = FrameHeader::parse(&[0x00]).unwrap_err();
        assert!(matches!(err, ZstdError::Corrupt { .. }));
    }
}
