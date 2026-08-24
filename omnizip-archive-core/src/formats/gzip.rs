//! GZIP single-file format (RFC 1952) — port of
//! `omnizip/formats/gzip.rb`. Header + raw DEFLATE + CRC32/ISIZE
//! trailer; multi-member on decode.
#![forbid(unsafe_code)]

use crate::error::ArchiveError;
pub const GZIP_MAGIC: [u8; 2] = [0x1F, 0x8B];
pub const CM_DEFLATE: u8 = 8;

pub const FTEXT: u8 = 0x01;
pub const FHCRC: u8 = 0x02;
pub const FEXTRA: u8 = 0x04;
pub const FNAME: u8 = 0x08;
pub const FCOMMENT: u8 = 0x10;

pub const OS_UNIX: u8 = 3;

/// Parsed member header metadata.
#[derive(Clone, Debug, Default)]
pub struct GzipMetadata {
    pub mtime: u32,
    pub original_name: Option<String>,
    pub comment: Option<String>,
    pub os: u8,
}

/// Options for [`compress`].
#[derive(Clone, Debug)]
pub struct GzipOptions {
    /// Compression level 0-9.
    pub level: u8,
    /// Original filename (FNAME field); None omits it.
    pub original_name: Option<String>,
    /// MTIME field (unix seconds; 0 = none).
    pub mtime: u32,
}

impl Default for GzipOptions {
    fn default() -> Self {
        Self {
            level: 6,
            original_name: None,
            mtime: 0,
        }
    }
}

/// CRC-32 (IEEE, reflected) — the gzip trailer checksum.
#[must_use]
pub fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, slot) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *slot = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ u32::from(b)) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

/// Compress `input` into a complete gzip member.
///
/// # Errors
///
/// Returns [`ArchiveError::InvalidArchive`] on internal deflate
/// failure.
pub fn compress(input: &[u8], options: &GzipOptions) -> Result<Vec<u8>, ArchiveError> {
    let mut out = Vec::with_capacity(input.len() / 2 + 32);

    // Header: magic, CM=8, FLG, MTIME, XFL (level hint), OS.
    out.extend_from_slice(&GZIP_MAGIC);
    out.push(CM_DEFLATE);
    let mut flags = 0u8;
    if options.original_name.is_some() {
        flags |= FNAME;
    }
    out.push(flags);
    out.extend_from_slice(&options.mtime.to_le_bytes());
    out.push(match options.level {
        9 => 2,
        1 => 4,
        _ => 0,
    });
    out.push(OS_UNIX);

    if let Some(name) = &options.original_name {
        out.extend_from_slice(name.as_bytes());
        out.push(0);
    }

    // Raw DEFLATE body (dynamic-Huffman when it wins, stored as fallback).
    let body = omnizip_libdeflate::deflate_dynamic::deflate_dynamic_huffman(input)
        .map_err(|e| ArchiveError::InvalidArchive(format!("deflate: {e}")))?;
    let body = match body {
        Some(b) => b,
        None => omnizip_libdeflate::deflate::deflate_stored(input)
            .map_err(|e| ArchiveError::InvalidArchive(format!("deflate-stored: {e}")))?,
    };
    out.extend_from_slice(&body);

    // Trailer: CRC32 + ISIZE (mod 2^32).
    out.extend_from_slice(&crc32(input).to_le_bytes());
    out.extend_from_slice(&(input.len() as u32).to_le_bytes());
    Ok(out)
}

/// Decompress a (possibly multi-member) gzip stream, verifying each
/// member's CRC32 and ISIZE trailer.
///
/// # Errors
///
/// [`ArchiveError::InvalidArchive`] on malformed structure and
/// [`ArchiveError::Checksum`] on trailer mismatch.
pub fn decompress(input: &[u8]) -> Result<Vec<u8>, ArchiveError> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while cursor < input.len() {
        let member_end = decompress_member(input, cursor, &mut out)?;
        cursor = member_end;
    }
    Ok(out)
}

/// Decompressed metadata + output for a single-member stream.
///
/// # Errors
///
/// As [`decompress`].
pub fn decompress_with_metadata(input: &[u8]) -> Result<(GzipMetadata, Vec<u8>), ArchiveError> {
    let mut out = Vec::new();
    let end = decompress_member(input, 0, &mut out)?;
    if end != input.len() {
        return Err(ArchiveError::InvalidArchive(
            "trailing bytes after single gzip member".into(),
        ));
    }
    let ((meta, _flags), _off) = parse_header(input)?;
    Ok((meta, out))
}

fn decompress_member(input: &[u8], start: usize, out: &mut Vec<u8>) -> Result<usize, ArchiveError> {
    let ((meta, flags), data_offset) = parse_header_at(input, start)?;

    let _ = meta;

    // Find the DEFLATE stream end by inflating from data_offset: the
    // in-house inflate returns the output; we then need to know how
    // many input bytes it consumed. It does not report that, so
    // inflate incrementally is not available — instead inflate with a
    // generous expected length and then locate the trailer by trying
    // every byte offset is wrong. We use a dedicated scan: inflate
    // assumes the stream is a prefix of `input[data_offset..]` and
    // stops at the final block; the trailer follows immediately.
    // Since `inflate` consumes exactly the deflate stream, we re-find
    // its end with a bounds search using the known ISIZE/CRC check at
    // candidate positions.
    let tail = &input[data_offset..];
    let (decompressed, consumed) = inflate_prefix(tail, out)?;

    let _ = flags;
    let trailer_at = data_offset + consumed;
    let trailer = input
        .get(trailer_at..trailer_at + 8)
        .ok_or_else(|| ArchiveError::InvalidArchive("gzip trailer truncated".into()))?;
    let stored_crc = u32::from_le_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]);
    let stored_isize = u32::from_le_bytes([trailer[4], trailer[5], trailer[6], trailer[7]]);
    let crc = crc32(&decompressed);
    if stored_crc != crc {
        return Err(ArchiveError::Checksum(format!(
            "gzip CRC32: stored {stored_crc:08X}, computed {crc:08X}"
        )));
    }
    if stored_isize != (decompressed.len() as u32) {
        return Err(ArchiveError::Checksum(format!(
            "gzip ISIZE: stored {stored_isize}, actual {}",
            decompressed.len()
        )));
    }
    Ok(trailer_at + 8)
}

/// Inflate the raw DEFLATE stream at the front of `input`, appending
/// the output to `out` and returning `(output, consumed_input_bytes)`.
fn inflate_prefix(input: &[u8], out: &mut Vec<u8>) -> Result<(Vec<u8>, usize), ArchiveError> {
    // The in-house inflate needs an expected-length hint; DEFLATE
    // streams do not carry one. Retry with a growing hint until it
    // succeeds — the hint only needs to bound the output.
    let mut hint = (input.len() * 4).max(64);
    loop {
        match omnizip_libdeflate::inflate::inflate_with_consumed(input, hint) {
            Ok((data, consumed)) => {
                out.extend_from_slice(&data);
                return Ok((data, consumed));
            }
            Err(_) if hint < (1 << 34) => hint = hint.saturating_mul(4),
            Err(e) => {
                return Err(ArchiveError::InvalidArchive(format!("inflate: {e}")));
            }
        }
    }
}

type HeaderParts = ((GzipMetadata, u8), usize);

fn parse_header(input: &[u8]) -> Result<HeaderParts, ArchiveError> {
    parse_header_at(input, 0)
}

fn parse_header_at(input: &[u8], start: usize) -> Result<HeaderParts, ArchiveError> {
    let bad = |m: &str| ArchiveError::InvalidArchive(format!("gzip header: {m}"));
    let d = input.get(start..).ok_or_else(|| bad("truncated"))?;
    if d.len() < 10 {
        return Err(bad("shorter than the 10-byte minimum"));
    }
    if d[0..2] != GZIP_MAGIC {
        return Err(bad("bad magic"));
    }
    if d[2] != CM_DEFLATE {
        return Err(bad("compression method is not deflate"));
    }
    let flags = d[3];
    let mtime = u32::from_le_bytes([d[4], d[5], d[6], d[7]]);
    let os = d[9];

    let mut pos = 10usize;
    if flags & FEXTRA != 0 {
        if d.len() < pos + 2 {
            return Err(bad("truncated FEXTRA length"));
        }
        let xlen = u16::from_le_bytes([d[pos], d[pos + 1]]) as usize;
        pos += 2 + xlen;
    }
    let mut original_name = None;
    if flags & FNAME != 0 {
        let end = d[pos..]
            .iter()
            .position(|&b| b == 0)
            .map(|p| pos + p)
            .ok_or_else(|| bad("unterminated FNAME"))?;
        original_name = Some(
            String::from_utf8_lossy(&d[pos..end])
                .trim_end_matches('\0')
                .to_string(),
        );
        pos = end + 1;
    }
    let mut comment = None;
    if flags & FCOMMENT != 0 {
        let end = d[pos..]
            .iter()
            .position(|&b| b == 0)
            .map(|p| pos + p)
            .ok_or_else(|| bad("unterminated FCOMMENT"))?;
        comment = Some(String::from_utf8_lossy(&d[pos..end]).into_owned());
        pos = end + 1;
    }
    if flags & FHCRC != 0 {
        pos += 2;
    }
    if pos > d.len() {
        return Err(bad("header fields run past the member"));
    }
    Ok((
        (
            GzipMetadata {
                mtime,
                original_name,
                comment,
                os,
            },
            flags,
        ),
        start + pos,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let data = b"hello hello hello hello hello".repeat(10);
        let gz = compress(&data, &GzipOptions::default()).unwrap();
        let back = decompress(&gz).unwrap();
        assert_eq!(back, data);
    }

    #[test]
    fn system_gzip_decodes() {
        // `printf 'hello world\n' | gzip -9 -N` (system gzip 1.12,
        // no FNAME because stdin carries no name).
        let gz: [u8; 32] = [
            0x1F, 0x8B, 0x08, 0x00, 0xBF, 0xCD, 0x8B, 0x6A, 0x02, 0x03, 0xCB, 0x48, 0xCD, 0xC9,
            0xC9, 0x57, 0x28, 0xCF, 0x2F, 0xCA, 0x49, 0xE1, 0x02, 0x00, 0x2D, 0x3B, 0x08, 0xAF,
            0x0C, 0x00, 0x00, 0x00,
        ];
        let (meta, out) = decompress_with_metadata(&gz).unwrap();
        assert_eq!(out, b"hello world\n");
        assert_eq!(meta.original_name, None);
    }

    #[test]
    fn multi_member() {
        let a = compress(b"first ", &GzipOptions::default()).unwrap();
        let mut b = compress(b"second", &GzipOptions::default()).unwrap();
        let mut both = a;
        both.append(&mut b);
        assert_eq!(decompress(&both).unwrap(), b"first second");
    }

    #[test]
    fn crc_is_standard() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }
}
