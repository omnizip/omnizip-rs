//! CFB reader — DIFAT/FAT chain walking, directory tree traversal
//! (storages as path components), stream reads through the regular
//! FAT or the mini stream.
#![forbid(unsafe_code)]

use crate::{fat, obj_type, parse_dir_entry, parse_header, CfbHeader, DirEntry, MINI_CUTOFF};
use omnizip_archive_core::{ArchiveEntry, ArchiveError, ArchiveReader, EntryKind};
use std::path::Path;

/// Reads a compound file held in memory.
pub struct OleReader {
    data: Vec<u8>,
    header: CfbHeader,
    /// The flat directory sector chain decoded into entries.
    dir: Vec<DirEntry>,
    /// Full FAT (sector id -> next).
    fat: Vec<u32>,
    /// Mini FAT (mini sector id -> next).
    minifat: Vec<u32>,
    /// The root entry's mini stream (lazily read).
    mini_stream: Vec<u8>,
    /// (path, dir index) for every stream.
    streams: Vec<(String, usize)>,
}

impl OleReader {
    /// Parse a compound file.
    ///
    /// # Errors
    ///
    /// [`ArchiveError`] on structure problems.
    pub fn from_bytes(data: &[u8]) -> Result<Self, ArchiveError> {
        let header = parse_header(data)?;
        let sector = header.sector_size();

        // FAT sectors from the (first 109 slots of the) DIFAT.
        let mut fat = Vec::new();
        for &fs in header.difat.iter() {
            if fs == fat::FREESECT {
                continue;
            }
            let off = header.sector_offset(fs);
            let s = data.get(off..off + sector).ok_or_else(|| {
                ArchiveError::InvalidArchive("ole: FAT sector out of bounds".into())
            })?;
            fat.extend(
                s.chunks_exact(4)
                    .map(|c| u32::from_le_bytes(c.try_into().expect("4"))),
            );
        }

        // Directory chain.
        let dir_bytes = read_chain(data, &header, &fat, header.first_dir_sector)?;
        let mut dir = Vec::with_capacity(dir_bytes.len() / 128);
        for i in 0..dir_bytes.len() / 128 {
            if dir_bytes[i * 128 + 66] != obj_type::UNKNOWN {
                dir.push(parse_dir_entry(&dir_bytes, i * 128)?);
            } else {
                dir.push(DirEntry::default());
            }
        }

        // Mini FAT.
        let mut minifat = Vec::new();
        if header.num_minifat_sectors > 0 && header.first_minifat_sector != fat::ENDOFCHAIN {
            let mf = read_chain(data, &header, &fat, header.first_minifat_sector)?;
            minifat.extend(
                mf.chunks_exact(4)
                    .map(|c| u32::from_le_bytes(c.try_into().expect("4"))),
            );
        }

        let root = dir
            .first()
            .ok_or_else(|| ArchiveError::InvalidArchive("ole: no root entry".into()))?;
        let mini_stream = if root.object_type == obj_type::ROOT
            && root.size > 0
            && root.start_sector != fat::ENDOFCHAIN
        {
            read_chain(data, &header, &fat, root.start_sector)?
        } else {
            Vec::new()
        };

        let mut reader = Self {
            data: data.to_vec(),
            header,
            dir,
            fat,
            minifat,
            mini_stream,
            streams: Vec::new(),
        };
        reader.index_streams()?;
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

    fn index_streams(&mut self) -> Result<(), ArchiveError> {
        if self.dir.is_empty() {
            return Ok(());
        }
        let root_child = self.dir[0].child;
        let mut out = Vec::new();
        if root_child != fat::NOSTREAM {
            self.walk_tree(root_child as usize, "", &mut out)?;
        }
        self.streams = out;
        Ok(())
    }

    /// In-order walk of a storage's sibling BST.
    fn walk_tree(
        &self,
        index: usize,
        prefix: &str,
        out: &mut Vec<(String, usize)>,
    ) -> Result<(), ArchiveError> {
        let entry = self.dir.get(index).ok_or_else(|| {
            ArchiveError::InvalidArchive(format!("ole: directory entry {index} missing"))
        })?;
        if entry.left != fat::NOSTREAM && entry.left as usize != index {
            self.walk_tree(entry.left as usize, prefix, out)?;
        }
        let path = if prefix.is_empty() {
            entry.name.clone()
        } else {
            format!("{prefix}/{}", entry.name)
        };
        match entry.object_type {
            obj_type::STREAM => out.push((path, index)),
            obj_type::STORAGE | obj_type::ROOT => {
                if entry.child != fat::NOSTREAM {
                    self.walk_tree(entry.child as usize, &path, out)?;
                }
            }
            _ => {}
        }
        if entry.right != fat::NOSTREAM && entry.right as usize != index {
            self.walk_tree(entry.right as usize, prefix, out)?;
        }
        Ok(())
    }

    /// Read one stream by full path (e.g. `Storage/Stream`).
    ///
    /// # Errors
    ///
    /// [`ArchiveError`] when the stream does not exist.
    pub fn read_stream(&self, path: &str) -> Result<Vec<u8>, ArchiveError> {
        let index = self
            .streams
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, i)| *i)
            .ok_or_else(|| ArchiveError::InvalidArchive(format!("ole: no stream '{path}'")))?;
        let entry = &self.dir[index];
        if entry.size == 0 || entry.start_sector == fat::ENDOFCHAIN {
            return Ok(Vec::new());
        }
        if entry.size < u64::from(self.header.mini_stream_cutoff.max(MINI_CUTOFF)) {
            self.read_mini(entry)
        } else {
            let mut out = read_chain(&self.data, &self.header, &self.fat, entry.start_sector)?;
            out.truncate(entry.size as usize);
            Ok(out)
        }
    }

    fn read_mini(&self, entry: &DirEntry) -> Result<Vec<u8>, ArchiveError> {
        let msize = self.header.mini_sector_size();
        let mut out = Vec::with_capacity(entry.size as usize);
        let mut sect = entry.start_sector;
        let mut guard = 0usize;
        while sect != fat::ENDOFCHAIN {
            let start = sect as usize * msize;
            let end = (start + msize).min(self.mini_stream.len());
            out.extend_from_slice(self.mini_stream.get(start..end).ok_or_else(|| {
                ArchiveError::InvalidArchive("ole: mini sector out of bounds".into())
            })?);
            sect = *self
                .minifat
                .get(sect as usize)
                .ok_or_else(|| ArchiveError::InvalidArchive("ole: minifat truncated".into()))?;
            guard += 1;
            if guard > 1 << 24 {
                return Err(ArchiveError::InvalidArchive("ole: minifat loop".into()));
            }
        }
        out.truncate(entry.size as usize);
        Ok(out)
    }

    /// All stream paths (storage paths included as directories).
    #[must_use]
    pub fn stream_paths(&self) -> Vec<(String, bool, u64)> {
        let mut out: Vec<(String, bool, u64)> = self
            .streams
            .iter()
            .map(|(p, i)| (p.clone(), false, self.dir[*i].size))
            .collect();
        // Include storages as directory entries (derived from stream
        // path prefixes).
        let mut seen = std::collections::BTreeSet::new();
        for (path, _, _) in &out {
            let mut p = path.as_str();
            while let Some(i) = p.rfind('/') {
                p = &p[..i];
                if !p.is_empty() {
                    seen.insert(p.to_string());
                }
            }
        }
        for s in seen {
            out.push((s, true, 0));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}

/// Read a full sector chain as bytes.
fn read_chain(
    data: &[u8],
    header: &CfbHeader,
    fat: &[u32],
    start: u32,
) -> Result<Vec<u8>, ArchiveError> {
    let sector = header.sector_size();
    let mut out = Vec::new();
    let mut sect = start;
    let mut guard = 0usize;
    while sect != fat::ENDOFCHAIN {
        let off = header.sector_offset(sect);
        out.extend_from_slice(
            data.get(off..off + sector)
                .ok_or_else(|| ArchiveError::InvalidArchive("ole: sector out of bounds".into()))?,
        );
        sect = *fat
            .get(sect as usize)
            .ok_or_else(|| ArchiveError::InvalidArchive("ole: fat truncated".into()))?;
        guard += 1;
        if guard > fat.len().max(1) {
            return Err(ArchiveError::InvalidArchive("ole: fat loop".into()));
        }
    }
    Ok(out)
}

impl ArchiveReader for OleReader {
    fn entries(&mut self) -> Result<Vec<ArchiveEntry>, ArchiveError> {
        Ok(self
            .stream_paths()
            .into_iter()
            .map(|(name, is_dir, size)| ArchiveEntry {
                name,
                size: Some(size),
                mtime: None,
                mode: Some(if is_dir { 0o755 } else { 0o644 }),
                kind: if is_dir {
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
        let paths = self.stream_paths();
        let (name, is_dir, _) = paths
            .get(index)
            .cloned()
            .ok_or_else(|| ArchiveError::InvalidArchive(format!("ole: no entry {index}")))?;
        if is_dir {
            return Ok(Vec::new());
        }
        self.read_stream(&name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_ole() {
        assert!(OleReader::from_bytes(b"surely not an ole file").is_err());
    }
}
