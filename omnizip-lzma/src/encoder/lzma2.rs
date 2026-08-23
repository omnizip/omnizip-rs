//! LZMA2 chunk encoder — wraps [`super::lzma1::Lzma1Encoder`] output
//! into the LZMA2 chunk format.
//!
//! Each chunk has a control byte + size fields + (optionally) LZMA
//! properties + LZMA1 stream. Chunks are concatenated and the stream
//! ends with a 0x00 control byte.

#![forbid(unsafe_code)]

use super::alone::LzmaOptions;
use crate::constants::MATCH_LEN_MAX;
use crate::LzmaError;

/// Default LZMA parameters (matches lzip/xz-utils defaults).
#[allow(dead_code)]
const DEFAULT_LC: u32 = 3;
#[allow(dead_code)]
const DEFAULT_LP: u32 = 0;
#[allow(dead_code)]
const DEFAULT_PB: u32 = 2;
#[allow(dead_code)]
const DEFAULT_DICT_SIZE: u32 = 16 * 1024 * 1024;

/// Maximum uncompressed size per LZMA2 chunk: `(1 << 21) - 1`.
const MAX_CHUNK_UNCOMPRESSED: usize = (1 << 21) - 1;

/// Compress `input` as an LZMA2 stream. Uses one LZMA1 encoder per
/// chunk with state reset at the start (level 3 = full reset for the
/// first chunk).
pub fn encode_lzma2_stream(input: &[u8]) -> Result<Vec<u8>, LzmaError> {
    encode_lzma2_stream_with_options(input, &LzmaOptions::default())
}

/// Compress `input` as an LZMA2 stream with explicit LZMA parameters
/// (lc, lp, pb). The `dict_size` field of `options` is used for
/// validation only — LZMA2 chunks always use the encoder's internal
/// dict size, which is fixed by `MAX_CHUNK_UNCOMPRESSED`.
///
/// # Errors
///

/// Emit one LZMA2 chunk for `chunk` (at absolute `offset`), splitting
/// when the compressed form cannot fit the format's 16-bit compressed
/// size field.
///
/// The LZMA2 chunk header stores `compressed_size - 1` in a u16, so a
/// chunk's compressed payload is capped at 65536 bytes — far below the
/// 2 MiB uncompressed chunk cap. A 2 MiB text chunk compresses to
/// several hundred KB, so large inputs used to error out ("LZMA2 chunk
/// compressed size exceeds u16") at every level. Upstream liblzma
/// terminates a chunk when the compressed size reaches its limit; the
/// equivalent here is bisection: re-encode halves until each fits. A
/// chunk of 64 KiB or less that still does not fit is incompressible
/// (LZMA expansion) — those are stored raw with the uncompressed
/// chunk control (0x01), exactly what the format exists for.
fn emit_lzma2_chunk(
    chunk: &[u8],
    offset: usize,
    input: &[u8],
    first_chunk: &mut bool,
    props_pending: &mut bool,
    out: &mut Vec<u8>,
    lc: u32,
    lp: u32,
    pb: u32,
    use_optimal: bool,
    options: &LzmaOptions,
) -> Result<(), LzmaError> {
    let prev_byte = if offset > 0 { input[offset - 1] } else { 0 };
    let mut encoder = crate::encoder::Lzma1Encoder::new(lc, lp, pb)
        .with_base_pos(offset as u32)
        .with_base_prev_byte(prev_byte)
        .without_eopm();
    if options.use_bt4 {
        encoder = encoder.with_bt4();
    }
    let compressed = if use_optimal {
        encoder.encode_optimal_with_tuning(chunk, options.max_chain_length, options.nice_match)
    } else {
        encoder.encode_with_tuning(chunk, options.max_chain_length, options.nice_match)
    };

    // LZMA2 chunks carry the LZMA1 symbols plus the range-coder
    // flush but NO EOPM: the reference decoder stops at the chunk's
    // uncompressed size and then requires the range coder to be
    // exactly finished (rc_is_finished); an EOPM leaves trailing
    // unconsumed bits and xz rejects the chunk ("Compressed data is
    // corrupt"). Whether an EOPM-carrying chunk happened to pass was
    // luck of the rc byte alignment.
    if compressed.len() > u16::MAX as usize {
        if chunk.len() > 1 << 16 {
            let mid = chunk.len() / 2;
            emit_lzma2_chunk(
                &chunk[..mid],
                offset,
                input,
                first_chunk,
                props_pending,
                out,
                lc,
                lp,
                pb,
                use_optimal,
                options,
            )?;
            emit_lzma2_chunk(
                &chunk[mid..],
                offset + mid,
                input,
                first_chunk,
                props_pending,
                out,
                lc,
                lp,
                pb,
                use_optimal,
                options,
            )?;
            return Ok(());
        }
        // Incompressible: store raw (control 0x01, size-1 in u16).
        // chunk.len() <= 64 KiB here, so the field always fits.
        // Control 0x01 = raw + DICTIONARY RESET; 0x02 = raw, dict
        // carries. Mid-stream raws MUST NOT reset the dictionary:
        // subsequent compressed chunks are encoded with distances that
        // reference data across the raw stretch (our match finders
        // treat the input as one continuous dictionary). Emitting 0x01
        // here made conformant decoders (xz) drop the dictionary and
        // then reject the next chunk's cross-boundary match — while
        // our own decoder (no reset concept) round-tripped happily
        // (omnizip#329).
        let control = if *first_chunk { 0x01 } else { 0x02 };
        out.push(control);
        out.extend_from_slice(&((chunk.len() - 1) as u16).to_be_bytes());
        out.extend_from_slice(chunk);
        // A dictionary-resetting raw chunk (0x01, only ever the first)
        // obliges the NEXT LZMA chunk to carry new properties
        // (lzma2_decoder.c: control >= 0xE0 || control == 1 sets
        // need_properties). Set-only: subsequent non-resetting raws
        // must not clear a pending obligation.
        if *first_chunk {
            *props_pending = true;
        }
        *first_chunk = false;
        return Ok(());
    }
    let usable_compressed_size = compressed.len();
    let u_size = chunk.len() - 1;
    let c_size = usable_compressed_size - 1;

    // LZMA2 reset-level bits (5-6 of control byte):
    //   0 = no reset (state + models + dict all carry)
    //   1 = reset state + models + reps (dict carries)
    //   2 = reset state + models + reps + read new props byte
    //   3 = reset state + models + reps + read new props + reset dict
    //
    // We reuse the Lzma1Encoder across chunks so probability
    // models adapt continuously, but the decoder side resets
    // everything for each chunk (because the encoded range coder
    // state carries through its byte-level output, the decoder
    // picks it up from a fresh range decoder init — but the LZMA
    // state machine has advanced). Using reset_level=1 here so
    // the decoder's reset_state matches the encoder's fresh LZMA
    // state per chunk. True model carry (reset_level=0 + decoder
    // also carrying models) is TODO and requires more work.
    let reset_level: u8 = if *first_chunk {
        3
    } else if *props_pending {
        // 2 = state reset + NEW PROPERTIES, dictionary carries — the
        // minimum a conformant decoder accepts after a dict-reset
        // chunk.
        2
    } else {
        1
    };
    *props_pending = false;
    let control: u8 = 0x80 | (reset_level << 5) | ((u_size >> 16) as u8 & 0x1F);
    out.push(control);
    out.extend_from_slice(&((u_size & 0xFFFF) as u16).to_be_bytes());
    out.extend_from_slice(&((c_size & 0xFFFF) as u16).to_be_bytes());

    if reset_level >= 2 {
        let props = (lc + 9 * lp + 45 * pb) as u8;
        out.push(props);
    }

    out.extend_from_slice(&compressed[..usable_compressed_size]);
    *first_chunk = false;
    Ok(())
}

/// Returns [`LzmaError::Corrupt`] on parameter validation failure.
pub fn encode_lzma2_stream_with_options(
    input: &[u8],
    options: &LzmaOptions,
) -> Result<Vec<u8>, LzmaError> {
    options.validate()?;
    let lc = options.lc;
    let lp = options.lp;
    let pb = options.pb;
    let use_optimal = options.use_optimal_parser;
    let mut out = Vec::new();

    if input.is_empty() {
        // Empty input: single end-of-stream byte.
        out.push(0x00);
        return Ok(out);
    }

    // Fresh encoder per chunk — works correctly across all chunk
    // sizes. State-reuse encoding via encode_chunk_inplace is
    // available as a public API for future LZMA2 reset_level=0 work
    // (see TODO 176 item A — requires decoder-side state-carry too).
    let mut offset = 0;
    let mut first_chunk = true;
    let mut props_pending = false;

    // Optimal-parser configuration: the reference's price-based parse
    // (encoder/optimum.rs, ported from lzma_encoder_optimum_normal.c)
    // with one shared match finder, carried probability models, and
    // command-boundary chunk cutting.
    if use_optimal && !options.use_bt4 {
        use crate::encoder::optimum::OptimumState;

        let nice_len = if options.nice_match > 0 {
            options.nice_match
        } else {
            128
        };
        let mut mf = omnizip_codecs::Bt4MatchFinder::new(input, options.dict_size, nice_len, 0);
        let mut st = OptimumState::new(nice_len, options.dict_size, pb);
        let mut encoder =
            crate::encoder::Lzma1Encoder::with_dict_size(lc, lp, pb, options.dict_size)
                .without_eopm();
        encoder.length_encoder.set_table_size(nice_len);
        encoder.rep_length_encoder.set_table_size(nice_len);
        for ps in 0..(1usize << pb as usize) {
            encoder.length_encoder.update_prices(ps);
            encoder.rep_length_encoder.update_prices(ps);
        }
        encoder.optimum_fill_dist_prices(&mut st);
        encoder.optimum_fill_align_prices(&mut st);

        let budget = MAX_CHUNK_UNCOMPRESSED.saturating_sub(MATCH_LEN_MAX as usize + 1);
        let max_compressed = (u16::MAX as usize + 1).saturating_sub(32);

        encoder.optimum_init(&mut st, input, &mut mf);
        offset = 1.min(input.len());
        // The init literal belongs to the first chunk even when it is
        // the entire input (single-byte streams).
        let mut emitted_any = false;

        while offset < input.len() || !emitted_any {
            let chunk_start = if emitted_any { offset } else { 0 };
            while offset < input.len() {
                let (back, len) =
                    encoder.optimum_next_symbol(&mut st, input, &mut mf, offset as u32);
                encoder.optimum_emit_symbol(input, back, len, offset, &mut st);
                offset += len as usize;
                if offset - chunk_start >= budget
                    || encoder.range_encoder_bytes() >= max_compressed
                    || offset >= input.len()
                {
                    break;
                }
            }
            let chunk_size = offset - chunk_start;
            let compressed = encoder.take_range_encoder();
            debug_assert!(chunk_size >= 1);
            let u_size = chunk_size - 1;
            let c_size = compressed.len() - 1;
            let reset_level: u8 = if first_chunk {
                3
            } else if props_pending {
                2
            } else {
                0
            };
            let control: u8 = 0x80 | (reset_level << 5) | ((u_size >> 16) as u8 & 0x1F);
            out.push(control);
            out.extend_from_slice(&((u_size & 0xFFFF) as u16).to_be_bytes());
            out.extend_from_slice(&((c_size & 0xFFFF) as u16).to_be_bytes());
            if reset_level >= 2 {
                let props = (lc + 9 * lp + 45 * pb) as u8;
                out.push(props);
            }
            out.extend_from_slice(&compressed);
            first_chunk = false;
            props_pending = false;
            emitted_any = true;
        }

        out.push(0x00);
        return Ok(out);
    }

    // Lazy-parser configuration: dictionary-carry streaming. One
    // shared match finder spans the whole input, so every chunk's
    // matches may reference earlier chunks (within dict_size) — the
    // decode is legal because only the first chunk resets the
    // dictionary. This is where nearly all of the reference's ratio
    // on repetitive data comes from (measured without it: 100 MB
    // repetitive fixture compressed 4.9x larger than `xz -6`).
    if !use_optimal && !options.use_bt4 {
        let mut mf = crate::encoder::match_finder::new_lzma_match_finder(input, options.dict_size);
        let mut encoder: Option<crate::encoder::Lzma1Encoder> = None;
        // Force a state reset (reset level >= 2) on the next shared
        // chunk after the bisect/raw fallback path, because those
        // chunks encode with fresh probability models while the
        // carried encoder would have kept its trained ones.
        let mut force_state_reset = false;
        // Leave headroom so a match accepted at the budget edge cannot
        // push the chunk past the 21-bit uncompressed-size field: the
        // stop check runs after the command, and one command adds at
        // most MATCH_LEN_MAX (273) positions.
        let budget = MAX_CHUNK_UNCOMPRESSED.saturating_sub(MATCH_LEN_MAX as usize + 1);
        while offset < input.len() {
            if encoder.is_none() {
                encoder = Some(
                    crate::encoder::Lzma1Encoder::with_dict_size(lc, lp, pb, options.dict_size)
                        .without_eopm(),
                );
            }
            let enc = encoder.as_mut().expect("encoder initialized");
            let (compressed, end) = enc.encode_lazy_range(
                input,
                &mut mf,
                offset,
                budget,
                (u16::MAX as usize + 1).saturating_sub(32),
                options.max_chain_length,
                options.nice_match,
            );
            let chunk_size = end - offset;
            if compressed.len() <= u16::MAX as usize && chunk_size <= MAX_CHUNK_UNCOMPRESSED {
                let u_size = chunk_size - 1;
                let c_size = compressed.len() - 1;
                let reset_level: u8 = if first_chunk {
                    3
                } else if props_pending || force_state_reset {
                    2
                } else {
                    // Continue with trained probability models,
                    // state, and rep distances across the chunk
                    // boundary — the decode-side counterpart of
                    // keeping one `Lzma1Encoder` alive per stream.
                    0
                };
                let control: u8 = 0x80 | (reset_level << 5) | ((u_size >> 16) as u8 & 0x1F);
                out.push(control);
                out.extend_from_slice(&((u_size & 0xFFFF) as u16).to_be_bytes());
                out.extend_from_slice(&((c_size & 0xFFFF) as u16).to_be_bytes());
                if reset_level >= 2 {
                    let props = (lc + 9 * lp + 45 * pb) as u8;
                    out.push(props);
                }
                out.extend_from_slice(&compressed);
                first_chunk = false;
                props_pending = false;
                force_state_reset = false;
                offset = end;
            } else {
                // Over-budget (incompressible): fall back to the
                // chunk-local bisect/raw path for this range.
                let chunk = &input[offset..end.min(input.len())];
                emit_lzma2_chunk(
                    chunk,
                    offset,
                    input,
                    &mut first_chunk,
                    &mut props_pending,
                    &mut out,
                    lc,
                    lp,
                    pb,
                    use_optimal,
                    options,
                )?;
                // The fallback's chunks encode with fresh models at
                // reset level >= 1, so the decoder ends up fully
                // reset; drop the carried encoder and mark the next
                // shared chunk as a state reset.
                encoder = None;
                force_state_reset = true;
                offset = end.min(input.len());
            }
        }

        // End-of-stream marker.
        out.push(0x00);
        return Ok(out);
    }

    while offset < input.len() {
        let remaining = input.len() - offset;
        let chunk_size = remaining.min(MAX_CHUNK_UNCOMPRESSED);
        let chunk = &input[offset..offset + chunk_size];
        emit_lzma2_chunk(
            chunk,
            offset,
            input,
            &mut first_chunk,
            &mut props_pending,
            &mut out,
            lc,
            lp,
            pb,
            use_optimal,
            options,
        )?;
        offset += chunk_size;
    }

    // End-of-stream marker.
    out.push(0x00);
    Ok(out)
}

#[cfg(test)]
mod tests {

    /// Regression (user report): LZMA2 used to error with "chunk
    /// compressed size exceeds u16" once any 2 MiB chunk compressed
    /// past 64 KiB — i.e. on any sufficiently large input. Chunks now
    /// bisect until they fit, and incompressible tail chunks are
    /// stored raw (control 0x01).
    #[test]
    fn chunk_compressed_size_always_fits_u16() {
        let mut data = Vec::new();
        for i in 0..600_000u32 {
            // Text-ish and compressible: a 2 MiB chunk compresses to
            // well over 64 KiB, forcing the bisection path.
            data.extend_from_slice(
                format!("record {i}: some compressible payload text\n").as_bytes(),
            );
        }
        let stream = super::encode_lzma2_stream(&data).expect("encode");
        // Walk the chunk headers: every compressed size must fit u16.
        let mut cursor = 0;
        let mut chunks = 0;
        while stream[cursor] != 0 {
            let control = stream[cursor];
            if control <= 2 {
                let size =
                    u16::from_be_bytes([stream[cursor + 1], stream[cursor + 2]]) as usize + 1;
                cursor += 3 + size;
            } else {
                let reset = (control >> 5) & 3;
                let csize =
                    u16::from_be_bytes([stream[cursor + 3], stream[cursor + 4]]) as usize + 1;
                assert!(csize <= u16::MAX as usize + 1);
                cursor += if reset >= 2 { 6 } else { 5 } + csize;
            }
            chunks += 1;
        }
        assert!(chunks > 1, "expected multiple chunks, got {chunks}");
        let (out, _) = crate::lzma2::decode_lzma2_stream(&stream).expect("decode");
        assert_eq!(out, data);
    }

    /// Regression (user report): small inputs whose range-coder state
    /// ends below TOP need one conditional tail byte after the flush
    /// — size-bounded decoders (xz) reject the chunk otherwise. The
    /// failing lengths (1, 28-30, 51, 66-68 for one fixture) must all
    /// round-trip; the sweep covers every prefix length.
    #[test]
    fn every_prefix_length_round_trips() {
        let base: Vec<u8> = (0..200u32)
            .map(|i| {
                let line = format!("row {i} has text and repetition repetition\n");
                line.as_bytes()[usize::from(i as u8) as usize % line.len()]
            })
            .collect();
        for n in 1..=base.len() {
            let input = &base[..n];
            let stream = super::encode_lzma2_stream(input).expect("encode");
            let (out, _) = crate::lzma2::decode_lzma2_stream(&stream).expect("decode");
            assert_eq!(out, input, "round-trip failed at length {n}");
        }
    }

    use super::*;
    use crate::lzma2::decode_lzma2_stream;

    #[test]
    fn empty_input_round_trips() {
        let compressed = encode_lzma2_stream(&[]).expect("encode");
        let (out, consumed) = decode_lzma2_stream(&compressed).expect("decode");
        assert!(out.is_empty());
        assert_eq!(consumed, 1); // just the EOS byte
    }

    #[test]
    fn small_input_round_trips() {
        let input = b"hello LZMA2 world";
        let compressed = encode_lzma2_stream(input).expect("encode");
        let (out, _) = decode_lzma2_stream(&compressed).expect("decode");
        assert_eq!(out, input);
    }

    #[test]
    fn determinism() {
        let encode_once = || encode_lzma2_stream(b"determinism test").unwrap();
        assert_eq!(encode_once(), encode_once());
    }

    #[test]
    fn multi_chunk_round_trips() {
        // Input larger than MAX_CHUNK_UNCOMPRESSED (2 MiB) forces
        // multiple LZMA2 chunks. Tracked in TODO 176 item A.
        let input: Vec<u8> = (0..2_100_000u32)
            .map(|i| {
                if i % 100 < 50 {
                    ((i % 26) + 97) as u8
                } else {
                    (i % 256) as u8
                }
            })
            .collect();
        let opts = super::LzmaOptions {
            use_optimal_parser: false,
            ..Default::default()
        };
        let compressed = encode_lzma2_stream_with_options(&input, &opts).expect("encode");
        let (out, _) = decode_lzma2_stream(&compressed).expect("decode");
        assert_eq!(out, input, "multi-chunk LZMA2 round-trip mismatch");
    }
}
