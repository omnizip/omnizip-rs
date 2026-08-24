//! 512-byte ustar header parse/build — port of
//! `omnizip/formats/tar/header.rb`.
#![forbid(unsafe_code)]

use crate::{
    BLOCK_SIZE, HEADER_SIZE, TYPE_DIRECTORY, TYPE_REGULAR, TYPE_SYMLINK, USTAR_MAGIC, USTAR_VERSION,
};
use omnizip_archive_core::{ArchiveEntry, EntryKind};

pub(crate) struct RawHeader {
    pub name: String,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub mtime: u64,
    pub typeflag: u8,
    pub linkname: String,
    pub prefix: String,
}

fn field(header: &[u8], offset: usize, size: usize) -> &[u8] {
    &header[offset..offset + size]
}

pub(crate) fn extract_string(header: &[u8], offset: usize, size: usize) -> String {
    let f = field(header, offset, size);
    let end = f.iter().position(|&b| b == 0).unwrap_or(f.len());
    String::from_utf8_lossy(&f[..end]).into_owned()
}

pub(crate) fn extract_octal(header: &[u8], offset: usize, size: usize) -> u64 {
    let text = extract_string(header, offset, size);
    let trimmed = text.trim().trim_end_matches('\0');
    if trimmed.is_empty() {
        return 0;
    }
    // GNU base-256 extension: high bit of the first byte set.
    let f = field(header, offset, size);
    if f[0] & 0x80 != 0 {
        let mut v: u64 = u64::from(f[0] & 0x7F);
        for &b in &f[1..] {
            v = (v << 8) | u64::from(b);
        }
        return v;
    }
    u64::from_str_radix(trimmed, 8).unwrap_or(0)
}

/// Sum of header bytes with the checksum field replaced by spaces —
/// the ustar checksum.
pub(crate) fn calculate_checksum(header: &[u8]) -> u32 {
    let mut sum = 0u32;
    for (i, &b) in header.iter().enumerate() {
        sum += if (148..156).contains(&i) {
            u32::from(b' ')
        } else {
            u32::from(b)
        };
    }
    sum
}

fn all_zeros(data: &[u8]) -> bool {
    data.iter().all(|&b| b == 0)
}

/// Parse one 512-byte header. `Ok(None)` = end-of-archive marker.
///
/// # Errors
///
/// [`omnizip_archive_core::ArchiveError::InvalidArchive`] on checksum
/// mismatch.
pub(crate) fn parse(
    header_data: &[u8],
) -> Result<Option<RawHeader>, omnizip_archive_core::ArchiveError> {
    use omnizip_archive_core::ArchiveError;
    if header_data.len() < HEADER_SIZE || all_zeros(header_data) {
        return Ok(None);
    }
    let name = extract_string(header_data, 0, 100);
    let mode = extract_octal(header_data, 100, 8) as u32;
    let uid = extract_octal(header_data, 108, 8) as u32;
    let gid = extract_octal(header_data, 116, 8) as u32;
    let size = extract_octal(header_data, 124, 12);
    let mtime = extract_octal(header_data, 136, 12);
    let typeflag = header_data[156];
    let linkname = extract_string(header_data, 157, 100);
    let magic = extract_string(header_data, 257, 6);
    let prefix = if magic.starts_with("ustar") {
        extract_string(header_data, 345, 155)
    } else {
        String::new()
    };

    let stored = extract_octal(header_data, 148, 8) as u32;
    let calculated = calculate_checksum(header_data);
    if stored != calculated {
        return Err(ArchiveError::InvalidArchive(format!(
            "tar header checksum mismatch for '{name}': stored {stored:o}, calculated {calculated:o}"
        )));
    }
    let _ = USTAR_MAGIC;
    Ok(Some(RawHeader {
        name,
        mode,
        uid,
        gid,
        size,
        mtime,
        typeflag,
        linkname,
        prefix,
    }))
}

/// Map a parsed raw header onto the unified entry model.
#[must_use]
pub(crate) fn to_entry(raw: &RawHeader) -> ArchiveEntry {
    let mut name = if raw.prefix.is_empty() {
        raw.name.clone()
    } else {
        format!("{}/{}", raw.prefix, raw.name)
    };
    let kind = match raw.typeflag {
        TYPE_DIRECTORY => EntryKind::Directory,
        TYPE_SYMLINK => EntryKind::Symlink(raw.linkname.clone()),
        crate::TYPE_HARD_LINK => EntryKind::HardLink(raw.linkname.clone()),
        TYPE_REGULAR | 0 => EntryKind::Regular,
        other => EntryKind::Other(other),
    };
    if matches!(kind, EntryKind::Directory) && !name.ends_with('/') && !name.is_empty() {
        name.push('/');
    }
    ArchiveEntry {
        name,
        size: Some(raw.size),
        mtime: Some(raw.mtime),
        mode: Some(raw.mode),
        kind,
        uid: Some(raw.uid),
        gid: Some(raw.gid),
        uname: String::new(),
        gname: String::new(),
        method: None,
    }
}

pub(crate) fn write_string(header: &mut [u8], value: &str, offset: usize, size: usize) {
    let bytes = value.as_bytes();
    let n = bytes.len().min(size.saturating_sub(1));
    header[offset..offset + n].copy_from_slice(&bytes[..n]);
}

pub(crate) fn write_octal(header: &mut [u8], value: u64, offset: usize, size: usize) {
    let text = format!("{:0width$o}", value, width = size - 1);
    let bytes = text.as_bytes();
    let n = bytes.len().min(size - 1);
    header[offset..offset + n].copy_from_slice(&bytes[..n]);
    header[offset + n] = 0;
}

/// Build a 512-byte ustar header for one entry.
#[must_use]
pub(crate) fn build(
    name: &str,
    mode: u32,
    uid: u32,
    gid: u32,
    size: u64,
    mtime: u64,
    typeflag: u8,
    linkname: &str,
) -> [u8; HEADER_SIZE] {
    let mut header = [0u8; HEADER_SIZE];

    // Split >100-byte names into prefix/name at a '/' boundary when
    // possible (callers handle longer names via GNU 'L' entries).
    let (prefix, short_name) = split_name(name);

    write_string(&mut header, &short_name, 0, 100);
    write_octal(&mut header, u64::from(mode), 100, 8);
    write_octal(&mut header, u64::from(uid), 108, 8);
    write_octal(&mut header, u64::from(gid), 116, 8);
    write_octal(&mut header, size, 124, 12);
    write_octal(&mut header, mtime, 136, 12);
    header[156] = if typeflag == 0 {
        TYPE_REGULAR
    } else {
        typeflag
    };
    write_string(&mut header, linkname, 157, 100);
    header[257..263].copy_from_slice(USTAR_MAGIC);
    header[263..265].copy_from_slice(USTAR_VERSION);
    write_string(&mut header, "root", 265, 32);
    write_string(&mut header, "root", 297, 32);
    write_octal(&mut header, 0, 329, 8);
    write_octal(&mut header, 0, 337, 8);
    write_string(&mut header, &prefix, 345, 155);

    let checksum = calculate_checksum(&header);
    let text = format!("{:06o}\0 ", checksum);
    header[148..156].copy_from_slice(&text.as_bytes()[..8]);
    header
}

/// Split `name` for the ustar name+prefix fields. Returns
/// (prefix, name) where both fit their fields, or (whole, truncated)
/// if impossible — callers must route over-long names to GNU 'L'.
#[must_use]
pub(crate) fn split_name(name: &str) -> (String, String) {
    let bytes = name.as_bytes();
    if bytes.len() <= 100 {
        return (String::new(), name.to_string());
    }
    // Find a '/' split so name ≤ 100 and prefix ≤ 155.
    let mut best: Option<(usize, usize)> = None;
    for (i, &b) in bytes.iter().enumerate() {
        if b != b'/' {
            continue;
        }
        let prefix_len = i;
        let name_len = bytes.len() - i - 1;
        if prefix_len <= 155 && name_len <= 100 && prefix_len > 0 && name_len > 0 {
            best = Some(best.map_or((prefix_len, name_len), |(bp, bn)| {
                if name_len > bn {
                    (prefix_len, name_len)
                } else {
                    (bp, bn)
                }
            }));
        }
    }
    match best {
        Some((pl, _)) => {
            let prefix = String::from_utf8_lossy(&bytes[..pl]).into_owned();
            let rest = String::from_utf8_lossy(&bytes[pl + 1..]).into_owned();
            (prefix, rest)
        }
        None => (String::new(), name.chars().take(99).collect()),
    }
}

/// Pad `len` up to the 512-byte block boundary.
#[must_use]
pub(crate) fn padding_len(len: usize) -> usize {
    let rem = len % BLOCK_SIZE;
    if rem == 0 {
        0
    } else {
        BLOCK_SIZE - rem
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_parse_round_trip() {
        let h = build(
            "dir/file.txt",
            0o644,
            1000,
            1000,
            12,
            1_700_000_000,
            TYPE_REGULAR,
            "",
        );
        let raw = parse(&h).unwrap().unwrap();
        assert_eq!(raw.name, "dir/file.txt");
        assert_eq!(raw.mode, 0o644);
        assert_eq!(raw.size, 12);
        assert_eq!(raw.mtime, 1_700_000_000);
        assert_eq!(raw.typeflag, TYPE_REGULAR);
    }

    #[test]
    fn checksum_mismatch_rejected() {
        let mut h = build("f", 0o644, 0, 0, 0, 0, TYPE_REGULAR, "");
        h[0] = b'X';
        assert!(parse(&h).is_err());
    }

    #[test]
    fn zero_block_is_end_marker() {
        assert!(parse(&[0u8; 512]).unwrap().is_none());
    }

    #[test]
    fn prefix_split() {
        let long = format!("{}/{}", "a".repeat(100), "f".repeat(50));
        let h = build(&long, 0o644, 0, 0, 0, 0, TYPE_REGULAR, "");
        let raw = parse(&h).unwrap().unwrap();
        assert_eq!(to_entry(&raw).name, long);
    }
}
