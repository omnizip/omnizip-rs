//! ISO 9660 image container — TODO.containers task 11: volume
//! descriptors (PVD + terminator), directory records with the
//! both-endian fields, recursive tree walk, Rock Ridge SUSP read (NM
//! names, PX modes) and Joliet supplementary descriptors, plus a
//! deterministic level-1 writer (fixed timestamps, sorted records,
//! little-endian path tables).
#![forbid(unsafe_code)]

pub mod reader;
pub mod writer;

/// Bytes per logical sector.
pub const SECTOR_SIZE: usize = 2048;
/// Volume descriptors start at sector 16.
pub const VOLUME_DESCRIPTOR_START: usize = 16;
/// Standard identifier at bytes 1..6 of every descriptor.
pub const ISO_IDENTIFIER: &[u8; 5] = b"CD001";

/// Descriptor types.
pub mod vd_type {
    pub const BOOT: u8 = 0;
    pub const PRIMARY: u8 = 1;
    pub const SUPPLEMENTARY: u8 = 2;
    pub const PARTITION: u8 = 3;
    pub const TERMINATOR: u8 = 255;
}

/// Directory record flags.
pub mod flags {
    pub const HIDDEN: u8 = 0x01;
    pub const DIRECTORY: u8 = 0x02;
    pub const ASSOCIATED: u8 = 0x04;
    pub const EXTENDED: u8 = 0x08;
    pub const PERMISSIONS: u8 = 0x10;
    pub const NOT_FINAL: u8 = 0x80;
}

/// The parsed primary (or supplementary) volume descriptor.
#[derive(Clone, Debug)]
pub struct VolumeDescriptor {
    pub type_: u8,
    pub system_identifier: String,
    pub volume_identifier: String,
    pub volume_space_size: u32,
    pub logical_block_size: u16,
    pub path_table_size: u32,
    pub path_table_location: u32,
    /// Root record: (extent, data_length).
    pub root: DirectoryRecord,
    /// True when this is a Joliet (UCS-2) supplementary descriptor.
    pub joliet: bool,
}

/// One directory record (file, dir, or the root pseudo-record).
#[derive(Clone, Debug, Default)]
pub struct DirectoryRecord {
    pub name: String,
    pub full_path: String,
    pub location: u32,
    pub data_length: u32,
    pub flags: u8,
    /// Recording date: years since 1900, month, day, hour, minute,
    /// second, timezone (signed 15-min steps).
    pub date: [u8; 7],
    pub system_use: Vec<u8>,
}

impl DirectoryRecord {
    #[must_use]
    pub const fn is_directory(&self) -> bool {
        self.flags & flags::DIRECTORY != 0
    }

    #[must_use]
    pub fn is_current(&self) -> bool {
        self.name == "\u{0}"
    }

    #[must_use]
    pub fn is_parent(&self) -> bool {
        self.name == "\u{1}"
    }

    /// Recording date → unix seconds (UTC-assumed, tz applied).
    #[must_use]
    pub fn mtime_unix(&self) -> u64 {
        let (y, mo, d, h, mi, s) = (
            u32::from(self.date[0]) + 1900,
            u32::from(self.date[1]),
            u32::from(self.date[2]),
            u32::from(self.date[3]),
            u32::from(self.date[4]),
            u32::from(self.date[5]),
        );
        if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || y < 1970 {
            return 0;
        }
        let days = days_from_civil(i64::from(y), mo, d);
        let tz_offset = i16::from(self.date[6].wrapping_sub(128)) as i64 * 900;
        let secs = days * 86_400 + i64::from(h * 3600 + mi * 60 + s) - tz_offset;
        secs.max(0) as u64
    }

    /// Rock Ridge NM name from the system-use area, if present.
    #[must_use]
    pub fn rock_ridge_name(&self) -> Option<String> {
        for entry in susp_entries(&self.system_use) {
            if entry.len() >= 5 && &entry[0..2] == b"NM" {
                // CONTINUE-flagged name chains are out of scope for
                // names under one sector; take this entry's payload.
                if let Some(name) = entry.get(5..) {
                    return Some(String::from_utf8_lossy(name).into_owned());
                }
            }
        }
        None
    }

    /// Rock Ridge PX (mode, uid, gid) from the system-use area.
    #[must_use]
    pub fn rock_ridge_mode(&self) -> Option<u32> {
        for entry in susp_entries(&self.system_use) {
            if entry.len() >= 5 && &entry[0..2] == b"PX" {
                let mode = u32::from_le_bytes(entry.get(4..8)?.try_into().ok()?);
                return Some(mode & 0o7777);
            }
        }
        None
    }
}

/// Iterate SUSP entries: two-byte signature, length, version, payload.
fn susp_entries(su: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut pos = 0usize;
    std::iter::from_fn(move || {
        // Records are 2-byte aligned; skip single padding bytes.
        while su.get(pos) == Some(&0) {
            pos += 1;
        }
        let len = su.get(pos).copied().unwrap_or(0) as usize;
        if len < 4 || pos + len > su.len() {
            return None;
        }
        let entry = &su[pos..pos + len];
        pos += len;
        Some(entry)
    })
}

/// Days from 1970-01-01 to y-m-d (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = i64::from(m) + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Parse one directory record at `offset` in `data`; `None` at a
/// padding/terminator byte.
pub fn parse_record(data: &[u8], offset: usize, joliet: bool) -> Option<DirectoryRecord> {
    let length = *data.get(offset)? as usize;
    if length == 0 {
        return None;
    }
    let rec = data.get(offset..offset + length)?;
    if rec.len() < 33 {
        return None;
    }
    let location = u32::from_le_bytes(rec[2..6].try_into().expect("4"));
    let data_length = u32::from_le_bytes(rec[10..14].try_into().expect("4"));
    let mut date = [0u8; 7];
    date.copy_from_slice(&rec[18..25]);
    let flags = rec[25];
    let name_len = rec[32] as usize;
    let name_bytes = rec.get(33..33 + name_len)?;

    let name = if name_bytes == b"\x00" {
        "\u{0}".into()
    } else if name_bytes == b"\x01" {
        "\u{1}".into()
    } else if joliet {
        let units: Vec<u16> = name_bytes
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(name_bytes).into_owned()
    };

    let mut su_offset = 33 + name_len;
    if name_len % 2 == 0 {
        su_offset += 1;
    }
    let system_use = rec.get(su_offset..).unwrap_or(&[]).to_vec();

    Some(DirectoryRecord {
        name,
        full_path: String::new(),
        location,
        data_length,
        flags,
        date,
        system_use,
    })
}

/// Parse a volume-descriptor sector.
pub fn parse_volume_descriptor(
    sector: &[u8],
) -> Result<VolumeDescriptor, omnizip_archive_core::ArchiveError> {
    use omnizip_archive_core::ArchiveError;
    if sector.len() < SECTOR_SIZE {
        return Err(ArchiveError::InvalidArchive(
            "iso: short descriptor sector".into(),
        ));
    }
    let type_ = sector[0];
    if sector[1..6] != *ISO_IDENTIFIER {
        return Err(ArchiveError::InvalidArchive(
            "iso: missing CD001 identifier".into(),
        ));
    }
    let joliet = type_ == vd_type::SUPPLEMENTARY
        && sector[88] == 0x25
        && sector[89] == 0x2f
        && sector[90] == 0x45;
    let carries_tree = type_ == vd_type::PRIMARY || type_ == vd_type::SUPPLEMENTARY;
    let root = if carries_tree {
        parse_record(&sector[156..190], 0, joliet)
            .ok_or_else(|| ArchiveError::InvalidArchive("iso: bad root record".into()))?
    } else {
        DirectoryRecord::default()
    };
    let ident = |start: usize, len: usize| {
        let field = &sector[start..start + len];
        let end = field
            .iter()
            .position(|&b| b == b' ' || b == 0)
            .unwrap_or(len);
        if joliet {
            let units: Vec<u16> = field[..end]
                .chunks_exact(2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        } else {
            String::from_utf8_lossy(&field[..end]).into_owned()
        }
    };
    Ok(VolumeDescriptor {
        type_,
        system_identifier: ident(8, 32),
        volume_identifier: ident(40, 32),
        volume_space_size: u32::from_le_bytes(sector[80..84].try_into().expect("4")),
        logical_block_size: u16::from_le_bytes(sector[128..130].try_into().expect("2")),
        path_table_size: u32::from_le_bytes(sector[132..136].try_into().expect("4")),
        path_table_location: u32::from_le_bytes(sector[140..144].try_into().expect("4")),
        root,
        joliet,
    })
}
