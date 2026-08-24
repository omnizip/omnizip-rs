//! RPM package format — TODO.containers task 09's RPM half: the
//! 96-byte lead, signature + main header regions (tag entries over a
//! data blob, 8-byte alignment), and the CPIO payload with its
//! compressor selection (gzip/bzip2/xz/zstd). Port of the Ruby
//! `formats/rpm/` module shape with the task-17 determinism rules
//! applied to the writer (fixed mtime, sorted tag order, derived
//! build time).
#![forbid(unsafe_code)]

pub mod reader;
pub mod writer;

/// Lead magic `ED AB EE DB`.
pub const LEAD_MAGIC: [u8; 4] = [0xED, 0xAB, 0xEE, 0xDB];
/// 8-byte header magic: `\x8e\xad\xe8` + version 1 + reserved.
pub const HEADER_MAGIC: [u8; 8] = [0x8E, 0xAD, 0xE8, 0x01, 0, 0, 0, 0];
pub const LEAD_SIZE: usize = 96;
pub const HEADER_HEADER_SIZE: usize = 16;
pub const TAG_ENTRY_SIZE: usize = 16;
/// The lead's signature_type value meaning "header signature present".
pub const HEADER_SIGNED_TYPE: u16 = 5;
pub const PACKAGE_BINARY: u16 = 0;

/// Tag data types (`rpm/rpmtag.h`).
pub mod types {
    pub const NULL: u32 = 0;
    pub const CHAR: u32 = 1;
    pub const INT8: u32 = 2;
    pub const INT16: u32 = 3;
    pub const INT32: u32 = 4;
    pub const INT64: u32 = 5;
    pub const STRING: u32 = 6;
    pub const BINARY: u32 = 7;
    pub const STRING_ARRAY: u32 = 8;
    pub const I18NSTRING: u32 = 9;
}

/// Tag ids used by the reader/writer (`rpm/rpmtag.h`, the Ruby
/// `TAG_IDS` table).
pub mod tags {
    pub const SIGSIZE: u32 = 257;
    pub const SHA1HEADER: u32 = 269;
    pub const NAME: u32 = 1000;
    pub const VERSION: u32 = 1001;
    pub const RELEASE: u32 = 1002;
    pub const EPOCH: u32 = 1003;
    pub const SUMMARY: u32 = 1004;
    pub const DESCRIPTION: u32 = 1005;
    pub const BUILDTIME: u32 = 1006;
    pub const BUILDHOST: u32 = 1007;
    pub const SIZE: u32 = 1009;
    pub const VENDOR: u32 = 1011;
    pub const LICENSE: u32 = 1014;
    pub const PACKAGER: u32 = 1015;
    pub const GROUP: u32 = 1016;
    pub const URL: u32 = 1020;
    pub const OS: u32 = 1021;
    pub const ARCH: u32 = 1022;
    pub const FILESIZES: u32 = 1028;
    pub const FILEMODES: u32 = 1030;
    pub const FILEUIDS: u32 = 1031;
    pub const FILEGIDS: u32 = 1032;
    pub const FILEMTIMES: u32 = 1034;
    pub const FILEDIGESTS: u32 = 1035;
    pub const FILELINKTOS: u32 = 1036;
    pub const FILEFLAGS: u32 = 1037;
    pub const FILEUSERNAME: u32 = 1039;
    pub const FILEGROUPNAME: u32 = 1040;
    pub const ARCHIVESIZE: u32 = 1046;
    pub const RPMVERSION: u32 = 1064;
    pub const DIRINDEXES: u32 = 1116;
    pub const BASENAMES: u32 = 1117;
    pub const DIRNAMES: u32 = 1118;
    pub const PAYLOADFORMAT: u32 = 1124;
    pub const PAYLOADCOMPRESSOR: u32 = 1125;
    pub const PAYLOADFLAGS: u32 = 1126;
    pub const EPOCHNUM: u32 = 5019;
}

/// One parsed header tag with its decoded value.
#[derive(Clone, Debug, PartialEq)]
pub enum TagValue {
    Int32(Vec<u32>),
    Int16(Vec<u16>),
    Int64(Vec<u64>),
    String(String),
    StringArray(Vec<String>),
    Binary(Vec<u8>),
}

impl TagValue {
    /// First u32 (buildtime, epoch, …).
    #[must_use]
    pub fn as_u32(&self) -> Option<u32> {
        match self {
            Self::Int32(v) => v.first().copied(),
            Self::Int64(v) => v.first().map(|&x| x as u32),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str_array(&self) -> Option<&[String]> {
        match self {
            Self::StringArray(v) => Some(v),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_u32_slice(&self) -> Option<&[u32]> {
        match self {
            Self::Int32(v) => Some(v),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_u16_slice(&self) -> Option<&[u16]> {
        match self {
            Self::Int16(v) => Some(v),
            _ => None,
        }
    }
}

/// A parsed header (signature or main).
#[derive(Clone, Debug, Default)]
pub struct RpmHeader {
    pub entries: Vec<(u32, TagValue)>,
    /// Total serialized length (16 + 16*n + blob, before padding).
    pub length: usize,
}

impl RpmHeader {
    #[must_use]
    pub fn get(&self, tag: u32) -> Option<&TagValue> {
        self.entries.iter().find(|(t, _)| *t == tag).map(|(_, v)| v)
    }
}

/// The parsed 96-byte lead.
#[derive(Clone, Debug)]
pub struct Lead {
    pub major: u8,
    pub minor: u8,
    pub type_: u16,
    pub architecture: u16,
    pub name: String,
    pub os: u16,
    pub signature_type: u16,
}

/// Parse a lead from `data` (must be ≥ 96 bytes).
pub fn parse_lead(data: &[u8]) -> Result<Lead, omnizip_archive_core::ArchiveError> {
    use omnizip_archive_core::ArchiveError;
    let raw = data
        .get(..LEAD_SIZE)
        .ok_or_else(|| ArchiveError::InvalidArchive("truncated RPM lead".into()))?;
    if raw[0..4] != LEAD_MAGIC {
        return Err(ArchiveError::InvalidArchive(
            "invalid RPM lead magic".into(),
        ));
    }
    let u16be = |o: usize| u16::from_be_bytes([raw[o], raw[o + 1]]);
    let name_end = raw[10..76].iter().position(|&b| b == 0).unwrap_or(66);
    Ok(Lead {
        major: raw[4],
        minor: raw[5],
        type_: u16be(6),
        architecture: u16be(8),
        name: String::from_utf8_lossy(&raw[10..10 + name_end]).into_owned(),
        os: u16be(76),
        signature_type: u16be(78),
    })
}

/// Parse one header (16-byte header-of-header + entries + blob)
/// starting at `offset`; returns the header and its padded length.
pub fn parse_header(
    data: &[u8],
    offset: usize,
) -> Result<(RpmHeader, usize), omnizip_archive_core::ArchiveError> {
    use omnizip_archive_core::ArchiveError;
    let hh = data
        .get(offset..offset + HEADER_HEADER_SIZE)
        .ok_or_else(|| ArchiveError::InvalidArchive("truncated RPM header header".into()))?;
    if hh[0..8] != HEADER_MAGIC {
        return Err(ArchiveError::InvalidArchive(
            "invalid RPM header magic".into(),
        ));
    }
    let n = u32::from_be_bytes(hh[8..12].try_into().expect("4")) as usize;
    let blob_len = u32::from_be_bytes(hh[12..16].try_into().expect("4")) as usize;
    let index_len = n * TAG_ENTRY_SIZE;
    let total = HEADER_HEADER_SIZE + index_len + blob_len;
    let region = data
        .get(offset..offset + total)
        .ok_or_else(|| ArchiveError::InvalidArchive("truncated RPM header body".into()))?;
    let index = &region[HEADER_HEADER_SIZE..HEADER_HEADER_SIZE + index_len];
    let blob = &region[HEADER_HEADER_SIZE + index_len..];

    let mut entries = Vec::with_capacity(n);
    for i in 0..n {
        let e = &index[i * TAG_ENTRY_SIZE..(i + 1) * TAG_ENTRY_SIZE];
        let tag = u32::from_be_bytes(e[0..4].try_into().expect("4"));
        let type_ = u32::from_be_bytes(e[4..8].try_into().expect("4"));
        let off = u32::from_be_bytes(e[8..12].try_into().expect("4")) as usize;
        let count = u32::from_be_bytes(e[12..16].try_into().expect("4")) as usize;
        let value = decode_value(type_, off, count, blob)
            .ok_or_else(|| ArchiveError::InvalidArchive(format!("tag {tag} out of bounds")))?;
        entries.push((tag, value));
    }
    Ok((
        RpmHeader {
            entries,
            length: total,
        },
        total,
    ))
}

fn decode_value(type_: u32, off: usize, count: usize, blob: &[u8]) -> Option<TagValue> {
    let field = |len: usize| blob.get(off..off + len).map(<[u8]>::to_vec);
    Some(match type_ {
        types::STRING | types::I18NSTRING => {
            let rest = blob.get(off..)?;
            let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
            TagValue::String(String::from_utf8_lossy(&rest[..end]).into_owned())
        }
        types::STRING_ARRAY => {
            let rest = blob.get(off..)?;
            let strings = rest
                .split(|&b| b == 0)
                .filter(|s| !s.is_empty())
                .take(count)
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .collect();
            TagValue::StringArray(strings)
        }
        types::INT32 => TagValue::Int32(
            field(count * 4)?
                .chunks_exact(4)
                .map(|c| u32::from_be_bytes(c.try_into().expect("4")))
                .collect(),
        ),
        types::INT16 => TagValue::Int16(
            field(count * 2)?
                .chunks_exact(2)
                .map(|c| u16::from_be_bytes(c.try_into().expect("2")))
                .collect(),
        ),
        types::INT64 => TagValue::Int64(
            field(count * 8)?
                .chunks_exact(8)
                .map(|c| u64::from_be_bytes(c.try_into().expect("8")))
                .collect(),
        ),
        types::INT8 | types::CHAR => TagValue::Binary(field(count)?),
        _ => TagValue::Binary(field(count)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lead_magic_check() {
        assert!(parse_lead(b"nope").is_err());
        let mut buf = vec![0u8; LEAD_SIZE];
        buf[0..4].copy_from_slice(&LEAD_MAGIC);
        buf[4] = 3;
        buf[6..8].copy_from_slice(&PACKAGE_BINARY.to_be_bytes());
        buf[10..14].copy_from_slice(b"test");
        let lead = parse_lead(&buf).unwrap();
        assert_eq!(lead.name, "test");
        assert_eq!(lead.signature_type, 0);
    }
}
