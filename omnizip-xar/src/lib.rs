//! XAR archive container — TODO.containers task 10: the 28-byte
//! header (`xar!` magic, both-endian tolerant size/version, BE u64
//! TOC lengths, checksum algorithm), the zlib-compressed XML table of
//! contents parsed and generated through quick-xml (the task's XML
//! decision), and the heap with per-file compression + SHA-1
//! extracted/archived checksums. Symlinks, directories, and nested
//! trees round-trip; creation is deterministic (fixed element order,
//! fixed creation-time from `WriteOptions`).
#![forbid(unsafe_code)]

pub mod reader;
pub mod toc;
pub mod writer;

/// `xar!`
pub const MAGIC: [u8; 4] = [0x78, 0x61, 0x72, 0x21];
/// Fixed header size (magic 4 + size 2 + version 2 + toc sizes 16 + alg 4).
pub const HEADER_SIZE: usize = 28;

/// Checksum algorithms (header field).
pub mod cksum {
    pub const NONE: u32 = 0;
    pub const SHA1: u32 = 1;
    pub const MD5: u32 = 2;
}

/// Encoding styles used in `<encoding style=…/>`.
pub const ENCODING_NONE: &str = "application/octet-stream";
pub const ENCODING_GZIP: &str = "application/x-gzip";

/// The parsed XAR header.
#[derive(Clone, Copy, Debug)]
pub struct XarHeader {
    pub toc_compressed_size: u64,
    pub toc_uncompressed_size: u64,
    pub checksum_algorithm: u32,
}

/// Parse the header; `data` must be ≥ [`HEADER_SIZE`].
pub fn parse_header(data: &[u8]) -> Result<XarHeader, omnizip_archive_core::ArchiveError> {
    use omnizip_archive_core::ArchiveError;
    let h = data
        .get(..HEADER_SIZE)
        .ok_or_else(|| ArchiveError::InvalidArchive("xar: header too short".into()))?;
    if h[0..4] != MAGIC {
        return Err(ArchiveError::InvalidArchive("xar: invalid magic".into()));
    }
    Ok(XarHeader {
        toc_compressed_size: u64::from_be_bytes(h[8..16].try_into().expect("8")),
        toc_uncompressed_size: u64::from_be_bytes(h[16..24].try_into().expect("8")),
        checksum_algorithm: u32::from_be_bytes(h[24..28].try_into().expect("4")),
    })
}

/// zlib (RFC 1950) compress: 2-byte header + raw deflate + adler32.
pub fn zlib_compress(data: &[u8]) -> Result<Vec<u8>, omnizip_archive_core::ArchiveError> {
    use omnizip_archive_core::ArchiveError;
    let raw = omnizip_libdeflate::deflate_dynamic::deflate_dynamic_huffman(data)
        .map_err(|e| ArchiveError::InvalidArchive(format!("deflate: {e}")))?
        .unwrap_or_else(|| {
            omnizip_libdeflate::deflate::deflate_stored(data).unwrap_or_else(|_| data.to_vec())
        });
    let mut out = Vec::with_capacity(raw.len() + 6);
    out.push(0x78);
    out.push(0x9C); // default compression, no dict, FCHECK bits valid
    out.extend_from_slice(&raw);
    out.extend_from_slice(&adler32(data).to_be_bytes());
    Ok(out)
}

/// zlib decompress: verify the adler32 trailer, inflate the body.
pub fn zlib_decompress(data: &[u8]) -> Result<Vec<u8>, omnizip_archive_core::ArchiveError> {
    use omnizip_archive_core::ArchiveError;
    if data.len() < 6 || data[0] & 0x0F != 8 {
        // Not zlib-wrapped: accept raw deflate as a fallback (some
        // writers emit it).
        return inflate_all(data);
    }
    let body = &data[2..data.len() - 4];
    let stored = u32::from_be_bytes(
        data[data.len() - 4..]
            .try_into()
            .map_err(|_| ArchiveError::InvalidArchive("xar: bad zlib trailer".into()))?,
    );
    let out = inflate_all(body)?;
    if adler32(&out) != stored {
        return Err(ArchiveError::Checksum("xar: zlib adler32 mismatch".into()));
    }
    Ok(out)
}

fn inflate_all(data: &[u8]) -> Result<Vec<u8>, omnizip_archive_core::ArchiveError> {
    use omnizip_archive_core::ArchiveError;
    let mut hint = (data.len() * 6).max(64);
    loop {
        match omnizip_libdeflate::inflate::inflate(data, hint) {
            Ok(d) => return Ok(d),
            Err(_) if hint < (1 << 32) => hint = hint.saturating_mul(4),
            Err(e) => return Err(ArchiveError::InvalidArchive(format!("inflate: {e}"))),
        }
    }
}

/// RFC 1950 adler32.
#[must_use]
pub fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &byte in chunk {
            a += u32::from(byte);
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}
