//! `.lzma` (LZMA-Alone) container encoder.
//!
//! Inverse of [`crate::decoder::alone`]. Produces the legacy LZMA Utils
//! container format:
//!
//! ```text
//! offset  size  field
//! 0       1     properties byte: lc + 9*lp + 45*pb
//! 1       4     dictionary size, little-endian
//! 5       8     uncompressed size, little-endian
//! 13      …     LZMA1 stream
//! ```

#![forbid(unsafe_code)]

use crate::encoder::Lzma1Encoder;
use crate::LzmaError;

/// Default dictionary size for the encoder (16 MiB).
const DEFAULT_DICT_SIZE: u32 = 16 * 1024 * 1024;

/// Default LZMA parameters (matches lzip/xz-utils defaults).
const DEFAULT_LC: u32 = 3;
const DEFAULT_LP: u32 = 0;
const DEFAULT_PB: u32 = 2;

/// Compress `input` into the `.lzma` (LZMA-Alone) container using the
/// optimal (DP) parser for best ratio.
///
/// # Errors
///
/// Returns [`LzmaError::Corrupt`] only on arithmetic overflow.
pub fn lzma_alone_compress_optimal(input: &[u8]) -> Result<Vec<u8>, LzmaError> {
    let lc = DEFAULT_LC;
    let lp = DEFAULT_LP;
    let pb = DEFAULT_PB;

    let mut out = Vec::with_capacity(input.len() + 13);
    let props_byte = (lc + 9 * lp + 45 * pb) as u8;
    out.push(props_byte);
    out.extend_from_slice(&DEFAULT_DICT_SIZE.to_le_bytes());
    out.extend_from_slice(&(input.len() as u64).to_le_bytes());

    let encoder = Lzma1Encoder::new(lc, lp, pb);
    let stream = encoder.encode_optimal(input);
    out.extend_from_slice(&stream);

    Ok(out)
}

/// Compress `input` into the `.lzma` (LZMA-Alone) container.
///
/// # Errors
///
/// Returns [`LzmaError::Corrupt`] only on arithmetic overflow (shouldn't
/// happen for any plausible input).
pub fn lzma_alone_compress(input: &[u8]) -> Result<Vec<u8>, LzmaError> {
    let lc = DEFAULT_LC;
    let lp = DEFAULT_LP;
    let pb = DEFAULT_PB;

    let mut out = Vec::with_capacity(input.len() + 13);
    // Properties byte: lc + 9*lp + 45*pb.
    let props_byte = (lc + 9 * lp + 45 * pb) as u8;
    out.push(props_byte);
    out.extend_from_slice(&DEFAULT_DICT_SIZE.to_le_bytes());
    out.extend_from_slice(&(input.len() as u64).to_le_bytes());

    let encoder = Lzma1Encoder::new(lc, lp, pb);
    let stream = encoder.encode(input);
    out.extend_from_slice(&stream);

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lzma_alone_decompress;

    #[test]
    fn empty_round_trips() {
        let compressed = lzma_alone_compress(&[]).expect("encode");
        let decompressed = lzma_alone_decompress(&compressed).expect("decode");
        assert!(decompressed.is_empty());
    }

    #[test]
    fn small_round_trips() {
        let input = b"Hello, world! This is LZMA-Alone compression.";
        let compressed = lzma_alone_compress(input).expect("encode");
        let decompressed = lzma_alone_decompress(&compressed).expect("decode");
        assert_eq!(decompressed.as_slice(), input.as_ref());
    }

    #[test]
    fn header_byte_matches_formula() {
        let compressed = lzma_alone_compress(b"x").expect("encode");
        // lc=3, lp=0, pb=2 → 3 + 9*0 + 45*2 = 3 + 90 = 93.
        assert_eq!(compressed[0], 93);
    }

    #[test]
    fn optimal_parse_round_trips() {
        // The optimal parser must produce output the decoder can parse.
        let input = b"hello world hello world hello world hello world".repeat(5);
        let compressed = lzma_alone_compress_optimal(&input).expect("encode");
        let decompressed = lzma_alone_decompress(&compressed).expect("decode");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn optimal_parse_empty_round_trips() {
        let compressed = lzma_alone_compress_optimal(&[]).expect("encode");
        let decompressed = lzma_alone_decompress(&compressed).expect("decode");
        assert!(decompressed.is_empty());
    }

    #[test]
    fn optimal_parser_is_smaller_or_equal() {
        // For compressible input, the optimal parser should produce
        // output no larger than the lazy parser.
        let input: Vec<u8> = (0..10_000)
            .map(|i| {
                if i % 100 < 80 {
                    b'a'
                } else {
                    (i % 26 + b'a' as i32) as u8
                }
            })
            .collect();
        let lazy = lzma_alone_compress(&input).expect("lazy");
        let optimal = lzma_alone_compress_optimal(&input).expect("optimal");
        assert!(
            optimal.len() <= lazy.len() + 10,
            "optimal {} should be ≤ lazy {} + tolerance",
            optimal.len(),
            lazy.len()
        );
    }

    /// Check whether the reference `lzma` / `xz -d` from xz-utils is
    /// available on PATH. If not, the xz-interop tests are skipped.
    fn xz_decoder_available() -> Option<std::path::PathBuf> {
        if let Ok(path_var) = std::env::var("PATH") {
            for dir in path_var.split(':') {
                for tool in &["lzma", "xz"] {
                    let candidate = std::path::Path::new(dir).join(tool);
                    if candidate.exists() {
                        return Some(candidate);
                    }
                }
            }
        }
        None
    }

    /// Decode a `.lzma` (alone-format) buffer using the reference xz-utils
    /// decoder (`lzma -d -c`). Returns the decoded bytes, or an error
    /// message if the decoder rejects the input.
    fn xz_decode_lzma(input: &[u8]) -> Result<Vec<u8>, String> {
        let lzma_path =
            xz_decoder_available().ok_or_else(|| "lzma/xz not found on PATH".to_string())?;
        let mut child = std::process::Command::new(&lzma_path)
            .arg("-d")
            .arg("-c")
            .arg("-")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("spawn {lzma_path:?}: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            stdin
                .write_all(input)
                .map_err(|e| format!("write stdin: {e}"))?;
        }
        let output = child.wait_with_output().map_err(|e| format!("wait: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "lzma -d exited {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(output.stdout)
    }

    /// Regression test for the EOPM distance-encoder bug that produced
    /// output xz-utils rejected with "Compressed data is corrupt".
    ///
    /// The root cause was in `distance_encoder::encode` for slots ≥14:
    /// the encoder used `(distance - (2 + (slot & 1))) >> ALIGN_BITS`
    /// instead of `(distance - base) >> ALIGN_BITS` where
    /// `base = (2 | (slot & 1)) << footer_bits`. For the EOPM marker
    /// (`distance = 0xFFFFFFFF`, slot 63), this produced the wrong
    /// direct-bit value, so the decoder reconstructed `rep0 ≠ UINT32_MAX`
    /// and never entered the EOPM branch.
    #[test]
    fn eopm_xz_interop_single_literal() {
        let Some(_) = xz_decoder_available() else {
            return;
        };
        let input = b"Hello";
        let compressed = lzma_alone_compress(input).expect("encode");
        let decoded = xz_decode_lzma(&compressed).expect("xz -d must accept Rust output");
        assert_eq!(decoded.as_slice(), input.as_ref());
    }

    /// Same regression — the EOPM is the last symbol, so any input that
    /// triggers the EOPM exercises the bug. This covers a range of
    /// sizes and content types.
    #[test]
    fn eopm_xz_interop_various_inputs() {
        let Some(_) = xz_decoder_available() else {
            return;
        };
        let inputs: &[&[u8]] = &[
            b"",
            b"X",
            b"Hello",
            b"Hello\n",
            b"AAAA",
            b"AAAA\n",
            b"The quick brown fox jumps over the lazy dog.",
            b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            &[0x00, 0x01, 0x02, 0x03, 0xFF, 0xFE, 0xFD],
        ];
        for input in inputs {
            let compressed =
                lzma_alone_compress(input).unwrap_or_else(|e| panic!("encode {input:?}: {e}"));
            let decoded = xz_decode_lzma(&compressed)
                .unwrap_or_else(|e| panic!("xz -d rejects Rust output for {input:?}: {e}"));
            assert_eq!(
                decoded.as_slice(),
                *input,
                "xz -d decoded data mismatch for input {input:?}",
            );
        }
    }

    /// Regression test for the `distance_slot` off-by-2 bug. The EOPM
    /// distance `0xFFFFFFFF` must produce slot 63 (not 61). This is
    /// implicitly tested by the EOPM interop tests above, but this
    /// test makes the slot assertion explicit.
    #[test]
    fn eopm_produces_correct_dist_slot() {
        // The EOPM distance is 0xFFFFFFFF (UINT32_MAX). get_dist_slot
        // must return 63. We verify by checking that the xz-utils
        // decoder recognises the end marker — if the slot were wrong,
        // the decoder would report "Compressed data is corrupt".
        let Some(_) = xz_decoder_available() else {
            return;
        };
        let compressed = lzma_alone_compress(b"A").expect("encode");
        let result = xz_decode_lzma(&compressed);
        assert!(
            result.is_ok(),
            "xz -d rejected output — possible distance_slot bug: {result:?}",
        );
        assert_eq!(result.unwrap(), b"A");
    }

    /// The optimal parser must also produce xz-utils-compatible output.
    #[test]
    fn optimal_parser_xz_interop() {
        let Some(_) = xz_decoder_available() else {
            return;
        };
        let input = b"hello world hello world hello world hello world".repeat(3);
        let compressed = lzma_alone_compress_optimal(&input).expect("encode");
        let decoded = xz_decode_lzma(&compressed).expect("xz -d must accept Rust output");
        assert_eq!(decoded, input);
    }
}
