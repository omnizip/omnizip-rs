//! 7z archive container — TODO.containers task 06: the 6-byte
//! signature + 32-byte start header (with CRC verification), the
//! property-encoded metadata header (pack/unpack/substreams/files
//! infos), folder coder chains mapped onto the in-house codecs (Copy,
//! LZMA, LZMA2, BZip2, Deflate + delta/BCJ filters), solid-block
//! extraction with caching, and deterministic writing (fixed FILETIME
//! mtimes, sorted entries, non-solid or one solid folder, 7zAES
//! stream/header encryption, multi-volume splits).
//!
//! Phases A (read), B (non-solid write) and C (solid write,
//! multi-volume, encrypted-header writing) are complete; reading
//! AES-encrypted archives is supported.
#![forbid(unsafe_code)]

pub mod parser;
pub mod reader;
pub mod writer;

use omnizip_archive_core::ArchiveError;

/// `7z\xBC\xAF\x27\x1C`
pub const SIGNATURE: [u8; 6] = [0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C];
/// Start header size (signature 6 + version 2 + crc 4 + offset 8 + size 8 + crc 4).
pub const START_HEADER_SIZE: usize = 32;

/// Property ids (`k7zPropId`).
pub mod property {
    pub const END: u64 = 0x00;
    pub const HEADER: u64 = 0x01;
    pub const ARCHIVE_PROPERTIES: u64 = 0x02;
    pub const ADDITIONAL_STREAMS_INFO: u64 = 0x03;
    pub const MAIN_STREAMS_INFO: u64 = 0x04;
    pub const FILES_INFO: u64 = 0x05;
    pub const PACK_INFO: u64 = 0x06;
    pub const UNPACK_INFO: u64 = 0x07;
    pub const SUBSTREAMS_INFO: u64 = 0x08;
    pub const SIZE: u64 = 0x09;
    pub const CRC: u64 = 0x0A;
    pub const FOLDER: u64 = 0x0B;
    pub const CODERS_UNPACK_SIZE: u64 = 0x0C;
    pub const NUM_UNPACK_STREAM: u64 = 0x0D;
    pub const EMPTY_STREAM: u64 = 0x0E;
    pub const EMPTY_FILE: u64 = 0x0F;
    pub const ANTI: u64 = 0x10;
    pub const NAME: u64 = 0x11;
    pub const CTIME: u64 = 0x12;
    pub const ATIME: u64 = 0x13;
    pub const MTIME: u64 = 0x14;
    pub const WIN_ATTRIB: u64 = 0x15;
    pub const COMMENT: u64 = 0x16;
    pub const ENCODED_HEADER: u64 = 0x17;
    pub const START_POS: u64 = 0x18;
    pub const DUMMY: u64 = 0x19;
}

/// Method ids (`k7zMethodID`).
pub mod method {
    pub const COPY: u64 = 0x00;
    pub const DELTA: u64 = 0x03;
    pub const LZMA2: u64 = 0x21;
    pub const LZMA: u64 = 0x030101;
    pub const PPMD: u64 = 0x030401;
    pub const BZIP2: u64 = 0x040202;
    pub const DEFLATE: u64 = 0x040108;
    pub const DEFLATE64: u64 = 0x040109;
    pub const BCJ_X86: u64 = 0x03030103;
    pub const BCJ_PPC: u64 = 0x03030205;
    pub const BCJ_IA64: u64 = 0x03030401;
    pub const BCJ_ARM: u64 = 0x03030501;
    pub const BCJ_ARMT: u64 = 0x03030701;
    pub const BCJ_SPARC: u64 = 0x03030805;
    pub const BCJ2: u64 = 0x0303011B;
    pub const ARM64: u64 = 0x03030601;
    pub const AES: u64 = 0x06F10701;

    #[must_use]
    pub fn name(id: u64) -> String {
        match id {
            COPY => "Copy".into(),
            DELTA => "Delta".into(),
            LZMA2 => "LZMA2".into(),
            LZMA => "LZMA".into(),
            PPMD => "PPMd".into(),
            BZIP2 => "BZip2".into(),
            DEFLATE => "Deflate".into(),
            DEFLATE64 => "Deflate64".into(),
            BCJ_X86 => "BCJ-x86".into(),
            BCJ2 => "BCJ2".into(),
            AES => "AES256".into(),
            other => format!("Unknown(0x{other:x})"),
        }
    }
}

/// One coder in a folder (id + optional properties + stream counts).
#[derive(Clone, Debug, Default)]
pub struct CoderInfo {
    pub method_id: u64,
    pub num_in_streams: u64,
    pub num_out_streams: u64,
    pub properties: Vec<u8>,
}

/// A folder: ordered coder chain + bind pairs + pack-stream mapping.
#[derive(Clone, Debug, Default)]
pub struct Folder {
    pub coders: Vec<CoderInfo>,
    /// (in-stream index, out-stream index) bindings.
    pub bind_pairs: Vec<(u64, u64)>,
    pub pack_stream_indices: Vec<u64>,
    /// One size per output stream.
    pub unpack_sizes: Vec<u64>,
    pub unpack_crc: Option<u32>,
}

impl Folder {
    /// Total uncompressed output size (the main output stream).
    #[must_use]
    pub fn uncompressed_size(&self) -> u64 {
        // The main output is the out-stream not consumed by a bind
        // pair; with the common single-chain shape it is the last
        // size.
        let main = self.main_out_stream();
        self.unpack_sizes
            .get(main as usize)
            .copied()
            .unwrap_or_else(|| self.unpack_sizes.iter().sum())
    }

    #[must_use]
    pub fn main_out_stream(&self) -> u64 {
        let num_out: u64 = self.coders.iter().map(|c| c.num_out_streams).sum();
        (0..num_out)
            .find(|o| !self.bind_pairs.iter().any(|&(_, out)| out == *o))
            .unwrap_or(num_out.saturating_sub(1))
    }
}

/// One file entry from the files-info section.
#[derive(Clone, Debug, Default)]
pub struct FileEntry {
    pub name: String,
    pub has_stream: bool,
    pub is_dir: bool,
    pub is_empty: bool,
    pub is_anti: bool,
    /// Unix mtime (seconds), from the Windows FILETIME field.
    pub mtime: Option<u64>,
    /// Windows attributes (unix mode in the high bits when made by
    /// 7-Zip on unix).
    pub attributes: Option<u32>,
    /// Stream size + folder mapping (filled by the reader).
    pub size: u64,
    pub folder_index: usize,
    pub file_index: usize,
    pub crc: Option<u32>,
}

impl FileEntry {
    /// Unix permission bits from the attributes (7-Zip stores the
    /// mode in the high 16 bits on unix).
    #[must_use]
    pub fn unix_mode(&self) -> Option<u32> {
        self.attributes.map(|a| (a >> 16) & 0o7777)
    }

    #[must_use]
    pub const fn is_symlink(&self) -> bool {
        false
    }
}

/// Streams info (pack + unpack + substreams) for the main streams.
#[derive(Clone, Debug, Default)]
pub struct StreamInfo {
    pub pack_pos: u64,
    pub pack_sizes: Vec<u64>,
    pub folders: Vec<Folder>,
    pub num_unpack_streams_in_folders: Vec<u64>,
    /// One size per substream across all folders.
    pub unpack_sizes: Vec<u64>,
    pub digests: Vec<Option<u32>>,
}

/// The parsed start header.
#[derive(Clone, Copy, Debug)]
pub struct StartHeader {
    pub next_header_offset: u64,
    pub next_header_size: u64,
    pub next_header_crc: u32,
}

/// Parse and verify the signature + start header.
pub fn parse_start_header(data: &[u8]) -> Result<StartHeader, ArchiveError> {
    let h = data.get(..START_HEADER_SIZE).ok_or_else(|| {
        ArchiveError::InvalidArchive("7z: file shorter than the start header".into())
    })?;
    if h[0..6] != SIGNATURE {
        return Err(ArchiveError::InvalidArchive("7z: invalid signature".into()));
    }
    if h[6] != 0 {
        return Err(ArchiveError::InvalidArchive(format!(
            "7z: unsupported major version {}",
            h[6]
        )));
    }
    let start_crc = u32::from_le_bytes(h[8..12].try_into().expect("4"));
    let next = &h[12..32];
    let computed = omnizip_archive_core::crc32(next);
    if computed != start_crc {
        return Err(ArchiveError::Checksum(format!(
            "7z: start-header CRC mismatch: stored {start_crc:08X}, computed {computed:08X}"
        )));
    }
    Ok(StartHeader {
        next_header_offset: u64::from_le_bytes(next[0..8].try_into().expect("8")),
        next_header_size: u64::from_le_bytes(next[8..16].try_into().expect("8")),
        next_header_crc: u32::from_le_bytes(next[16..20].try_into().expect("4")),
    })
}

/// Windows FILETIME (100ns since 1601) → unix seconds.
#[must_use]
pub const fn filetime_to_unix(ft: u64) -> u64 {
    ft / 10_000_000 - 11_644_473_600
}

/// Unix seconds → Windows FILETIME.
#[must_use]
pub const fn unix_to_filetime(unix: u64) -> u64 {
    (unix + 11_644_473_600) * 10_000_000
}

/// The 7z AES key derivation (7zAes.cpp `CKeyInfo::CalcKey`, 7-Zip
/// 24+): a single running SHA-256 over `2^cycles_power` replicas of
/// `salt || password-UTF-16LE || LE32(i) || 4×0`, `i` counting the
/// replicas from 0. `cycles_power == 0x3F` skips the KDF (key =
/// `salt || password`, zero-padded to 32 bytes).
///
/// The password enters as UTF-16LE because 7-Zip carries passwords as
/// `wchar_t` strings and calls `CryptoSetPassword` with the raw UTF-16
/// bytes (verified against 7zz 26.00-written archives).
#[must_use]
pub fn aes256_kdf(password: &str, salt: &[u8], cycles_power: u8) -> [u8; 32] {
    let pw16: Vec<u8> = password.encode_utf16().flat_map(u16::to_le_bytes).collect();
    if cycles_power == 0x3F {
        let mut key = [0u8; 32];
        for (slot, &b) in key.iter_mut().zip(salt.iter().chain(pw16.iter())) {
            *slot = b;
        }
        return key;
    }
    let num_rounds = 1u64 << cycles_power;
    let unroll = num_rounds.min(64) as u32;
    let mut sha = omnizip_crypto::Sha256::new();
    // One replica block per update, mirroring the reference's 64x
    // unroll; chunking does not change the digest.
    let mut block = Vec::with_capacity(unroll as usize * (salt.len() + pw16.len() + 8));
    let mut r: u32 = 0;
    while r < num_rounds as u32 {
        block.clear();
        for i in r..r + unroll {
            block.extend_from_slice(salt);
            block.extend_from_slice(&pw16);
            block.extend_from_slice(&i.to_le_bytes());
            block.extend_from_slice(&[0u8; 4]);
        }
        sha.update(&block);
        r += unroll;
    }
    sha.finalize()
}
