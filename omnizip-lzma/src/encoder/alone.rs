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

/// Maximum legal LZMA literal-context bits.
pub const MAX_LC: u32 = 8;
/// Maximum legal LZMA literal-position bits.
pub const MAX_LP: u32 = 4;
/// Maximum legal LZMA position bits.
pub const MAX_PB: u32 = 4;
/// Hard limit: `lc + lp <= 4` per the LZMA spec.
pub const LC_LP_SUM_MAX: u32 = 4;
/// Minimum dictionary size (matches xz-utils floor).
pub const MIN_DICT_SIZE: u32 = 4096;
/// Maximum dictionary size for the `.lzma` container (u32 max).
pub const MAX_DICT_SIZE: u32 = u32::MAX;

/// User-tunable LZMA encoder parameters.
///
/// These map to the standard `xz` / `lzma` CLI flags. The wire format
/// stores them in a 1-byte properties field (`lc + 9*lp + 45*pb`).
///
/// ```rust
/// use omnizip_lzma::LzmaOptions;
/// use omnizip_lzma::encoder::lzma_alone_compress_with_options;
///
/// let opts = LzmaOptions {
///     lc: 3,                      // literal-context bits (default 3)
///     lp: 0,                      // literal-position bits (default 0)
///     pb: 2,                      // position bits (default 2)
///     dict_size: 16 * 1024 * 1024, // 16 MB (default)
///     use_optimal_parser: true,  // slower but better ratio
///     ..LzmaOptions::default()
/// };
/// let bytes = lzma_alone_compress_with_options(b"input", &opts).unwrap();
/// ```
#[derive(Debug, Clone, Copy)]
pub struct LzmaOptions {
    /// Literal-context bits (0..=8). Higher = more context for literal
    /// coding. Default 3.
    pub lc: u32,
    /// Literal-position bits (0..=4). Default 0.
    pub lp: u32,
    /// Position bits (0..=4). Default 2.
    pub pb: u32,
    /// Dictionary size in bytes (4 KB..=4 GB). Larger = better ratio
    /// on inputs with long-range repeats. Default 16 MB.
    pub dict_size: u32,
    /// If true, use the optimal (DP) parser — slower but better ratio.
    /// If false, use the lazy parser — faster, slightly worse ratio.
    pub use_optimal_parser: bool,
    /// Match-finder chain depth. Larger = better matches, slower encode.
    /// 0 = use the encoder default (`DEFAULT_CHAIN_LENGTH`).
    pub max_chain_length: u32,
    /// Stop the chain walk once a match this long is found. 0 = disabled
    /// (always walk the full chain). Larger = faster encode, slightly
    /// worse ratio on inputs where the chain has progressively longer
    /// matches (rare).
    pub nice_match: u32,
    /// LZMA2 reset mode for cross-call state reuse. See [`ResetMode`].
    /// Default: [`ResetMode::Full`] (same as fresh allocation every call).
    /// Set to [`ResetMode::ReuseState`] for batch workloads to skip
    /// probability-model reset and carry adaptation forward.
    pub reset_mode: ResetMode,
}

/// Controls what gets reset between LZMA2 chunks (and between
/// `LzmaCompressor::compress` calls).
///
/// Maps directly to the LZMA2 chunk header's `reset_level` field
/// (bits 5-6 of the control byte).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ResetMode {
    /// Full reset: state + properties + all probability models.
    /// Same as creating a fresh encoder. This is the default.
    #[default]
    Full,
    /// Reuse state: keep probability models + properties, reset only
    /// the encoder state + repeat offsets. Faster than Full for
    /// batch workloads.
    ReuseState,
    /// Warm: carry everything forward (models + state + rep offsets).
    /// The fastest mode but changes the output bytes (no reset
    /// markers in the LZMA2 chunk headers).
    Warm,
}

impl Default for LzmaOptions {
    fn default() -> Self {
        Self {
            lc: DEFAULT_LC,
            lp: DEFAULT_LP,
            pb: DEFAULT_PB,
            dict_size: DEFAULT_DICT_SIZE,
            use_optimal_parser: false,
            max_chain_length: 0,
            nice_match: 0,
            reset_mode: ResetMode::Full,
        }
    }
}

impl LzmaOptions {
    /// Validate the parameters against the LZMA spec.
    ///
    /// # Errors
    ///
    /// Returns [`LzmaError::Corrupt`] if any parameter is out of range.
    pub fn validate(&self) -> Result<(), LzmaError> {
        if self.lc > MAX_LC {
            return Err(LzmaError::Corrupt {
                reason: format!("lc={} exceeds max {}", self.lc, MAX_LC),
            });
        }
        if self.lp > MAX_LP {
            return Err(LzmaError::Corrupt {
                reason: format!("lp={} exceeds max {}", self.lp, MAX_LP),
            });
        }
        if self.pb > MAX_PB {
            return Err(LzmaError::Corrupt {
                reason: format!("pb={} exceeds max {}", self.pb, MAX_PB),
            });
        }
        if self.lc + self.lp > LC_LP_SUM_MAX {
            return Err(LzmaError::Corrupt {
                reason: format!(
                    "lc + lp = {} + {} = {} exceeds {} (spec hard limit)",
                    self.lc,
                    self.lp,
                    self.lc + self.lp,
                    LC_LP_SUM_MAX
                ),
            });
        }
        if self.dict_size < MIN_DICT_SIZE {
            return Err(LzmaError::Corrupt {
                reason: format!(
                    "dict_size={} below minimum {}",
                    self.dict_size, MIN_DICT_SIZE
                ),
            });
        }
        Ok(())
    }
}

/// Compress `input` with explicit user-tunable options.
///
/// # Errors
///
/// Returns [`LzmaError::Corrupt`] if `options` fails validation or on
/// arithmetic overflow.
pub fn lzma_alone_compress_with_options(
    input: &[u8],
    options: &LzmaOptions,
) -> Result<Vec<u8>, LzmaError> {
    options.validate()?;
    let lc = options.lc;
    let lp = options.lp;
    let pb = options.pb;

    let mut out = Vec::with_capacity(input.len() + 13);
    let props_byte = (lc + 9 * lp + 45 * pb) as u8;
    out.push(props_byte);
    out.extend_from_slice(&options.dict_size.to_le_bytes());
    out.extend_from_slice(&(input.len() as u64).to_le_bytes());

    let encoder = Lzma1Encoder::new(lc, lp, pb);
    let stream = if options.use_optimal_parser {
        encoder.encode_optimal_with_tuning(input, options.max_chain_length, options.nice_match)
    } else {
        encoder.encode_with_tuning(input, options.max_chain_length, options.nice_match)
    };
    out.extend_from_slice(&stream);

    Ok(out)
}

/// Compress `input` into the `.lzma` (LZMA-Alone) container using the
/// optimal (DP) parser for best ratio.
///
/// # Errors
///
/// Returns [`LzmaError::Corrupt`] only on arithmetic overflow.
pub fn lzma_alone_compress_optimal(input: &[u8]) -> Result<Vec<u8>, LzmaError> {
    let opts = LzmaOptions {
        use_optimal_parser: true,
        ..Default::default()
    };
    lzma_alone_compress_with_options(input, &opts)
}

/// Compress `input` into the `.lzma` (LZMA-Alone) container.
///
/// # Errors
///
/// Returns [`LzmaError::Corrupt`] only on arithmetic overflow (shouldn't
/// happen for any plausible input).
pub fn lzma_alone_compress(input: &[u8]) -> Result<Vec<u8>, LzmaError> {
    lzma_alone_compress_with_options(input, &LzmaOptions::default())
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
