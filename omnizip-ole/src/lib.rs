//! OLE2 compound files (CFB, MS-CFB) — TODO.containers task 12's OLE
//! half: header, DIFAT/FAT sector chains, the 128-byte directory
//! entries (name/UTF-16, object type, red-black sibling ids), and the
//! mini-FAT stream for sub-4096-byte streams; plus a valid writer
//! (balanced directory trees, mini stream). The MSI half lives in
//! [`msi`]: string pool, `_Tables`/`_Columns`, typed row decode.
#![forbid(unsafe_code)]

pub mod msi;
pub mod reader;
pub mod writer;

/// `D0 CF 11 E0 A1 B1 1A E1`
pub const MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
/// Sector size for version-3 files (2^9).
pub const SECTOR_SIZE: usize = 512;
/// Mini-sector size (2^6).
pub const MINI_SECTOR_SIZE: usize = 64;
/// Streams smaller than this live in the mini stream.
pub const MINI_CUTOFF: u32 = 4096;

/// Special FAT values.
pub mod fat {
    pub const FREESECT: u32 = 0xFFFF_FFFF;
    pub const ENDOFCHAIN: u32 = 0xFFFF_FFFE;
    pub const FATSECT: u32 = 0xFFFF_FFFD;
    pub const DIFSECT: u32 = 0xFFFF_FFFC;
    pub const NOSTREAM: u32 = 0xFFFF_FFFF;
}

/// Directory object types.
pub mod obj_type {
    pub const UNKNOWN: u8 = 0;
    pub const STORAGE: u8 = 1;
    pub const STREAM: u8 = 2;
    pub const ROOT: u8 = 5;
}

/// The parsed CFB header.
#[derive(Clone, Copy, Debug)]
pub struct CfbHeader {
    pub sector_shift: u16,
    pub mini_sector_shift: u16,
    pub num_fat_sectors: u32,
    pub first_dir_sector: u32,
    pub mini_stream_cutoff: u32,
    pub first_minifat_sector: u32,
    pub num_minifat_sectors: u32,
    /// First 109 DIFAT entries.
    pub difat: [u32; 109],
}

impl CfbHeader {
    #[must_use]
    pub fn sector_size(&self) -> usize {
        1usize << self.sector_shift
    }

    #[must_use]
    pub fn mini_sector_size(&self) -> usize {
        1usize << self.mini_sector_shift
    }

    /// Byte offset of sector `n` (sector ids are 0-based after the
    /// 512-byte header).
    #[must_use]
    pub fn sector_offset(&self, n: u32) -> usize {
        512 + n as usize * self.sector_size()
    }
}

/// Parse the 512-byte header.
pub fn parse_header(data: &[u8]) -> Result<CfbHeader, omnizip_archive_core::ArchiveError> {
    use omnizip_archive_core::ArchiveError;
    let h = data
        .get(..512)
        .ok_or_else(|| ArchiveError::InvalidArchive("ole: file shorter than the header".into()))?;
    if h[0..8] != MAGIC {
        return Err(ArchiveError::InvalidArchive("ole: invalid magic".into()));
    }
    let u16le = |o: usize| u16::from_le_bytes([h[o], h[o + 1]]);
    let u32le = |o: usize| u32::from_le_bytes(h[o..o + 4].try_into().expect("4"));
    if u16le(0x1C) != 0xFFFE {
        return Err(ArchiveError::InvalidArchive(
            "ole: unexpected byte order".into(),
        ));
    }
    let mut difat = [0u32; 109];
    for (i, slot) in difat.iter_mut().enumerate() {
        *slot = u32le(0x4C + i * 4);
    }
    Ok(CfbHeader {
        sector_shift: u16le(0x1E),
        mini_sector_shift: u16le(0x20),
        num_fat_sectors: u32le(0x2C),
        first_dir_sector: u32le(0x30),
        mini_stream_cutoff: u32le(0x38),
        first_minifat_sector: u32le(0x3C),
        num_minifat_sectors: u32le(0x40),
        difat,
    })
}

/// One 128-byte directory entry.
#[derive(Clone, Debug, Default)]
pub struct DirEntry {
    pub name: String,
    pub object_type: u8,
    pub left: u32,
    pub right: u32,
    pub child: u32,
    pub start_sector: u32,
    pub size: u64,
}

/// Parse one directory entry at `offset`.
pub fn parse_dir_entry(
    data: &[u8],
    offset: usize,
) -> Result<DirEntry, omnizip_archive_core::ArchiveError> {
    use omnizip_archive_core::ArchiveError;
    let e = data
        .get(offset..offset + 128)
        .ok_or_else(|| ArchiveError::InvalidArchive("ole: directory entry out of bounds".into()))?;
    let name_len = u16::from_le_bytes([e[64], e[65]]) as usize;
    let name_len = name_len.saturating_sub(2).min(62); // includes the NUL
    let units: Vec<u16> = e[..name_len]
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let u32le = |o: usize| u32::from_le_bytes(e[o..o + 4].try_into().expect("4"));
    Ok(DirEntry {
        name: String::from_utf16_lossy(&units),
        object_type: e[66],
        left: u32le(68),
        right: u32le(72),
        child: u32le(76),
        start_sector: u32le(116),
        size: u64::from_le_bytes(e[120..128].try_into().expect("8")),
    })
}
