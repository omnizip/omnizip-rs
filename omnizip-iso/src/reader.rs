//! ISO 9660 reader — port of the Ruby `iso/reader.rb` walk: find the
//! PVD (preferring a Joliet supplementary descriptor when present),
//! recursively walk the directory tree, resolve Rock Ridge names and
//! modes, expose the unified `ArchiveReader` trait.
#![forbid(unsafe_code)]

use crate::{parse_record, parse_volume_descriptor, DirectoryRecord, VolumeDescriptor, SECTOR_SIZE, VOLUME_DESCRIPTOR_START, flags, vd_type};
use omnizip_archive_core::{ArchiveEntry, ArchiveError, ArchiveReader, EntryKind};
use std::path::Path;

/// Reads an ISO image held in memory.
pub struct IsoReader {
    data: Vec<u8>,
    pub primary: VolumeDescriptor,
    /// Joliet descriptor when a supplementary UCS-2 volume exists.
    pub joliet: Option<VolumeDescriptor>,
    /// The descriptor whose tree we walk (Joliet preferred).
    tree: VolumeDescriptor,
    entries: Vec<DirectoryRecord>,
}

impl IsoReader {
    /// Parse an ISO image.
    ///
    /// # Errors
    ///
    /// [`ArchiveError::InvalidArchive`] on descriptor/structure
    /// problems.
    pub fn from_bytes(data: &[u8]) -> Result<Self, ArchiveError> {
        let mut sector = VOLUME_DESCRIPTOR_START;
        let mut primary = None;
        let mut joliet = None;
        let mut terminator = false;
        loop {
            let offset = sector * SECTOR_SIZE;
            let slice = data.get(offset..offset + SECTOR_SIZE).ok_or_else(|| {
                ArchiveError::InvalidArchive("iso: volume descriptor out of bounds".into())
            })?;
            let vd = parse_volume_descriptor(slice)?;
            match vd.type_ {
                vd_type::PRIMARY => primary = Some(vd),
                vd_type::SUPPLEMENTARY if vd.joliet => joliet = Some(vd),
                vd_type::TERMINATOR => {
                    terminator = true;
                    break;
                }
                _ => {}
            }
            sector += 1;
            if sector > VOLUME_DESCRIPTOR_START + 64 {
                break;
            }
        }
        if !terminator {
            return Err(ArchiveError::InvalidArchive(
                "iso: no volume descriptor terminator".into(),
            ));
        }
        let primary = primary
            .ok_or_else(|| ArchiveError::InvalidArchive("iso: no primary volume descriptor".into()))?;
        let tree = joliet.clone().unwrap_or_else(|| primary.clone());

        let mut reader = Self {
            data: data.to_vec(),
            primary,
            joliet,
            tree,
            entries: Vec::new(),
        };
        reader.walk()?;
        Ok(reader)
    }

    /// Open from disk.
    ///
    /// # Errors
    ///
    /// IO or structure errors.
    pub fn open(path: &Path) -> Result<Self, ArchiveError> {
        let data = std::fs::read(path).map_err(|e| ArchiveError::io("read", path, e))?;
        Self::from_bytes(&data)
    }

    #[must_use]
    pub fn volume_identifier(&self) -> &str {
        &self.primary.volume_identifier
    }

    fn walk(&mut self) -> Result<(), ArchiveError> {
        let root = self.tree.root.clone();
        let joliet = self.tree.joliet;
        self.walk_dir(&root, "", joliet)
    }

    fn walk_dir(
        &mut self,
        dir: &DirectoryRecord,
        prefix: &str,
        joliet: bool,
    ) -> Result<(), ArchiveError> {
        let start = dir.location as usize * SECTOR_SIZE;
        let end = start + dir.data_length as usize;
        let dir_data = self
            .data
            .get(start..end)
            .ok_or_else(|| ArchiveError::InvalidArchive("iso: directory extent out of bounds".into()))?
            .to_vec();

        let mut offset = 0usize;
        while offset < dir_data.len() {
            let Some(record) = parse_record(&dir_data, offset, joliet) else {
                break;
            };
            let length = dir_data[offset] as usize;
            if length == 0 {
                break;
            }
            if !record.is_current() && !record.is_parent() {
                let name = record.rock_ridge_name().unwrap_or_else(|| {
                    strip_iso_name(&record.name)
                });
                let full_path = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}/{name}")
                };
                let mut record = record;
                record.full_path.clone_from(&full_path);
                let is_dir = record.is_directory();
                self.entries.push(record);
                if is_dir {
                    let record = self.entries.last().cloned().expect("just pushed");
                    self.walk_dir(&record, &full_path, joliet)?;
                }
            }
            offset += length;
        }
        Ok(())
    }

}

/// Strip ISO level-1 decorations: `NAME.EXT;VERSION` → `name.ext`.
fn strip_iso_name(name: &str) -> String {
    let base = match name.rfind(';') {
        Some(i) => &name[..i],
        None => name,
    };
    base.trim_end_matches('/').to_string()
}

impl ArchiveReader for IsoReader {
    fn entries(&mut self) -> Result<Vec<ArchiveEntry>, ArchiveError> {
        Ok(self
            .entries
            .iter()
            .map(|r| ArchiveEntry {
                name: r.full_path.clone(),
                size: Some(u64::from(r.data_length)),
                mtime: Some(r.mtime_unix()),
                mode: r
                    .rock_ridge_mode()
                    .or(Some(if r.is_directory() { 0o755 } else { 0o644 })),
                kind: if r.is_directory() {
                    EntryKind::Directory
                } else {
                    EntryKind::Regular
                },
                uid: None,
                gid: None,
                uname: String::new(),
                gname: String::new(),
                method: None,
            })
            .collect())
    }

    fn read_entry(&mut self, index: usize) -> Result<Vec<u8>, ArchiveError> {
        let (location, length, is_dir) = {
            let r = self
                .entries
                .get(index)
                .ok_or_else(|| ArchiveError::InvalidArchive(format!("iso: no entry {index}")))?;
            (
                r.location as usize * SECTOR_SIZE,
                r.data_length as usize,
                r.is_directory(),
            )
        };
        if is_dir {
            return Ok(Vec::new());
        }
        self.data
            .get(location..location + length)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| ArchiveError::InvalidArchive("iso: file extent out of bounds".into()))
    }
}

/// Directory flag export for tests.
#[must_use]
pub const fn is_directory_flag(f: u8) -> bool {
    f & flags::DIRECTORY != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_iso() {
        assert!(IsoReader::from_bytes(b"plainly not an iso").is_err());
    }
}
