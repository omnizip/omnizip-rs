//! Standard bzip2 wire format (`BZh` magic) — independent of the
//! Ruby-compatible custom format used by [`crate::codec::Bzip2Codec`].
//!
//! Output produced here is decodable by `bzip2 -d`. The pipeline is
//! identical (RLE1 → BWT → MTF → RLE2 → Huffman) but the bit-level
//! serialization matches the upstream bzip2 spec.
//!
//! See <https://sourceware.org/bzip2/docs.html> for the format.

#![forbid(unsafe_code)]

pub mod bitwriter;
pub mod crc32;
pub mod decompress;
pub mod huffman;
pub mod mtf;
pub mod rle2;

use bitwriter::Bz2BitWriter;
use huffman::{canonical_codes, code_lengths};
use mtf::{build_seed, mtf_encode_with_seed};

use crate::bwt::bwt_encode;
use crate::rle::rle_encode;

/// Block header magic: BCD of π (first 6 digits each side).
const BLOCK_MAGIC: u64 = 0x3141_5926_5359;
/// End-of-stream magic: BCD of √π.
const EOS_MAGIC: u64 = 0x1772_4538_5090;
/// Always 0 — modern bzip2 doesn't randomise blocks.
const RANDOMISED_FLAG: bool = false;
/// Number of Huffman tables. bzip2's range is 2..=6. We use 2 (the
/// minimum) and write the same table twice.
const N_GROUPS: u8 = 2;
/// Symbols per selector chunk (~50 in upstream bzip2).
const GROUP_SIZE: usize = 50;

/// Compress `input` into a standard `.bz2` stream compatible with
/// `bzip2 -d`. `level` selects the block size `100_000..=900_000` in
/// 100 KB steps.
///
/// # Errors
///
/// Returns [`OmnizipError::LevelOutOfRange`] if level is not 1..=9.
pub fn compress(input: &[u8], level: u8) -> Result<Vec<u8>, OmnizipError> {
    if !(1..=9).contains(&level) {
        return Err(OmnizipError::LevelOutOfRange {
            codec: CodecId::BZIP2,
            level,
            min: 1,
            max: 9,
        });
    }
    let block_size = usize::from(level) * 100_000;

    let mut writer = Bz2BitWriter::new();
    // Stream header: "BZh" + level digit (ASCII '1'..'9').
    writer.write_bits(u32::from(b'B'), 8);
    writer.write_bits(u32::from(b'Z'), 8);
    writer.write_bits(u32::from(b'h'), 8);
    let level_digit = b'0' + level;
    writer.write_bits(u32::from(level_digit), 8);

    if input.is_empty() {
        // Per bzip2: even an empty stream must end with the EOS magic.
        // No blocks, no CRC.
        writer.write48(EOS_MAGIC);
        return Ok(writer.finish());
    }

    // bzip2 combines block CRCs into a running 32-bit checksum:
    // combined = rotate_left(combined, 1) XOR block_crc, starting at 0.
    let mut combined: u32 = 0;
    for chunk in input.chunks(block_size) {
        let block_crc = crc32::crc32(chunk);
        combined = combined.rotate_left(1) ^ block_crc;
        encode_block(chunk, &mut writer);
    }
    // Write the EOS magic, then the 32-bit combined CRC.
    writer.write48(EOS_MAGIC);
    writer.write_bits(combined, 32);

    Ok(writer.finish())
}

fn encode_block(block: &[u8], writer: &mut Bz2BitWriter) {
    // Pipeline: RLE1 → BWT → MTF (seeded with active bytes) → RLE2 → Huffman.
    let rle1 = rle_encode(block);
    let (bwt, primary_index) = bwt_encode(&rle1);
    let seed = build_seed(&bwt);
    let n_in_use = seed.len();
    let mtf = mtf_encode_with_seed(&bwt, &seed);
    let symbols = rle2::mtf_to_symbols(&mtf, n_in_use);

    // Write block header.
    writer.write48(BLOCK_MAGIC);
    writer.write_bits(crc32::crc32(block), 32);
    writer.write_bit(RANDOMISED_FLAG);
    // origPtr is 24 bits — must fit since block_size ≤ 900_000.
    writer.write_bits(primary_index & 0xFF_FFFF, 24);

    // Symbol usage map. bzip2 writes one bit per group (group 0 first,
    // group 15 last), then for each USED group, one bit per byte (byte
    // 0 first, byte 15 last). Equivalent to writing a 16-bit "groups
    // used" value with group i in bit (15 - i) MSB-first.
    let (groups_used, group_maps) = build_symbol_map(&bwt);
    writer.write_bits(groups_used, 16);
    for g in &group_maps {
        writer.write_bits(u32::from(*g), 16);
    }

    // nGroups (3 bits) + nSelectors (15 bits).
    let n_symbols = symbols.len();
    let n_selectors = n_symbols.div_ceil(GROUP_SIZE).max(1).min(18_002);
    writer.write_bits(u32::from(N_GROUPS), 3);
    writer.write_bits(n_selectors as u32, 15);
    // Selectors are MTF-coded. MTF value N emits N '1' bits then a '0'.
    // All our selectors = 0 (always use table 0) → single '0' bit each.
    for _ in 0..n_selectors {
        writer.write_bit(false);
    }

    // Build Huffman code lengths over the alphabet of size n_in_use + 2.
    let alphabet_size = n_in_use + 2;
    let mut freqs = vec![0u32; alphabet_size];
    for &sym in &symbols {
        freqs[usize::from(sym)] += 1;
    }
    let lengths = code_lengths(&freqs);
    let codes = canonical_codes(&lengths);

    // Write the Huffman table N_GROUPS times (identical tables).
    for _ in 0..N_GROUPS {
        write_huffman_table(writer, &lengths);
    }

    // Encode each chunk using table 0's codes.
    for chunk in symbols.chunks(GROUP_SIZE) {
        for &sym in chunk {
            let (code, len) = codes[usize::from(sym)];
            writer.write_bits(code, u32::from(len));
        }
    }
}

/// Write a Huffman code-length table using bzip2's delta encoding.
///
/// The starting length (5 bits) is the initial value of `uc`. Then
/// for each symbol in `0..lengths.len()`, emit +1 ('10') or -1 ('11')
/// adjustments until `uc` matches the symbol's length, then a '0'
/// terminator.
///
/// bzip2 requires all symbols in the alphabet to receive a code, so
/// `lengths` should have no zeros (handled by [`code_lengths`]).
fn write_huffman_table(writer: &mut Bz2BitWriter, lengths: &[u8]) {
    // 5-bit starting length = first symbol's length.
    writer.write_bits(u32::from(lengths[0]), 5);
    let mut current: i32 = i32::from(lengths[0]);

    // For each symbol in 0..alphaSize: emit adjustments + '0' terminator.
    // Symbol 0 has no adjustments (current already matches), just emits '0'.
    for &target in lengths {
        let mut diff = i32::from(target) - current;
        while diff != 0 {
            writer.write_bit(true);
            if diff > 0 {
                writer.write_bit(false); // '10' → +1
                diff -= 1;
                current += 1;
            } else {
                writer.write_bit(true); // '11' → -1
                diff += 1;
                current -= 1;
            }
        }
        writer.write_bit(false); // '0' → terminator for this symbol
    }
}

/// Build the bzip2 symbol usage map: a 16-bit "groups used" word
/// (group i in bit `15 - i` so MSB-first encoding matches bzip2's
/// group-0-first bit order), and one 16-bit detail word per used
/// group (byte j in bit `15 - j` for the same reason).
fn build_symbol_map(data: &[u8]) -> (u32, Vec<u16>) {
    let mut used = [false; 256];
    for &b in data {
        used[b as usize] = true;
    }

    let mut groups_used: u32 = 0;
    let mut detail: Vec<u16> = Vec::new();
    for group_idx in 0..16 {
        let mut group_bits: u16 = 0;
        let mut any = false;
        for j in 0..16 {
            let byte_idx = group_idx * 16 + j;
            if used[byte_idx] {
                // Bit (15 - j) so MSB-first encoding writes byte 0 first.
                group_bits |= 1 << (15 - j);
                any = true;
            }
        }
        if any {
            groups_used |= 1 << (15 - group_idx);
            detail.push(group_bits);
        }
    }
    (groups_used, detail)
}

use omnizip_codecs::{CodecId, OmnizipError};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_out_of_range_levels() {
        assert!(compress(b"x", 0).is_err());
        assert!(compress(b"x", 10).is_err());
    }

    #[test]
    fn empty_input_produces_valid_header_and_eos() {
        let out = compress(b"", 9).unwrap();
        // BZh9 header.
        assert_eq!(&out[..3], b"BZh");
        assert_eq!(out[3], b'9');
        // EOS magic (6 bytes) follows immediately.
        // No CRC since there are no blocks.
        assert!(out.len() >= 3 + 1 + 6);
    }

    #[test]
    fn small_input_starts_with_bzh_magic_and_block_magic() {
        let out = compress(b"banana", 9).unwrap();
        assert_eq!(&out[..4], b"BZh9");
        // The next 6 bytes are the block magic 0x314159265359, possibly
        // aligned at a bit offset depending on the bit writer. Since
        // we just wrote 32 bits of header (byte-aligned), the block
        // magic starts at byte 4.
        assert_eq!(&out[4..10], &[0x31, 0x41, 0x59, 0x26, 0x53, 0x59]);
    }

    #[test]
    fn symbol_map_for_single_byte_value() {
        let data = vec![0x42u8; 10];
        let (groups, detail) = build_symbol_map(&data);
        // Group 4 (bytes 0x40-0x4F) contains 0x42 = bit 2 in the group.
        // With the MSB-first encoding, byte 2 of group sets bit (15-2)=13.
        assert_eq!(groups, 1 << (15 - 4)); // group 4 → bit 11
        assert_eq!(detail, vec![1 << (15 - 2)]); // byte 2 in group → bit 13
    }
}

#[cfg(test)]
mod cli_tests {
    use super::*;
    use std::io::Write;
    use std::process::{Command, Stdio};

    fn bzip2_decode(compressed: &[u8]) -> std::io::Result<Vec<u8>> {
        let mut child = Command::new("bzip2")
            .arg("-dc")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        if let Some(stdin) = child.stdin.as_mut() {
            stdin.write_all(compressed)?;
        }
        let output = child.wait_with_output()?;
        if !output.status.success() {
            return Err(std::io::Error::other(format!(
                "bzip2 failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(output.stdout)
    }

    #[test]
    fn small_input_decodes_via_bzip2_cli() {
        let input = b"banana banana banana banana banana";
        let compressed = compress(input, 9).unwrap();
        eprintln!(
            "compressed {} bytes -> {} bytes",
            input.len(),
            compressed.len()
        );
        eprintln!("hex: {:02x?}", compressed);
        let decoded = bzip2_decode(&compressed).unwrap();
        assert_eq!(decoded, input);
    }
}

#[cfg(test)]
mod detailed_tests {
    use super::*;
    use std::io::Write;
    use std::process::{Command, Stdio};

    #[test]
    fn dump_with_tvv() {
        let input = b"banana banana banana banana banana";
        let compressed = compress(input, 9).unwrap();
        eprintln!("compressed hex: {:02x?}", compressed);
        let mut child = Command::new("bzip2")
            .args(["-tvv"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(stdin) = child.stdin.as_mut() {
            stdin.write_all(&compressed).unwrap();
        }
        let output = child.wait_with_output().unwrap();
        eprintln!("stderr: {}", String::from_utf8_lossy(&output.stderr));
        eprintln!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    }
}

#[cfg(test)]
mod full_parity {
    use super::*;
    use std::io::Write;
    use std::process::{Command, Stdio};

    fn bzip2_decode(compressed: &[u8]) -> Vec<u8> {
        let mut child = Command::new("bzip2")
            .arg("-dc")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(stdin) = child.stdin.as_mut() {
            stdin.write_all(compressed).unwrap();
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "bzip2 -dc failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    #[test]
    fn banana_decodes() {
        let input = b"banana banana banana banana banana";
        let compressed = compress(input, 9).unwrap();
        assert_eq!(bzip2_decode(&compressed), input);
    }

    #[test]
    fn pangrams_decodes() {
        let input = b"The quick brown fox jumps over the lazy dog. Pack my box with five dozen liquor jugs.";
        let compressed = compress(input, 9).unwrap();
        assert_eq!(bzip2_decode(&compressed), input);
    }

    #[test]
    fn single_byte_decodes() {
        let input = b"a";
        let compressed = compress(input, 9).unwrap();
        assert_eq!(bzip2_decode(&compressed), input);
    }

    #[test]
    fn long_run_decodes() {
        let input = vec![b'a'; 50];
        let compressed = compress(&input, 9).unwrap();
        assert_eq!(bzip2_decode(&compressed), input);
    }

    #[test]
    fn multi_block_input_decodes() {
        // 200 KB input forces 3 blocks at level 1 (100 KB blocks).
        let input: Vec<u8> = (0..200_000).map(|i| (i % 251) as u8).collect();
        let compressed = compress(&input, 1).unwrap();
        assert_eq!(bzip2_decode(&compressed), input);
    }
}
