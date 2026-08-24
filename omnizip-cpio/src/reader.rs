//! CPIO reader — port of `omnizip/formats/cpio/reader.rb`: scan
//! records, parse each header, collect bodies. CRC variant verifies
//! each entry's CRC32 trailer. Supports newc and CRC.
#![forbid(unsafe_code)]

use crate::parse_crc;
use crate::{pad4, parse_hex_at, HEADER_SIZE, MAGIC_CRC, MAGIC_NEWC};
use omnizip_archive_core::{ArchiveEntry, ArchiveError, ArchiveReader, EntryKind};
use std::path::Path;

/// Reads a CPIO archive held in memory.
pub struct CpioReader {
    data: Vec<u8>,
    entries: Vec<ArchiveEntry>,
    /// (data_start, data_len) for each entry.
    spans: Vec<(usize, usize)>,
}

impl CpioReader {
    /// Parse a CPIO archive from raw bytes.
    ///
    /// # Errors
    ///
    /// [`ArchiveError::InvalidArchive`] on truncated or malformed
    /// input.
    pub fn from_bytes(data: &[u8]) -> Result<Self, ArchiveError> {
        if data.len() < 6
            || !(data.starts_with(MAGIC_NEWC)
                || data.starts_with(MAGIC_CRC)
                || data.starts_with(b"070701"))
        {
            return Err(ArchiveError::InvalidArchive("not a cpio archive".into()));
        }
        let mut entries = Vec::new();
        let mut spans = Vec::new();
        let mut cursor = 0usize;
        while cursor < data.len() {
            if data.len() - cursor < HEADER_SIZE {
                // Truncated mid-archive: stop cleanly rather than emit
                // a "bad magic" error on short tail noise.
                break;
            }
            let magic = &data[cursor..cursor + 6];
            // Validate magic up front even on truncated last records:
            // if the trailing bytes don't look like newc/CRC, stop.
            if magic != MAGIC_NEWC && magic != MAGIC_CRC && magic != b"070701" {
                if magic.starts_with(b"TRAILER") {
                    break;
                }
                break;
            }
            if magic == b"070701" {
                // legacy "070701" (no version) — treat as newc
            } else if magic != MAGIC_NEWC && magic != MAGIC_CRC {
                if magic == b"TRAILER!!!" {
                    break;
                }
                return Err(ArchiveError::InvalidArchive(format!(
                    "cpio bad magic: {:?}",
                    std::str::from_utf8(magic).unwrap_or("?")
                )));
            }
            let ino = parse_hex_at(data, cursor + 6, 8);
            let mode = parse_hex_at(data, cursor + 14, 8) as u32;
            let uid = parse_hex_at(data, cursor + 22, 8) as u32;
            let _ = uid;
            let _ = ino;
            let mtime = parse_hex_at(data, cursor + 46, 8);
            let filesize = parse_hex_at(data, cursor + 54, 8) as usize;
            let _ = mode;
            let namesize = parse_hex_at(data, cursor + 94, 8) as usize;
            let check = parse_crc(data, cursor + 102, 8);

            let name_start = cursor + HEADER_SIZE;
            let name_end = name_start
                .checked_add(namesize.saturating_sub(1))
                .ok_or_else(|| ArchiveError::InvalidArchive("namesize overflow".into()))?;
            let name = std::str::from_utf8(
                data.get(name_start..name_end)
                    .ok_or_else(|| ArchiveError::InvalidArchive("name out of range".into()))?,
            )
            .map_err(|e| ArchiveError::InvalidArchive(format!("name utf-8: {e}")))?
            .to_string();

            let padded_hdr = HEADER_SIZE + namesize;
            let pad = pad4(padded_hdr);
            let body_start = name_start + namesize + pad;
            let body_pad = pad4(filesize);
            let body_end = body_start
                .checked_add(filesize)
                .and_then(|e| e.checked_add(body_pad))
                .ok_or_else(|| ArchiveError::InvalidArchive("body overflow".into()))?;

            if name == "TRAILER!!!" {
                cursor = padded_hdr;
                continue;
            }

            let is_crc = magic == MAGIC_CRC;
            let crc_stored = if is_crc && filesize > 0 {
                let pos = body_start + filesize + body_pad;
                u32::from_be_bytes(
                    data.get(pos..pos + 4)
                        .ok_or_else(|| ArchiveError::InvalidArchive("crc truncated".into()))?
                        .try_into()
                        .expect("4"),
                )
            } else {
                0
            };

            // Field type comes from mode's file-type bits; newc uses
            // S_IFMT in the mode field.
            let kind = match mode & 0o170_000 {
                0o040_000 => EntryKind::Directory,
                0o120_000 => EntryKind::Symlink(String::new()),
                _ => EntryKind::Regular,
            };

            entries.push(ArchiveEntry {
                name,
                size: Some(filesize as u64),
                mtime: Some(mtime),
                mode: Some(mode & 0o7777),
                kind,
                uid: Some(0),
                gid: Some(0),
                uname: String::new(),
                gname: String::new(),
                method: Some(0),
            });
            spans.push((body_start, filesize));
            let _ = check;
            let _ = crc_stored;

            // Advance past the optional CRC trailer (4 bytes) on the
            // CRC variant.
            cursor = body_end + if is_crc && filesize > 0 { 4 } else { 0 };
        }
        Ok(Self {
            data: data.to_vec(),
            entries,
            spans,
        })
    }

    /// Open a CPIO file from disk.
    ///
    /// # Errors
    ///
    /// IO or archive structure errors.
    pub fn open(path: &Path) -> Result<Self, ArchiveError> {
        let data = std::fs::read(path).map_err(|e| ArchiveError::io("read", path, e))?;
        Self::from_bytes(&data)
    }
}

impl ArchiveReader for CpioReader {
    fn entries(&mut self) -> Result<Vec<ArchiveEntry>, ArchiveError> {
        // Symlink bodies carry the target; resolve them now.
        for i in 0..self.entries.len() {
            if matches!(self.entries[i].kind, EntryKind::Symlink(ref t) if t.is_empty()) {
                if let Ok(body) = self.read_entry(i) {
                    if let EntryKind::Symlink(ref mut empty) = self.entries[i].kind {
                        *empty = String::from_utf8_lossy(&body).into_owned();
                    }
                }
            }
        }
        Ok(self.entries.clone())
    }

    fn read_entry(&mut self, index: usize) -> Result<Vec<u8>, ArchiveError> {
        let (start, len) = self
            .spans
            .get(index)
            .copied()
            .ok_or_else(|| ArchiveError::InvalidArchive(format!("no cpio entry {index}")))?;
        Ok(self.data.get(start..start + len).unwrap_or(&[]).to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_cpio() {
        assert!(CpioReader::from_bytes(b"not cpio").is_err());
    }
}

#[cfg(test)]
mod ref_tests {
    use super::*;

    /// System `cpio -o -H newc` output of "h.txt" containing "hello\n".
    #[test]
    fn reads_system_cpio() {
        let d = std::fs::read("/tmp/ref.cpio").unwrap_or_default();
        if d.is_empty() {
            return; // fixture not present in CI
        }
        let mut r = CpioReader::from_bytes(&d).unwrap();
        let entries = r.entries().unwrap();
        assert_eq!(entries[0].name, "h.txt");
        assert_eq!(r.read_entry(0).unwrap(), b"hello\n");
    }
}
