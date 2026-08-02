//! Top-level ZSTD decoder — drives frame → blocks → output.
//!
//! Ported from `omnizip/lib/omnizip/algorithms/zstandard/decoder.rb`
//! (225 LOC, MIT, Ribose Inc.).
//!
//! ## Pipeline
//!
//! ```text
//! for each frame:
//!   parse frame header
//!   reset per-frame state (repeat offsets, previous Huffman/FSE tables)
//!   for each block:
//!     parse block header
//!     dispatch on block type:
//!       Raw  → copy `block_size` bytes verbatim
//!       RLE  → expand one byte to `block_size` copies
//!       Compressed → decode literals + sequences + execute
//!     break on last_block
//!   verify optional content checksum (XXHash64 truncated to u32)
//! ```
//!
//! ## Statefulness
//!
//! `ZstdDecoder` holds per-frame state that survives across blocks:
//!
//! - The most recent Huffman table (for `Treeless` literals).
//! - The most recent FSE tables (for `MODE_REPEAT` sequence streams).
//! - The three repeat-offset slots.
//!
//! The state is reset at the start of every frame.

#![forbid(unsafe_code)]

use crate::constants::{MAGIC_NUMBER, SKIPPABLE_MAGIC_BASE, SKIPPABLE_MAGIC_MASK};
use crate::frame::{BlockHeader, FrameHeader};
use crate::huffman::HuffmanTable;
use crate::literals::decode_literals_section;
use crate::sequences::{decode_sequences_section, SequenceExecutor};
use crate::ZstdError;

/// Pure-Rust ZSTD decoder. Construct once, call [`Self::decode_stream`]
/// per input. State is reset between calls.
#[derive(Debug, Default)]
pub struct ZstdDecoder {
    previous_huffman_table: Option<HuffmanTable>,
    previous_fse_tables: (),
    executor: SequenceExecutor,
}

impl ZstdDecoder {
    /// Construct a fresh decoder with default per-frame state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Decode a complete ZSTD stream (one or more concatenated frames).
    ///
    /// # Errors
    ///
    /// Returns [`ZstdError::Corrupt`] on structural problems and
    /// [`ZstdError::Unsupported`] on not-yet-implemented features
    /// (Huffman FSE-compressed weights, `MODE_FSE` sequence tables,
    /// real `XXHash32` verification).
    pub fn decode_stream(&mut self, input: &[u8]) -> Result<Vec<u8>, ZstdError> {
        self.decode_stream_with_prefix(input, &[])
    }

    /// Decode a ZSTD stream with a dictionary prefix priming the
    /// output window. Each frame's sequences may back-reference
    /// positions in `prefix`. The returned bytes exclude the prefix.
    ///
    /// Used by [`crate::decompress_with_dict`].
    ///
    /// # Errors
    ///
    /// See [`Self::decode_stream`].
    pub fn decode_stream_with_prefix(
        &mut self,
        input: &[u8],
        prefix: &[u8],
    ) -> Result<Vec<u8>, ZstdError> {
        let mut output = Vec::new();
        let mut remaining = input;

        loop {
            if remaining.len() < 4 {
                if remaining.is_empty() {
                    break;
                }
                return Err(ZstdError::Corrupt {
                    reason: format!(
                        "trailing {} bytes are not a complete magic",
                        remaining.len()
                    ),
                });
            }
            let magic =
                u32::from_le_bytes([remaining[0], remaining[1], remaining[2], remaining[3]]);

            if (magic & SKIPPABLE_MAGIC_MASK) == SKIPPABLE_MAGIC_BASE {
                remaining = Self::skip_skippable_frame(remaining)?;
                continue;
            }
            if magic != MAGIC_NUMBER {
                return Err(ZstdError::Corrupt {
                    reason: format!("invalid ZSTD magic: 0x{magic:08X}"),
                });
            }
            let after_magic = &remaining[4..];

            // Borrow `self` mutably for the duration of one frame, then
            // release before the next loop iteration rebinds `remaining`.
            let (frame_output, rest) = self.decode_frame_with_prefix(after_magic, prefix)?;
            output.extend_from_slice(&frame_output);
            remaining = rest;
        }
        Ok(output)
    }

    /// Skip a skippable frame — 4 bytes magic + 4 bytes size + N bytes.
    fn skip_skippable_frame(input: &[u8]) -> Result<&[u8], ZstdError> {
        if input.len() < 8 {
            return Err(ZstdError::Corrupt {
                reason: "truncated skippable frame".into(),
            });
        }
        let size = u32::from_le_bytes([input[4], input[5], input[6], input[7]]) as usize;
        let end = 8usize
            .checked_add(size)
            .ok_or_else(|| ZstdError::Corrupt {
                reason: format!("skippable frame size {size} overflows usize"),
            })?;
        if input.len() < end {
            return Err(ZstdError::Corrupt {
                reason: "truncated skippable frame body".into(),
            });
        }
        Ok(&input[end..])
    }

    fn decode_frame_with_prefix<'a>(
        &mut self,
        input: &'a [u8],
        prefix: &[u8],
    ) -> Result<(Vec<u8>, &'a [u8]), ZstdError> {
        // Reset per-frame state.
        self.previous_huffman_table = None;
        self.previous_fse_tables = ();
        self.executor = SequenceExecutor::new();

        let (header, after_header) = FrameHeader::parse(input)?;
        // Prime the output window with the dictionary prefix. Sequences
        // may back-reference positions in [0, prefix.len()). The prefix
        // is stripped from the returned value and excluded from the
        // checksum.
        let prefix_len = prefix.len();
        let mut output: Vec<u8> = prefix.to_vec();
        let mut remaining = after_header;

        loop {
            let (block, after_block) = BlockHeader::parse(remaining)?;
            if block.is_reserved() {
                return Err(ZstdError::Corrupt {
                    reason: format!("reserved block type (raw=0x{:06X})", block.raw),
                });
            }

            let after_block_data =
                self.decode_block(block, after_block, &mut output)?;
            remaining = after_block_data;
            if block.last_block {
                break;
            }
        }

        if header.has_checksum() {
            if remaining.len() < 4 {
                return Err(ZstdError::Corrupt {
                    reason: "truncated frame checksum".into(),
                });
            }
            // ZSTD frame checksum: XXH64 of decoded output, truncated to 32 bits.
            // The checksum covers only the plaintext (after the prefix).
            let expected = u32::from_le_bytes([
                remaining[0],
                remaining[1],
                remaining[2],
                remaining[3],
            ]);
            let plaintext = &output[prefix_len..];
            let actual = crate::xxhash::zstd_frame_checksum(plaintext);
            if expected != actual {
                return Err(ZstdError::Corrupt {
                    reason: format!(
                        "frame checksum mismatch: stored {expected:#010X}, computed {actual:#010X}"
                    ),
                });
            }
            remaining = &remaining[4..];
        }

        // Strip the dictionary prefix from the returned output.
        Ok((output[prefix_len..].to_vec(), remaining))
    }

    fn decode_block<'a>(
        &mut self,
        block: BlockHeader,
        input: &'a [u8],
        output: &mut Vec<u8>,
    ) -> Result<&'a [u8], ZstdError> {
        if block.is_raw() {
            let size = block.block_size as usize;
            if input.len() < size {
                return Err(ZstdError::Corrupt {
                    reason: format!(
                        "truncated raw block: need {size} bytes, got {}",
                        input.len()
                    ),
                });
            }
            output.extend_from_slice(&input[..size]);
            Ok(&input[size..])
        } else if block.is_rle() {
            if input.is_empty() {
                return Err(ZstdError::Corrupt {
                    reason: "truncated RLE block: missing repeated byte".into(),
                });
            }
            let byte = input[0];
            output.extend(std::iter::repeat(byte).take(block.block_size as usize));
            Ok(&input[1..])
        } else if block.is_compressed() {
            self.decode_compressed_block(block, input, output)
        } else {
            Err(ZstdError::Corrupt {
                reason: format!("reserved block type {}", block.block_type),
            })
        }
    }

    fn decode_compressed_block<'a>(
        &mut self,
        block: BlockHeader,
        input: &'a [u8],
        output: &mut Vec<u8>,
    ) -> Result<&'a [u8], ZstdError> {
        let block_end = block.block_size as usize;
        if input.len() < block_end {
            return Err(ZstdError::Corrupt {
                reason: format!(
                    "truncated compressed block: header says {}, got {}",
                    block_end,
                    input.len()
                ),
            });
        }
        let block_input = &input[..block_end];
        let after_block = &input[block_end..];

        // 1. Literals section.
        let lit_section = decode_literals_section(block_input, self.previous_huffman_table.as_ref())?;
        if lit_section.huffman_table.is_some() {
            self.previous_huffman_table = lit_section.huffman_table;
        }
        let literals = lit_section.literals;
        let after_literals = &block_input[lit_section.consumed..];

        // 2. Sequences section. Pass the executor so offset resolution
        //    can update repeat-offset slots inline during decode.
        let seq_section = decode_sequences_section(
            after_literals,
            &self.previous_fse_tables,
            &mut self.executor,
        )?;
        let () = seq_section.fse_tables;

        // 3. Execute sequences against literals, appending to the
        //    frame-level output buffer (which may already contain the
        //    dictionary prefix). This lets back-references into the
        //    prefix resolve correctly.
        self.executor.execute(&literals, &seq_section.sequences, output)?;

        Ok(after_block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_magic() {
        let mut d = ZstdDecoder::new();
        let err = d.decode_stream(&[0x00, 0x00, 0x00, 0x00]).unwrap_err();
        assert!(matches!(err, ZstdError::Corrupt { .. }));
    }

    #[test]
    fn empty_input_yields_empty_output() {
        let mut d = ZstdDecoder::new();
        assert_eq!(d.decode_stream(&[]).expect("empty"), b"");
    }

    #[test]
    fn skippable_frame_is_skipped() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0x50, 0x2A, 0x4D, 0x18]); // skippable magic
        bytes.extend_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);
        let mut d = ZstdDecoder::new();
        let out = d.decode_stream(&bytes).expect("skippable");
        assert!(out.is_empty());
    }

    #[test]
    fn raw_block_decodes_byte_for_byte() {
        let payload = b"hello";
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0x28, 0xB5, 0x2F, 0xFD]); // magic
        bytes.push(0x00); // descriptor
        bytes.push(0x10); // window descriptor
        let raw_hdr = 1u32 | (5u32 << 3); // last=1, type=0, size=5
        bytes.push((raw_hdr & 0xFF) as u8);
        bytes.push(((raw_hdr >> 8) & 0xFF) as u8);
        bytes.push(((raw_hdr >> 16) & 0xFF) as u8);
        bytes.extend_from_slice(payload);
        let mut d = ZstdDecoder::new();
        let out = d.decode_stream(&bytes).expect("decode");
        assert_eq!(out, payload);
    }

    #[test]
    fn rle_block_expands_to_block_size() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0x28, 0xB5, 0x2F, 0xFD]); // magic
        bytes.push(0x00);
        bytes.push(0x10);
        let raw_hdr = 1u32 | (1u32 << 1) | (4u32 << 3); // last=1, type=1, size=4
        bytes.push((raw_hdr & 0xFF) as u8);
        bytes.push(((raw_hdr >> 8) & 0xFF) as u8);
        bytes.push(((raw_hdr >> 16) & 0xFF) as u8);
        bytes.push(b'X');
        let mut d = ZstdDecoder::new();
        let out = d.decode_stream(&bytes).expect("decode");
        assert_eq!(out, b"XXXX");
    }

    #[test]
    fn per_frame_state_resets_between_calls() {
        // Decode two frames back-to-back. The second frame's repeat
        // offsets must start from defaults, not whatever the first
        // frame left.
        let mut bytes = Vec::new();
        // Frame 1: RLE block of 4 'A's, last block, no checksum.
        bytes.extend_from_slice(&[0x28, 0xB5, 0x2F, 0xFD]); // magic
        bytes.push(0x00);
        bytes.push(0x10);
        let raw1 = 1u32 | (1u32 << 1) | (4u32 << 3);
        bytes.push((raw1 & 0xFF) as u8);
        bytes.push(((raw1 >> 8) & 0xFF) as u8);
        bytes.push(((raw1 >> 16) & 0xFF) as u8);
        bytes.push(b'A');
        // Frame 2: RLE block of 3 'B's, last block.
        bytes.extend_from_slice(&[0x28, 0xB5, 0x2F, 0xFD]);
        bytes.push(0x00);
        bytes.push(0x10);
        let raw2 = 1u32 | (1u32 << 1) | (3u32 << 3);
        bytes.push((raw2 & 0xFF) as u8);
        bytes.push(((raw2 >> 8) & 0xFF) as u8);
        bytes.push(((raw2 >> 16) & 0xFF) as u8);
        bytes.push(b'B');
        let mut d = ZstdDecoder::new();
        let out = d.decode_stream(&bytes).expect("decode");
        assert_eq!(out, b"AAAABBB");
    }
}
